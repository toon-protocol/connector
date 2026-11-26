//! How the client edge's claim gate finds a **voucher**'s channel (ADR 0074,
//! issue #1341; spec #1684, issue #1714): through the node's settlement
//! networks, one collection keyed by CAIP-2 network
//! ([`connector_runtime::SettlementNetworks`]). The gate owns freshness,
//! signature verification and the journal; each network's batch settlement
//! port owns admission and collateral.
//!
//! # The seam
//!
//! **Whether a node accepts vouchers at all is whether its gate was given a
//! settlement network** ([`crate::ClientClaimGate::with_settlement_networks`]).
//! A gate settling on none refuses every voucher by name
//! ([`crate::ClaimIngestRejection::BatchSettlementNotAccepted`]) -- ADR 0074
//! decision 1's "off unless configured". A voucher's own `network` picks the
//! one network asked about its channel (ADR 0086), and nothing here matches
//! on the chain to find it: [`VoucherChannels`] looks the network up, presents
//! the channel to its receiving half in the port's spelling and reads back
//! what the gate verifies against.
//!
//! # What the journal remembers
//!
//! A voucher signs only its channel's id, and on EVM that id is a hash of a
//! `ChannelConfig` the contract never gives back -- so a node that forgot the
//! config could never `claim` a voucher it had already accepted. The gate
//! therefore journals each channel it accepts a first voucher on as a
//! [`JournalEntry::BatchChannelAdmitted`], in the same batch as the voucher,
//! and [`journaled_batch_channels`] reads them back: the gate, to present a
//! known channel's config when a later voucher carries none, and the runtime,
//! to restore every such channel to its network at boot.
//!
//! # What a network admits
//!
//! Only channels its port would **admit** (ADR 0074 decisions 2 and 5): on
//! EVM, `receiver` and `receiverAuthorizer` both this connector's settlement
//! address, a token it settles in, a `withdrawDelay` at or above its
//! published minimum, and a nonzero `payerAuthorizer`; on Solana, a channel
//! account whose PDA re-derives from its own seeds, Open, with `payee` and
//! `rent_payer` this connector's sponsor key, a mint it settles in, a
//! one-recipient `distribution_hash` and a `grace_period` at or above its
//! minimum. Anything else is an unknown channel.
//!
//! The gate takes the signer from what the port reads and nothing else: the
//! verified `ChannelConfig` on EVM, `authorized_signer` on Solana (ADR 0074
//! decision 4), the `PublicKey` on XRPL. And it takes the collateral bound
//! without caching it, because on EVM that figure can fall (decision 5):
//! keeping it current -- dropping a cache on `WithdrawInitiated` -- is the
//! port's job.

use std::collections::HashSet;
use std::sync::{Mutex, MutexGuard};

use connector_domain::client_claim::{
    canonical_channel_key, EVM_NAMESPACE, SOLANA_NAMESPACE, XRPL_CHANNEL_NAMESPACE,
};
use connector_domain::{AssetChain, JournalEntry, Network, XrplNetwork};
use connector_runtime::{JournalError, SettlementNetwork, SettlementNetworks};
use connector_settlement::batch::{
    BatchChannelState, BatchSettlementBackend, BatchSettlementError, ChannelPresentation,
    EvmChannelConfig,
};
use connector_settlement::ChannelId;
use connector_signer::BatchChannelConfig;

use crate::channels::{ChannelLookupFailed, ChannelResolutionError, ChannelTerminal};

/// The claim gate's view of the node's settlement networks: finds a
/// voucher's channel on the network the voucher names, and on no other.
///
/// Remembers every channel it has seen admitted in this process. A network's
/// port also answers for channels it only **restored** -- ones the journal
/// holds vouchers on, brought back at boot without judging them (ADR 0074
/// decision 5) -- so "the port can read it" is not "a new voucher may be
/// accepted on it". A channel not yet admitted here is admitted, under this
/// node's current rules, before its first new voucher.
pub(crate) struct VoucherChannels {
    networks: SettlementNetworks,
    admitted: Mutex<HashSet<ChannelId>>,
}

impl VoucherChannels {
    pub(crate) fn new(networks: SettlementNetworks) -> VoucherChannels {
        VoucherChannels {
            networks,
            admitted: Mutex::new(HashSet::new()),
        }
    }

    /// Whether this node settles on `network` (ADR 0086 decision 1).
    pub(crate) fn settles_on(&self, network: &Network) -> bool {
        self.networks.settles_on(network)
    }

    /// Whether this node settles on any network of `chain`: what decides
    /// between refusing a voucher as "not accepted here" and as "on a network
    /// not settled".
    pub(crate) fn settles_on_chain(&self, chain: AssetChain) -> bool {
        self.networks
            .iter()
            .any(|held| held.network().chain() == chain)
    }

    /// The channel `presentation` names on `network`, read through that
    /// network's receiving half, if the network admits it. `Ok(None)` for a
    /// network this node does not settle on, and for a channel it does not
    /// admit.
    pub(crate) async fn find(
        &self,
        network: &Network,
        presentation: ChannelPresentation,
    ) -> Result<Option<BatchChannelState>, ChannelResolutionError> {
        let Some(held) = self.networks.get(network) else {
            return Ok(None);
        };
        resolution(self.state_of(held.as_ref(), presentation).await)
    }

    /// The channel's state now, for a voucher the gate is about to accept:
    /// one read for a channel admitted in this process, and admission --
    /// which reads it too -- for one that was not. A channel the port only
    /// restored is admitted here like any other, so a rule tightened since
    /// its vouchers were accepted refuses its new ones.
    async fn state_of(
        &self,
        network: &dyn SettlementNetwork,
        presentation: ChannelPresentation,
    ) -> Result<BatchChannelState, BatchSettlementError> {
        let receiver: std::sync::Arc<dyn BatchSettlementBackend> = network.receiver();
        let channel = presentation.channel().clone();
        if self.admitted().contains(&channel) {
            match receiver.channel_state(&channel).await {
                Err(BatchSettlementError::ChannelNotAdmitted(_)) => {}
                read => return read,
            }
        }
        let state = receiver.admit(presentation).await?;
        self.admitted().insert(channel);
        Ok(state)
    }

    fn admitted(&self) -> MutexGuard<'_, HashSet<ChannelId>> {
        self.admitted
            .lock()
            .expect("voucher channels lock poisoned")
    }
}

impl std::fmt::Debug for VoucherChannels {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("VoucherChannels")
            .field(&self.networks)
            .finish()
    }
}

/// How a port answer reads to the gate.
///
/// * A channel that accepts vouchers is found.
/// * One that no longer accepts them -- Solana `Closing` or `Sealed` -- is
///   **terminal**: it exists, and no new voucher may be accepted on it (ADR
///   0074 decisions 3 and 5).
/// * One that does not exist, or that this node does not admit, is not
///   found: the gate refuses it as an unknown channel. The admission refusal
///   is logged, since an operator whose clients keep opening channels this
///   node will not take wants to know why.
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

/// An XRPL channel id as the port spells it: 64 lowercase hex, no prefix.
pub(crate) fn xrpl_channel(channel_id: &[u8; 32]) -> ChannelId {
    ChannelId(hex::encode(channel_id))
}

/// An EVM channel id as the port spells it: `0x` and 64 lowercase hex.
pub(crate) fn evm_channel(channel_id: &[u8; 32]) -> ChannelId {
    ChannelId(format!("0x{}", hex::encode(channel_id)))
}

/// A Solana channel account as the port spells it: base58.
pub(crate) fn solana_channel(channel_account: &[u8; 32]) -> ChannelId {
    ChannelId(bs58::encode(channel_account).into_string())
}

/// The gate's config as the port carries it: the same seven fields.
pub(crate) fn port_config(config: &BatchChannelConfig) -> EvmChannelConfig {
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

/// A batch-settlement channel the client edge has accepted a voucher on, as
/// its journal records it: enough to restore the channel to its backend
/// (ADR 0074 decision 2) without the client presenting anything again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JournaledBatchChannel {
    /// The channel's id and the `ChannelConfig` it is the hash of -- the
    /// config the gate verified hashes to it before the channel's first
    /// voucher was accepted.
    Evm {
        channel_id: [u8; 32],
        config: BatchChannelConfig,
    },
    /// The channel account. Every other field is on chain.
    Solana { channel_account: [u8; 32] },
    /// The channel's id, and the network its voucher named. The id binds no
    /// network (ADR 0087 decision 2), so a restart that did not remember
    /// which ledger the channel lives on could not read it from a node with
    /// several XRPL tables.
    Xrpl {
        channel_id: [u8; 32],
        network: XrplNetwork,
    },
}

/// Bytes in an EVM presentation: five addresses, the `withdrawDelay` as a
/// big-endian `u64`, and the salt.
const EVM_PRESENTATION_LEN: usize = 5 * 20 + 8 + 32;

impl JournaledBatchChannel {
    /// The canonical key this channel's watermark is filed under -- the key
    /// a voucher on it produces (`ClientClaim::channel_key`).
    pub fn channel_key(&self) -> String {
        match self {
            JournaledBatchChannel::Evm { channel_id, .. } => {
                format!("{EVM_NAMESPACE}:0x{}", hex::encode(channel_id))
            }
            JournaledBatchChannel::Solana { channel_account } => format!(
                "{SOLANA_NAMESPACE}:{}",
                bs58::encode(channel_account).into_string()
            ),
            JournaledBatchChannel::Xrpl { channel_id, .. } => {
                format!("{XRPL_CHANNEL_NAMESPACE}:{}", hex::encode(channel_id))
            }
        }
    }

    /// The journal entry that records this channel.
    pub(crate) fn to_entry(self) -> JournalEntry {
        let presentation = match &self {
            JournaledBatchChannel::Evm { config, .. } => {
                let mut bytes = Vec::with_capacity(EVM_PRESENTATION_LEN);
                for address in [
                    config.payer,
                    config.payer_authorizer,
                    config.receiver,
                    config.receiver_authorizer,
                    config.token,
                ] {
                    bytes.extend_from_slice(&address);
                }
                bytes.extend_from_slice(&config.withdraw_delay.to_be_bytes());
                bytes.extend_from_slice(&config.salt);
                bytes
            }
            JournaledBatchChannel::Solana { .. } => Vec::new(),
            // The CAIP-2 network, UTF-8: all the ledger cannot give back.
            JournaledBatchChannel::Xrpl { network, .. } => network.caip2().as_bytes().to_vec(),
        };
        JournalEntry::BatchChannelAdmitted {
            channel_id: self.channel_key(),
            presentation,
        }
    }

    /// [`Self::to_entry`]'s inverse; `None` for an entry no build of this
    /// gate writes.
    fn from_entry(channel_id: &str, presentation: &[u8]) -> Option<JournaledBatchChannel> {
        match channel_id.split_once(':')? {
            (EVM_NAMESPACE, id) => {
                let id = hex::decode(id.strip_prefix("0x")?).ok()?;
                if presentation.len() != EVM_PRESENTATION_LEN {
                    return None;
                }
                let address = |index: usize| -> [u8; 20] {
                    presentation[index * 20..(index + 1) * 20]
                        .try_into()
                        .expect("a 20-byte slice")
                };
                let delay: [u8; 8] = presentation[100..108].try_into().expect("8 bytes");
                Some(JournaledBatchChannel::Evm {
                    channel_id: id.try_into().ok()?,
                    config: BatchChannelConfig {
                        payer: address(0),
                        payer_authorizer: address(1),
                        receiver: address(2),
                        receiver_authorizer: address(3),
                        token: address(4),
                        withdraw_delay: u64::from_be_bytes(delay),
                        salt: presentation[108..].try_into().expect("32 bytes"),
                    },
                })
            }
            (SOLANA_NAMESPACE, account) if presentation.is_empty() => {
                let account = bs58::decode(account).into_vec().ok()?;
                Some(JournaledBatchChannel::Solana {
                    channel_account: account.try_into().ok()?,
                })
            }
            (XRPL_CHANNEL_NAMESPACE, id) => {
                let network = std::str::from_utf8(presentation)
                    .ok()?
                    .parse::<Network>()
                    .ok()?;
                let Network::Xrpl(network) = network else {
                    return None;
                };
                Some(JournaledBatchChannel::Xrpl {
                    channel_id: hex::decode(id).ok()?.try_into().ok()?,
                    network,
                })
            }
            _ => None,
        }
    }
}

/// Every batch-settlement channel `entries` records, once each, in the order
/// first recorded. A later record of the same channel -- written again after
/// a rollback or a failed batch left its watermark empty -- is the same
/// channel and is not repeated.
///
/// # Errors
///
/// [`JournalError::Corrupt`] for a record no build of the gate writes: the
/// journal is this node's money state, and a channel it cannot read back is
/// one whose accepted vouchers it could never land, so the node refuses to
/// start rather than drop it (ADR 0009).
pub fn journaled_batch_channels(
    entries: &[JournalEntry],
) -> Result<Vec<JournaledBatchChannel>, JournalError> {
    let mut seen = std::collections::HashSet::new();
    let mut channels = Vec::new();
    for entry in entries {
        let JournalEntry::BatchChannelAdmitted {
            channel_id,
            presentation,
        } = entry
        else {
            continue;
        };
        let channel =
            JournaledBatchChannel::from_entry(channel_id, presentation).ok_or_else(|| {
                JournalError::Corrupt(format!(
                    "unreadable batch-settlement channel '{channel_id}'"
                ))
            })?;
        if seen.insert(channel.channel_key()) {
            channels.push(channel);
        }
    }
    Ok(channels)
}

/// The channel of the first `toon-channel` entry in a claim journal, or
/// `None` when it holds only vouchers (ADR 0075 decision 8, issue #1385).
///
/// A journal an older build wrote may hold claims on TOON's own channels:
/// the peer book's (`peer-claims.log`), and a pre-ADR 0075 client's in the
/// client edge's book. This build can neither judge, land nor drain one, so
/// the runtime refuses such a journal at boot, by name, rather than skip
/// entries somebody could still redeem.
///
/// A `toon-channel` entry is one only TOON's channels ever produced -- an
/// `OutboundClaimSigned` or an `InboundFulfillmentRecorded` -- or a claim
/// entry (`InboundClaimAccepted`, `InboundClaimRolledBack`,
/// `InboundClaimWatermarkReset`) on a channel the journal never admitted as
/// an x402 channel: the gate journals a voucher channel's
/// `BatchChannelAdmitted` with, and immediately before, its first accepted
/// voucher, so a claim on a channel with none is not a voucher's.
pub fn first_toon_channel_entry(entries: &[JournalEntry]) -> Option<String> {
    let admitted: std::collections::HashSet<String> = entries
        .iter()
        .filter_map(|entry| match entry {
            JournalEntry::BatchChannelAdmitted { channel_id, .. } => {
                Some(canonical_channel_key(channel_id))
            }
            // An allowance's watermark (ADR 0080) rides the same entry kinds
            // under `zk-note:<id>`; its grant is what makes that key an
            // allowance's rather than a retired channel's.
            JournalEntry::AllowanceGranted { allowance_id, .. } => Some(format!(
                "{}:{allowance_id}",
                connector_domain::client_claim::ZK_NOTE_NAMESPACE
            )),
            _ => None,
        })
        .collect();
    entries.iter().find_map(|entry| match entry {
        JournalEntry::OutboundClaimSigned { channel_id, .. }
        | JournalEntry::InboundFulfillmentRecorded { channel_id, .. } => Some(channel_id.clone()),
        JournalEntry::InboundClaimAccepted { channel_id, .. }
        | JournalEntry::InboundClaimRolledBack { channel_id, .. }
        | JournalEntry::InboundClaimWatermarkReset { channel_id }
            if !admitted.contains(&canonical_channel_key(channel_id)) =>
        {
            Some(channel_id.clone())
        }
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use connector_settlement::batch::{AdmissionRefusal, BatchChannelStatus, VoucherSigner};

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
            assert_eq!(found.voucher_ceiling(), 1_000);
        }
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
        assert_eq!(xrpl_channel(&[0xab; 32]).0, "ab".repeat(32));
        assert_eq!(
            solana_channel(&[0x01; 32]).0,
            "4vJ9JU1bJJE96FWSJKvHsmmFADCg4gpZQff4P3bkLKi"
        );
    }

    /// ADR 0075 decision 8: a journal of vouchers alone holds no
    /// `toon-channel` entry, whatever else it records about them.
    #[test]
    fn a_voucher_journal_holds_no_toon_channel_entry() {
        let channel = evm();
        let key = channel.channel_key();
        let entries = vec![
            channel.to_entry(),
            JournalEntry::InboundClaimAccepted {
                channel_id: key.clone(),
                nonce: connector_domain::VOUCHER_WATERMARK_NONCE,
                cumulative_amount: 10,
                signature: vec![1; 65],
            },
            JournalEntry::InboundClaimWatermarkReset { channel_id: key },
            JournalEntry::OutboundVoucherSigned {
                channel_id: "evm:0x01".to_string(),
                cumulative_amount: 5,
            },
        ];
        assert_eq!(first_toon_channel_entry(&entries), None);
        assert_eq!(first_toon_channel_entry(&[]), None);
    }

    /// A claim on a channel the journal never admitted as an x402 channel is
    /// a `toon-channel` claim, and so is every entry kind only TOON's
    /// channels produced. Each is named by its channel.
    #[test]
    fn a_toon_channel_entry_is_found_and_named() {
        let voucher = evm();
        let toon = format!("0x{}", "ab".repeat(32));
        for entry in [
            JournalEntry::InboundClaimAccepted {
                channel_id: toon.clone(),
                nonce: 3,
                cumulative_amount: 90,
                signature: vec![0; 65],
            },
            JournalEntry::InboundClaimWatermarkReset {
                channel_id: toon.clone(),
            },
            JournalEntry::InboundClaimRolledBack {
                channel_id: toon.clone(),
                nonce: 2,
                cumulative_amount: 40,
            },
            JournalEntry::OutboundClaimSigned {
                peer_id: "peer-b".to_string(),
                channel_id: toon.clone(),
                nonce: 2,
                cumulative_amount: 150,
            },
            JournalEntry::InboundFulfillmentRecorded {
                channel_id: toon.clone(),
                amount: 7,
            },
        ] {
            let entries = vec![voucher.to_entry(), entry.clone()];
            assert_eq!(
                first_toon_channel_entry(&entries),
                Some(toon.clone()),
                "{entry:?}"
            );
        }
    }

    fn evm() -> JournaledBatchChannel {
        JournaledBatchChannel::Evm {
            channel_id: [0xab; 32],
            config: BatchChannelConfig {
                payer: [1; 20],
                payer_authorizer: [2; 20],
                receiver: [3; 20],
                receiver_authorizer: [4; 20],
                token: [5; 20],
                withdraw_delay: 86_400,
                salt: [6; 32],
            },
        }
    }

    fn solana() -> JournaledBatchChannel {
        JournaledBatchChannel::Solana {
            channel_account: [0xc3; 32],
        }
    }

    #[test]
    fn a_journaled_channel_reads_back_as_itself_on_either_chain() {
        let entries = vec![evm().to_entry(), solana().to_entry()];
        assert_eq!(
            journaled_batch_channels(&entries).unwrap(),
            vec![evm(), solana()]
        );
    }

    /// The key is the one a voucher on the channel is filed under, so the
    /// record and the watermark always name the same channel.
    #[test]
    fn a_journaled_channel_is_keyed_as_its_vouchers_are() {
        assert_eq!(evm().channel_key(), format!("evm:0x{}", "ab".repeat(32)));
        assert_eq!(
            solana().channel_key(),
            format!("solana:{}", bs58::encode([0xc3; 32]).into_string())
        );
    }

    fn xrpl(network: XrplNetwork) -> JournaledBatchChannel {
        JournaledBatchChannel::Xrpl {
            channel_id: [0x5d; 32],
            network,
        }
    }

    /// ADR 0087 decision 2: an XRPL channel is keyed `xrpl:<channel id>`, as
    /// its vouchers are, and its journal entry carries the network its
    /// voucher named -- which the id cannot say -- so a restart reads it
    /// back from the ledger it lives on.
    #[test]
    fn an_xrpl_channel_is_keyed_as_its_vouchers_are_and_remembers_its_network() {
        let channel = xrpl(XrplNetwork::Testnet);
        assert_eq!(channel.channel_key(), format!("xrpl:{}", "5d".repeat(32)));
        let entries = vec![
            xrpl(XrplNetwork::Testnet).to_entry(),
            xrpl(XrplNetwork::Standalone).to_entry(),
            // The same id on the same network is the same channel.
            xrpl(XrplNetwork::Testnet).to_entry(),
        ];
        assert_eq!(
            journaled_batch_channels(&entries).unwrap(),
            vec![xrpl(XrplNetwork::Testnet)],
            "an id binds no network, so it is one channel however often it is recorded"
        );
        let JournalEntry::BatchChannelAdmitted { presentation, .. } =
            xrpl(XrplNetwork::Standalone).to_entry()
        else {
            panic!("a channel admission");
        };
        assert_eq!(presentation, b"xrpl:1000");
    }

    #[test]
    fn an_xrpl_record_naming_no_xrpl_network_refuses_the_replay() {
        for presentation in [&b""[..], b"eip155:1", b"xrpl:7", &[0xff, 0xfe]] {
            let entries = vec![JournalEntry::BatchChannelAdmitted {
                channel_id: format!("xrpl:{}", "5d".repeat(32)),
                presentation: presentation.to_vec(),
            }];
            assert!(
                matches!(
                    journaled_batch_channels(&entries),
                    Err(JournalError::Corrupt(_))
                ),
                "{presentation:?}"
            );
        }
    }

    #[test]
    fn a_channel_recorded_twice_is_one_channel() {
        let entries = vec![
            evm().to_entry(),
            JournalEntry::InboundClaimWatermarkReset {
                channel_id: evm().channel_key(),
            },
            evm().to_entry(),
        ];
        assert_eq!(journaled_batch_channels(&entries).unwrap(), vec![evm()]);
    }

    #[test]
    fn a_record_this_gate_never_writes_refuses_the_replay() {
        for (channel_id, presentation) in [
            (evm().channel_key(), vec![0u8; 3]),
            ("evm:not-hex".to_string(), vec![0u8; EVM_PRESENTATION_LEN]),
            (solana().channel_key(), vec![1u8]),
            ("mina:whatever".to_string(), Vec::new()),
        ] {
            let entries = vec![JournalEntry::BatchChannelAdmitted {
                channel_id,
                presentation,
            }];
            assert!(matches!(
                journaled_batch_channels(&entries),
                Err(JournalError::Corrupt(_))
            ));
        }
    }
}
