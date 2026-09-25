//! Where a node's x402 `batch-settlement` backends meet its client edge
//! (ADR 0074, epic #1349): the adapter from the receive-only settlement port
//! ([`BatchSettlementBackend`], #1340) to the claim gate's seam
//! ([`BatchSettlementChannels`], #1341), and the boot step that re-admits
//! every channel the client edge's journal holds vouchers on.
//!
//! Here rather than in either crate it joins, for the reason
//! `SettlementChannelSource` is: `connector-client-edge` does not depend on
//! the settlement crates and should not, and ADR 0001 puts construction in
//! `connector-cli`.
//!
//! **Thin on purpose.** Admission, collateral and the chain reads are the
//! port's; freshness, the signature and the journal are the gate's. This
//! translates one vocabulary into the other and decides nothing but how a
//! port error reads to the gate.

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use connector_client_edge::{
    AdmittedEvmVoucherChannel, AdmittedSolanaVoucherChannel, BatchSettlementChannels,
    ChannelLookupFailed, ChannelResolutionError, ChannelTerminal, JournaledBatchChannel,
};
use connector_settlement::batch::{
    BatchChannelState, BatchSettlementBackend, BatchSettlementError, ChannelPresentation,
    EvmChannelConfig, VoucherSigner,
};
use connector_settlement::ChannelId;
use connector_signer::{BatchChannelConfig, BatchSettlementDomain};

/// This node's batch-settlement backends, one per chain it has opted in on
/// (`[settlement.<chain>.batch_settlement]`), as the claim gate asks them.
/// A chain whose table is absent is `None` here, and the gate refuses its
/// vouchers by name.
pub(crate) struct BatchSettlementChannelsAdapter {
    evm: Option<EvmLeg>,
    solana: Option<Arc<dyn BatchSettlementBackend>>,
}

struct EvmLeg {
    backend: Arc<dyn BatchSettlementBackend>,
    /// The domain EVM vouchers are verified under: this chain and the
    /// configured `x402BatchSettlement`, both from the backend that proved
    /// them against the chain at connect.
    domain: BatchSettlementDomain,
}

impl BatchSettlementChannelsAdapter {
    /// `None` when neither chain has opted in: a gate given nothing refuses
    /// every voucher by name, which is what "off unless configured" means.
    pub(crate) fn new(
        evm: Option<(Arc<dyn BatchSettlementBackend>, BatchSettlementDomain)>,
        solana: Option<Arc<dyn BatchSettlementBackend>>,
    ) -> Option<BatchSettlementChannelsAdapter> {
        if evm.is_none() && solana.is_none() {
            return None;
        }
        Some(BatchSettlementChannelsAdapter {
            evm: evm.map(|(backend, domain)| EvmLeg { backend, domain }),
            solana,
        })
    }
}

impl fmt::Debug for BatchSettlementChannelsAdapter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BatchSettlementChannelsAdapter")
            .field("evm", &self.evm.as_ref().map(|leg| leg.domain))
            .field("solana", &self.solana.is_some())
            .finish()
    }
}

#[async_trait]
impl BatchSettlementChannels for BatchSettlementChannelsAdapter {
    fn evm_domain(&self) -> Option<BatchSettlementDomain> {
        self.evm.as_ref().map(|leg| leg.domain)
    }

    fn accepts_solana(&self) -> bool {
        self.solana.is_some()
    }

    /// An EVM channel is found only from its config: the contract stores a
    /// hash. With none -- a voucher that carried none, on a channel the gate
    /// has no record of -- there is nothing to look up.
    async fn evm(
        &self,
        channel_id: &[u8; 32],
        presented_config: Option<&BatchChannelConfig>,
    ) -> Result<Option<AdmittedEvmVoucherChannel>, ChannelResolutionError> {
        let (Some(leg), Some(config)) = (&self.evm, presented_config) else {
            return Ok(None);
        };
        let presentation = ChannelPresentation::Evm {
            channel: evm_channel(channel_id),
            config: port_config(config),
        };
        let state = state_of(leg.backend.as_ref(), presentation).await;
        Ok(resolution(state)?.map(|state| AdmittedEvmVoucherChannel {
            config: *config,
            max_cumulative: max_cumulative(&state),
        }))
    }

    async fn solana(
        &self,
        channel_account: &[u8; 32],
    ) -> Result<Option<AdmittedSolanaVoucherChannel>, ChannelResolutionError> {
        let Some(backend) = &self.solana else {
            return Ok(None);
        };
        let presentation = ChannelPresentation::Solana {
            channel: solana_channel(channel_account),
        };
        let state = state_of(backend.as_ref(), presentation).await;
        Ok(
            resolution(state)?.and_then(|state| match state.voucher_signer {
                VoucherSigner::Solana(authorized_signer) => Some(AdmittedSolanaVoucherChannel {
                    authorized_signer,
                    max_cumulative: max_cumulative(&state),
                }),
                // A Solana backend reporting an EVM signer is not a channel
                // this gate can verify a voucher against.
                VoucherSigner::Evm(_) => None,
            }),
        )
    }
}

/// The channel's state now: one read for a channel the backend has already
/// admitted, and admission -- which reads it too -- for one it has not.
async fn state_of(
    backend: &dyn BatchSettlementBackend,
    presentation: ChannelPresentation,
) -> Result<BatchChannelState, BatchSettlementError> {
    match backend.channel_state(presentation.channel()).await {
        Err(BatchSettlementError::ChannelNotAdmitted(_)) => backend.admit(presentation).await,
        read => read,
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

/// The port's voucher ceiling, in the gate's `u64` amounts. A ceiling wider
/// than any `u64` bounds nothing a voucher here can name, so it saturates.
fn max_cumulative(state: &BatchChannelState) -> u64 {
    u64::try_from(state.voucher_ceiling()).unwrap_or(u64::MAX)
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

/// Re-admit every batch-settlement channel `channels` names to its chain's
/// backend, so the port's `channel_state` and `land` work on it from the
/// moment this node serves -- the journal is the only place an EVM
/// channel's config survives a restart (ADR 0074 decision 2).
///
/// Best effort, channel by channel, and never a refusal to start: a channel
/// that cannot be re-admitted now is logged and left, and the gate still
/// admits it again from the same record on its next voucher. A channel whose
/// chain has since been opted out of is logged too: its vouchers can no
/// longer be accepted here, and whatever it still holds unlanded is the
/// operator's to collect.
pub(crate) async fn readmit_journaled_channels(
    channels: &[JournaledBatchChannel],
    evm: Option<&dyn BatchSettlementBackend>,
    solana: Option<&dyn BatchSettlementBackend>,
) {
    for channel in channels {
        let (backend, presentation) = match *channel {
            JournaledBatchChannel::Evm { channel_id, config } => (
                evm,
                ChannelPresentation::Evm {
                    channel: evm_channel(&channel_id),
                    config: port_config(&config),
                },
            ),
            JournaledBatchChannel::Solana { channel_account } => (
                solana,
                ChannelPresentation::Solana {
                    channel: solana_channel(&channel_account),
                },
            ),
        };
        let channel = presentation.channel().clone();
        let Some(backend) = backend else {
            tracing::warn!(
                %channel,
                chain = presentation.chain(),
                "the client-edge journal holds vouchers on a batch-settlement channel on a chain \
                 this node no longer opts in to; they will not be landed from here"
            );
            continue;
        };
        if let Err(error) = backend.admit(presentation).await {
            tracing::warn!(
                %channel,
                %error,
                "could not re-admit a batch-settlement channel the client-edge journal holds \
                 vouchers on; its next voucher admits it again"
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
            assert_eq!(max_cumulative(&found), 1_000);
        }
    }

    #[test]
    fn a_ceiling_wider_than_u64_saturates_rather_than_wraps() {
        let wide = state(BatchChannelStatus::Open, u128::from(u64::MAX), 5);
        assert_eq!(max_cumulative(&wide), u64::MAX);
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

    #[test]
    fn channel_ids_are_spelled_as_the_port_spells_them() {
        assert_eq!(evm_channel(&[0xab; 32]).0, format!("0x{}", "ab".repeat(32)));
        assert_eq!(
            solana_channel(&[0x01; 32]).0,
            solana_sdk::pubkey::Pubkey::new_from_array([0x01; 32]).to_string()
        );
    }
}
