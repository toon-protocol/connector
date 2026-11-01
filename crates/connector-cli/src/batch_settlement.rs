//! Where a node's x402 `batch-settlement` backends meet its client edge
//! (ADR 0074, epic #1349): the adapter from the receive-only settlement port
//! ([`BatchSettlementBackend`], #1340) to the claim gate's seam
//! ([`BatchSettlementChannels`], #1341), the boot step that restores
//! every channel the client edge's journal holds vouchers on, and the view
//! of the claim gate the watchers and sweeps (#1344) read the latest voucher
//! on each channel from ([`ClaimGateVouchers`]).
//!
//! Here rather than in either crate it joins: `connector-client-edge` does
//! not depend on the settlement crates and should not, and ADR 0001 puts
//! construction in `connector-cli`.
//!
//! **Thin on purpose.** Admission, collateral and the chain reads are the
//! port's; freshness, the signature and the journal are the gate's. This
//! translates one vocabulary into the other and decides nothing but how a
//! port error reads to the gate.

use std::collections::HashSet;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;
use connector_client_edge::{
    AdmittedEvmVoucherChannel, AdmittedSolanaVoucherChannel, AdmittedXrplVoucherChannel,
    BatchSettlementChannels, ChannelLookupFailed, ChannelResolutionError, ChannelTerminal,
    ClientClaimGate, JournaledBatchChannel,
};
use connector_domain::{Network, XrplNetwork};
use connector_runtime::network_of_presentation;
use connector_settlement::batch::{
    BatchChannelState, BatchSettlementBackend, BatchSettlementError, ChannelPresentation,
    EvmChannelConfig, HeldVoucher, HeldVouchers, Voucher, VoucherSigner,
};
use connector_settlement::ChannelId;
use connector_signer::{BatchChannelConfig, BatchSettlementDomain};

/// This node's batch-settlement backends, one per network it has opted in
/// on -- each `[settlement.eip155.<chainId>]` table and the one
/// `[settlement.solana]` (ADR 0086) -- as the claim gate asks them. A
/// network whose table is absent has no leg here, and the gate refuses its
/// vouchers by name.
pub(crate) struct BatchSettlementChannelsAdapter {
    /// One leg per EVM network, in table order.
    evm: Vec<EvmLeg>,
    /// The Solana backend, and the CAIP-2 network it reached at connect.
    solana: Option<(Arc<dyn BatchSettlementBackend>, Network)>,
    /// One leg per XRPL network, in table order (ADR 0087).
    xrpl: Vec<(Arc<dyn BatchSettlementBackend>, XrplNetwork)>,
    /// Every channel this adapter has seen `admit` accept in this process.
    /// A backend also answers for channels it only **restored** -- ones the
    /// journal holds vouchers on, brought back at boot without judging them
    /// (ADR 0074 decision 5) -- so "the backend can read it" is not "a new
    /// voucher may be accepted on it". A channel absent here is admitted,
    /// under this node's current rules, before its first new voucher.
    admitted: Mutex<HashSet<ChannelId>>,
}

struct EvmLeg {
    backend: Arc<dyn BatchSettlementBackend>,
    /// The domain EVM vouchers are verified under: this chain and the
    /// configured `x402BatchSettlement`, both from the backend that proved
    /// them against the chain at connect.
    domain: BatchSettlementDomain,
}

impl BatchSettlementChannelsAdapter {
    /// `None` when no network has opted in: a gate given nothing refuses
    /// every voucher by name, which is what "off unless configured" means.
    pub(crate) fn new(
        evm: Vec<(Arc<dyn BatchSettlementBackend>, BatchSettlementDomain)>,
        solana: Option<(Arc<dyn BatchSettlementBackend>, Network)>,
        xrpl: Vec<(Arc<dyn BatchSettlementBackend>, XrplNetwork)>,
    ) -> Option<BatchSettlementChannelsAdapter> {
        if evm.is_empty() && solana.is_none() && xrpl.is_empty() {
            return None;
        }
        Some(BatchSettlementChannelsAdapter {
            evm: evm
                .into_iter()
                .map(|(backend, domain)| EvmLeg { backend, domain })
                .collect(),
            solana,
            xrpl,
            admitted: Mutex::new(HashSet::new()),
        })
    }

    /// The leg of EVM network `network`, if this node settles on it.
    fn evm_leg(&self, network: &Network) -> Option<&EvmLeg> {
        let chain_id = network.eip155_chain_id()?;
        self.evm.iter().find(|leg| leg.domain.chain_id == chain_id)
    }
}

impl fmt::Debug for BatchSettlementChannelsAdapter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BatchSettlementChannelsAdapter")
            .field(
                "evm",
                &self.evm.iter().map(|leg| leg.domain).collect::<Vec<_>>(),
            )
            .field("solana", &self.solana.is_some())
            .field(
                "xrpl",
                &self
                    .xrpl
                    .iter()
                    .map(|(_, network)| network)
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

#[async_trait]
impl BatchSettlementChannels for BatchSettlementChannelsAdapter {
    fn evm_domains(&self) -> Vec<BatchSettlementDomain> {
        self.evm.iter().map(|leg| leg.domain).collect()
    }

    fn solana_network(&self) -> Option<Network> {
        self.solana.as_ref().map(|(_, network)| network.clone())
    }

    fn xrpl_networks(&self) -> Vec<XrplNetwork> {
        self.xrpl.iter().map(|(_, network)| *network).collect()
    }

    /// An XRPL channel is found from its id alone: every field is on the
    /// ledger. Asked of the backend of the network the voucher names and
    /// no other (ADR 0087 decision 2).
    async fn xrpl(
        &self,
        network: &XrplNetwork,
        channel_id: &[u8; 32],
    ) -> Result<Option<AdmittedXrplVoucherChannel>, ChannelResolutionError> {
        let Some((backend, _)) = self.xrpl.iter().find(|(_, held)| held == network) else {
            return Ok(None);
        };
        let presentation = ChannelPresentation::Xrpl {
            channel: xrpl_channel(channel_id),
        };
        let state = self.state_of(backend.as_ref(), presentation).await;
        Ok(
            resolution(state)?.and_then(|state| match state.voucher_signer {
                VoucherSigner::Xrpl(public_key) => Some(AdmittedXrplVoucherChannel {
                    public_key,
                    max_cumulative: max_cumulative_xrpl(&state),
                }),
                // An XRPL backend reporting another chain's signer is not a
                // channel this gate can verify a voucher against.
                VoucherSigner::Evm(_) | VoucherSigner::Solana(_) => None,
            }),
        )
    }

    /// An EVM channel is found only from its config: the contract stores a
    /// hash. With none -- a voucher that carried none, on a channel the gate
    /// has no record of -- there is nothing to look up.
    async fn evm(
        &self,
        network: &Network,
        channel_id: &[u8; 32],
        presented_config: Option<&BatchChannelConfig>,
    ) -> Result<Option<AdmittedEvmVoucherChannel>, ChannelResolutionError> {
        // ADR 0086: asked of `network`'s backend and no other.
        let (Some(leg), Some(config)) = (self.evm_leg(network), presented_config) else {
            return Ok(None);
        };
        let presentation = ChannelPresentation::Evm {
            channel: evm_channel(channel_id),
            config: port_config(config),
        };
        let state = self.state_of(leg.backend.as_ref(), presentation).await;
        Ok(resolution(state)?.map(|state| AdmittedEvmVoucherChannel {
            config: *config,
            max_cumulative: max_cumulative_evm(&state),
        }))
    }

    async fn solana(
        &self,
        channel_account: &[u8; 32],
    ) -> Result<Option<AdmittedSolanaVoucherChannel>, ChannelResolutionError> {
        let Some((backend, _)) = &self.solana else {
            return Ok(None);
        };
        let presentation = ChannelPresentation::Solana {
            channel: solana_channel(channel_account),
        };
        let state = self.state_of(backend.as_ref(), presentation).await;
        Ok(
            resolution(state)?.and_then(|state| match state.voucher_signer {
                VoucherSigner::Solana(authorized_signer) => Some(AdmittedSolanaVoucherChannel {
                    authorized_signer,
                    max_cumulative: max_cumulative_solana(&state),
                }),
                // A Solana backend reporting another chain's signer is not a
                // channel this gate can verify a voucher against.
                VoucherSigner::Evm(_) | VoucherSigner::Xrpl(_) => None,
            }),
        )
    }
}

impl BatchSettlementChannelsAdapter {
    /// The channel's state now, for a voucher the gate is about to accept:
    /// one read for a channel admitted in this process, and admission --
    /// which reads it too -- for one that was not. A channel the backend
    /// only restored is admitted here like any other, so a rule tightened
    /// since its vouchers were accepted refuses its new ones.
    async fn state_of(
        &self,
        backend: &dyn BatchSettlementBackend,
        presentation: ChannelPresentation,
    ) -> Result<BatchChannelState, BatchSettlementError> {
        let channel = presentation.channel().clone();
        if self.admitted().contains(&channel) {
            match backend.channel_state(&channel).await {
                Err(BatchSettlementError::ChannelNotAdmitted(_)) => {}
                read => return read,
            }
        }
        let state = backend.admit(presentation).await?;
        self.admitted().insert(channel);
        Ok(state)
    }

    fn admitted(&self) -> MutexGuard<'_, HashSet<ChannelId>> {
        self.admitted
            .lock()
            .expect("batch-settlement adapter lock poisoned")
    }
}

/// How a port answer reads to the gate.
///
/// * A channel that accepts vouchers is found.
/// * One that no longer accepts them -- Solana `Closing` or `Sealed` -- is
///   **terminal**: it exists, and no new voucher may be accepted on it (ADR
///   0074 decisions 3 and 5).
/// * One that does not exist, or that this node does not admit, is not
///   found: the gate refuses it as an unknown channel, which is the seam's
///   contract. The admission refusal is logged, since an operator whose
///   clients keep opening channels this node will not take wants to know
///   why.
/// * A chain that could not be read is a failed lookup, never an absence.
fn resolution(
    state: Result<BatchChannelState, BatchSettlementError>,
) -> Result<Option<BatchChannelState>, ChannelResolutionError> {
    match state {
        Ok(state) if state.status.accepts_vouchers() => Ok(Some(state)),
        Ok(state) => Err(ChannelResolutionError::Terminal(ChannelTerminal(format!(
            "batch-settlement channel '{}' is {:?} and accepts no new voucher",
            state.id, state.status
        )))),
        Err(BatchSettlementError::NotAdmissible { channel, refusal }) => {
            tracing::info!(
                %channel,
                %refusal,
                "refusing a voucher: its batch-settlement channel is not one this node admits"
            );
            Ok(None)
        }
        Err(
            BatchSettlementError::ChannelNotFound(_)
            | BatchSettlementError::ChannelIdMismatch { .. }
            | BatchSettlementError::WrongChain { .. }
            | BatchSettlementError::ChannelNotAdmitted(_),
        ) => Ok(None),
        Err(BatchSettlementError::ChannelSealed(channel)) => Err(ChannelResolutionError::Terminal(
            ChannelTerminal(format!("batch-settlement channel '{channel}' is sealed")),
        )),
        Err(error) => Err(ChannelResolutionError::LookupFailed(ChannelLookupFailed(
            error.to_string(),
        ))),
    }
}

/// The port's voucher ceiling, in the gate's `u128` EVM amounts (ADR 0074
/// decision 3, amended 2026-09-30 #1429): EVM's amount is `u128`, matching
/// `x402BatchSettlement`'s own `maxClaimableAmount`, so the ceiling carries
/// through exactly -- nothing narrows it here any more.
fn max_cumulative_evm(state: &BatchChannelState) -> u128 {
    state.voucher_ceiling()
}

/// The port's voucher ceiling, in the gate's `u64` Solana amounts. Solana's
/// signed message and SPL amounts stay `u64` (ADR 0074 decision 3, amended
/// 2026-09-30 #1429), so a ceiling wider than any `u64` bounds nothing a
/// Solana voucher here can name, and it saturates.
///
/// Saturating is right here and nowhere else a `u128` narrows: this is a
/// bound, and clamping a bound down loses no value, where a voucher amount
/// narrowed must never be dropped or truncated. The EVM leg above no longer
/// narrows at all.
fn max_cumulative_solana(state: &BatchChannelState) -> u64 {
    u64::try_from(state.voucher_ceiling()).unwrap_or(u64::MAX)
}

/// The port's voucher ceiling, in the gate's `u64` XRPL amounts: drops.
/// XRP's whole supply is inside `u64` (ADR 0087 decision 2), so the
/// saturation here only bounds a figure the ledger could not hold.
fn max_cumulative_xrpl(state: &BatchChannelState) -> u64 {
    u64::try_from(state.voucher_ceiling()).unwrap_or(u64::MAX)
}

/// An XRPL channel id as the port spells it: 64 lowercase hex, no prefix.
fn xrpl_channel(channel_id: &[u8; 32]) -> ChannelId {
    ChannelId(
        channel_id
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    )
}

/// An EVM channel id as the port spells it: `0x` and 64 lowercase hex.
fn evm_channel(channel_id: &[u8; 32]) -> ChannelId {
    ChannelId(format!(
        "0x{}",
        channel_id
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    ))
}

/// A Solana channel account as the port spells it: base58.
fn solana_channel(channel_account: &[u8; 32]) -> ChannelId {
    ChannelId(solana_sdk::pubkey::Pubkey::new_from_array(*channel_account).to_string())
}

/// The gate's config as the port carries it: the same seven fields.
fn port_config(config: &BatchChannelConfig) -> EvmChannelConfig {
    EvmChannelConfig {
        payer: config.payer,
        payer_authorizer: config.payer_authorizer,
        receiver: config.receiver,
        receiver_authorizer: config.receiver_authorizer,
        token: config.token,
        withdraw_delay: config.withdraw_delay,
        salt: config.salt,
    }
}

/// The claim gate as the watchers and sweeps read it (ADR 0074 decision 5):
/// every batch-settlement channel it has accepted a voucher on, with that
/// channel's latest voucher. The gate stays the one place a voucher is
/// accepted; this only reads what it holds.
pub(crate) struct ClaimGateVouchers(pub(crate) Arc<ClientClaimGate>);

impl HeldVouchers for ClaimGateVouchers {
    fn held_vouchers(&self) -> Vec<HeldVoucher> {
        self.0
            .batch_channels()
            .into_iter()
            .filter_map(|channel| {
                let (amount, signature) = self.0.latest_inbound_claim(&channel.channel_key())?;
                Some(held_voucher(channel, amount, signature))
            })
            .collect()
    }

    /// An XRPL channel id binds no network, so the gate's record of the
    /// network the channel's voucher named answers where the presentation
    /// cannot (ADR 0087 decision 2).
    fn network_of(&self, channel: &ChannelId) -> Option<String> {
        self.0
            .batch_channels()
            .into_iter()
            .find_map(|held| match held {
                JournaledBatchChannel::Xrpl {
                    channel_id,
                    network,
                } if xrpl_channel(&channel_id) == *channel => Some(network.caip2().to_string()),
                _ => None,
            })
    }
}

/// [`ClaimGateVouchers`] narrowed to the channels on one network (ADR
/// 0086): what one network's watcher lands, so a backend is never handed a
/// channel on another network. Which network a channel is on is proved from
/// its presentation against `networks`, every network this node settles on.
pub(crate) struct NetworkVouchers {
    pub(crate) held: ClaimGateVouchers,
    pub(crate) network: Network,
    pub(crate) networks: Vec<Network>,
}

impl HeldVouchers for NetworkVouchers {
    fn held_vouchers(&self) -> Vec<HeldVoucher> {
        self.held
            .held_vouchers()
            .into_iter()
            .filter(|held| {
                // The gate's record first: an XRPL id says no network.
                let recorded = self
                    .held
                    .network_of(held.presentation.channel())
                    .and_then(|network| network.parse::<Network>().ok());
                recorded
                    .or_else(|| network_of_presentation(&held.presentation, &self.networks))
                    .as_ref()
                    == Some(&self.network)
            })
            .collect()
    }
}

/// The claim gate as the connector asks which network a payment's channel
/// is on (ADR 0086 decision 14): the network whose domain reproduces the
/// channel the gate journaled, among every network this node settles on.
pub(crate) struct GateChannelNetworks {
    pub(crate) gate: Arc<ClientClaimGate>,
    pub(crate) networks: Vec<Network>,
}

impl connector_runtime::ChannelNetworks for GateChannelNetworks {
    fn network_of(&self, channel_key: &str) -> Option<Network> {
        let channel = self
            .gate
            .batch_channels()
            .into_iter()
            .find(|channel| channel.channel_key() == channel_key)?;
        match channel {
            // Recorded with the channel: its id says none (ADR 0087).
            JournaledBatchChannel::Xrpl { network, .. } => Some(Network::Xrpl(network)),
            channel => network_of_presentation(&presentation(channel), &self.networks),
        }
    }
}

/// A journaled channel and its latest voucher, as the port names them.
fn held_voucher(channel: JournaledBatchChannel, amount: u128, signature: Vec<u8>) -> HeldVoucher {
    HeldVoucher {
        presentation: presentation(channel),
        voucher: Voucher {
            cumulative_amount: amount,
            signature,
        },
    }
}

/// A journaled channel as it is presented to the port.
fn presentation(channel: JournaledBatchChannel) -> ChannelPresentation {
    match channel {
        JournaledBatchChannel::Evm { channel_id, config } => ChannelPresentation::Evm {
            channel: evm_channel(&channel_id),
            config: port_config(&config),
        },
        JournaledBatchChannel::Solana { channel_account } => ChannelPresentation::Solana {
            channel: solana_channel(&channel_account),
        },
        JournaledBatchChannel::Xrpl { channel_id, .. } => ChannelPresentation::Xrpl {
            channel: xrpl_channel(&channel_id),
        },
    }
}

/// Restore every batch-settlement channel `channels` names to its chain's
/// backend, so the port's `channel_state` and `land` work on it from the
/// moment this node serves -- the journal is the only place an EVM
/// channel's config survives a restart (ADR 0074 decision 2).
///
/// **Restored, not re-admitted.** These channels hold vouchers this node
/// already accepted, and landing them is what protects that value (ADR 0074
/// decision 5). Re-running admission here would strand them whenever a rule
/// had tightened since -- a raised minimum delay -- while the payer walks
/// off with what it paid. So only what landing needs is checked: the
/// channel still exists on chain under the id its presentation derives. The
/// rules still gate **new** vouchers: the claim gate admits each channel
/// afresh, under this node's current terms, before it accepts one.
///
/// Best effort, channel by channel, and never a refusal to start: a channel
/// that cannot be restored now is logged and left, unadmitted. Whether
/// anything tries again is the backend's to say (XRPL's: ADR 0087, the
/// #1641 update). A channel whose chain has since been opted
/// out of is logged too: its vouchers can no longer be accepted here, and
/// whatever it still holds unlanded is the operator's to collect.
///
/// An EVM channel goes to the network whose domain reproduces its id (ADR
/// 0086): its journal key names no network, and its id binds one.
pub(crate) async fn restore_journaled_channels(
    channels: &[JournaledBatchChannel],
    backends: &[(Network, Arc<dyn BatchSettlementBackend>)],
) {
    for channel in channels {
        let presentation = presentation(*channel);
        let network = match channel {
            // The network the voucher named, journaled with the channel.
            JournaledBatchChannel::Xrpl { network, .. } => Some(Network::Xrpl(*network)),
            _ => {
                network_of_presentation(&presentation, backends.iter().map(|(network, _)| network))
            }
        };
        let backend = backends
            .iter()
            .find(|(held, _)| Some(held) == network.as_ref())
            .map(|(_, backend)| backend);
        let channel = presentation.channel().clone();
        let Some(backend) = backend else {
            tracing::warn!(
                %channel,
                chain = presentation.chain(),
                "the client-edge journal holds vouchers on a batch-settlement channel on a \
                 network this node no longer opts in to; they will not be landed from here"
            );
            continue;
        };
        if let Err(error) = backend.restore(presentation).await {
            tracing::warn!(
                %channel,
                %error,
                "could not restore a batch-settlement channel the client-edge journal records; \
                 it is not admitted in this process, and only a backend that watches it \
                 will try again"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use connector_settlement::batch::{AdmissionRefusal, BatchChannelStatus};

    fn state(status: BatchChannelStatus, landed: u128, collateral: u128) -> BatchChannelState {
        BatchChannelState {
            id: ChannelId("c".to_string()),
            status,
            voucher_signer: VoucherSigner::Solana([7; 32]),
            landed,
            collateral,
        }
    }

    #[test]
    fn a_channel_that_accepts_vouchers_is_found_at_its_ceiling() {
        for status in [BatchChannelStatus::Open, BatchChannelStatus::Withdrawing] {
            let found = resolution(Ok(state(status, 300, 700)))
                .expect("found")
                .expect("some");
            assert_eq!(max_cumulative_evm(&found), 1_000);
            assert_eq!(max_cumulative_solana(&found), 1_000);
        }
    }

    /// ADR 0074 decision 3, amended 2026-09-30 (#1429): EVM's ceiling is
    /// `u128` and carries a value above `u64::MAX` through exactly.
    #[test]
    fn an_evm_ceiling_above_u64_is_not_saturated() {
        let above = u128::from(u64::MAX) + 1_000;
        let wide = state(BatchChannelStatus::Open, above, 0);
        assert_eq!(max_cumulative_evm(&wide), above);
    }

    #[test]
    fn a_solana_ceiling_wider_than_u64_saturates_rather_than_wraps() {
        let wide = state(BatchChannelStatus::Open, u128::from(u64::MAX), 5);
        assert_eq!(max_cumulative_solana(&wide), u64::MAX);
    }

    /// ADR 0074 decision 5: no voucher is accepted once a channel is
    /// closing, and a sealed one takes nothing at all.
    #[test]
    fn a_channel_that_accepts_no_voucher_is_terminal_not_unknown() {
        for status in [BatchChannelStatus::Closing, BatchChannelStatus::Sealed] {
            assert!(matches!(
                resolution(Ok(state(status, 300, 0))),
                Err(ChannelResolutionError::Terminal(_))
            ));
        }
    }

    #[test]
    fn a_channel_that_is_not_there_or_not_ours_is_unknown() {
        let channel = ChannelId("c".to_string());
        for error in [
            BatchSettlementError::ChannelNotFound(channel.clone()),
            BatchSettlementError::NotAdmissible {
                channel: channel.clone(),
                refusal: AdmissionRefusal::TokenNotSettled,
            },
            BatchSettlementError::ChannelIdMismatch {
                presented: channel.clone(),
                derived: channel.clone(),
            },
        ] {
            assert_eq!(resolution(Err(error)), Ok(None));
        }
    }

    /// An unreachable chain is never read as "no such channel": the gate
    /// refuses the voucher as a failed lookup, which a client retries.
    #[test]
    fn a_chain_that_could_not_be_read_is_a_failed_lookup() {
        assert!(matches!(
            resolution(Err(BatchSettlementError::Backend("timed out".to_string()))),
            Err(ChannelResolutionError::LookupFailed(_))
        ));
    }

    /// A watcher lands what the gate journaled: an EVM channel with the
    /// config `claim` needs, a Solana one by its account, each carrying its
    /// latest voucher's amount and signature unchanged.
    #[test]
    fn a_journaled_channel_is_held_under_the_ports_presentation() {
        let config = BatchChannelConfig {
            payer: [1; 20],
            payer_authorizer: [2; 20],
            receiver: [3; 20],
            receiver_authorizer: [4; 20],
            token: [5; 20],
            withdraw_delay: 86_400,
            salt: [6; 32],
        };
        let evm = held_voucher(
            JournaledBatchChannel::Evm {
                channel_id: [0xab; 32],
                config,
            },
            300,
            vec![9; 65],
        );
        assert_eq!(
            evm.presentation,
            ChannelPresentation::Evm {
                channel: evm_channel(&[0xab; 32]),
                config: port_config(&config),
            }
        );
        assert_eq!(evm.voucher.cumulative_amount, 300);
        assert_eq!(evm.voucher.signature, vec![9; 65]);

        let solana = held_voucher(
            JournaledBatchChannel::Solana {
                channel_account: [0x01; 32],
            },
            7,
            vec![8; 64],
        );
        assert_eq!(
            solana.presentation,
            ChannelPresentation::Solana {
                channel: solana_channel(&[0x01; 32]),
            }
        );
        assert_eq!(solana.voucher.cumulative_amount, 7);
    }

    #[test]
    fn channel_ids_are_spelled_as_the_port_spells_them() {
        assert_eq!(evm_channel(&[0xab; 32]).0, format!("0x{}", "ab".repeat(32)));
        assert_eq!(
            solana_channel(&[0x01; 32]).0,
            solana_sdk::pubkey::Pubkey::new_from_array([0x01; 32]).to_string()
        );
    }
}
