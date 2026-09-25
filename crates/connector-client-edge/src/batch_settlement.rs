//! What the client edge's claim gate needs from an x402 `batch-settlement`
//! backend to admit a **voucher** (ADR 0074, issue #1341): the seam between
//! the gate, which owns freshness, signature verification and the journal,
//! and the receive-only settlement port (#1340) whose EVM and Solana
//! implementations (#1342, #1343) own admission and collateral.
//!
//! # The seam
//!
//! **Whether a node accepts vouchers at all is whether its gate was given one
//! of these** ([`crate::ClientClaimGate::with_batch_settlement`]). A gate
//! without one refuses every voucher by name
//! ([`crate::ClaimIngestRejection::BatchSettlementNotAccepted`]) -- ADR 0074
//! decision 1's "off unless configured". `connector-cli`'s runtime passes
//! one exactly when a `[settlement.<chain>.batch_settlement]` table is
//! written, adapting that chain's receive-only settlement port to this
//! trait.
//!
//! # What the journal remembers
//!
//! A voucher signs only its channel's id, and on EVM that id is a hash of a
//! `ChannelConfig` the contract never gives back -- so a node that forgot the
//! config could never `claim` a voucher it had already accepted. The gate
//! therefore journals each channel it accepts a first voucher on as a
//! [`JournalEntry::BatchChannelAdmitted`], in the same batch as the voucher,
//! and [`journaled_batch_channels`] reads them back: the gate, to hand a
//! known channel's config to [`BatchSettlementChannels::evm`] when a later
//! voucher carries none, and the runtime, to restore every such channel to
//! its backend at boot.
//!
//! # What an implementation owes
//!
//! Only channels it would **admit** (ADR 0074 decisions 2 and 5): on EVM,
//! `receiver` and `receiverAuthorizer` both this connector's settlement
//! address, a token it settles in, a `withdrawDelay` at or above its
//! published minimum, and no zero `payerAuthorizer` over a contract-wallet
//! `payer`; on Solana, a channel account whose PDA re-derives from its own
//! seeds, Open, with `payee` and `rent_payer` this connector's sponsor key,
//! a mint it settles in, a one-recipient `distribution_hash` and a
//! `grace_period` at or above its minimum. Anything else is `Ok(None)`,
//! which the gate reports as an unknown channel.
//!
//! The gate takes the signer from what this returns and nothing else: the
//! verified `ChannelConfig` on EVM, `authorized_signer` on Solana (ADR 0074
//! decision 4). And it takes `max_cumulative` as the collateral bound
//! without caching it, because on EVM that figure can fall (decision 5):
//! keeping it current -- dropping a cache on `WithdrawInitiated` -- is the
//! implementation's job.

use async_trait::async_trait;
use connector_domain::client_claim::{EVM_NAMESPACE, SOLANA_NAMESPACE};
use connector_domain::JournalEntry;
use connector_runtime::JournalError;
use connector_signer::{BatchChannelConfig, BatchSettlementDomain};

use crate::channels::ChannelResolutionError;

/// An EVM batch-settlement channel this connector admits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdmittedEvmVoucherChannel {
    /// The channel's `ChannelConfig`. The gate re-hashes it and refuses a
    /// config that does not hash to the channel the voucher names, so an
    /// implementation that got this wrong fails closed.
    pub config: BatchChannelConfig,
    /// The highest cumulative amount a voucher on this channel may name and
    /// still be claimable -- the backend's current reading of the channel's
    /// escrow, net of any pending withdrawal (ADR 0074 decision 5).
    pub max_cumulative: u64,
}

/// A Solana batch-settlement channel this connector admits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdmittedSolanaVoucherChannel {
    /// The channel account's `authorized_signer`, fixed at open.
    pub authorized_signer: [u8; 32],
    /// As [`AdmittedEvmVoucherChannel::max_cumulative`]: on Solana, the
    /// channel's `deposit`, which only `top_up` moves while it is Open.
    pub max_cumulative: u64,
}

/// A batch-settlement backend, as the claim gate sees it. See this module's
/// doc for the seam it is and what an implementation owes.
#[async_trait]
pub trait BatchSettlementChannels: Send + Sync + std::fmt::Debug {
    /// The EIP-712 domain EVM vouchers are verified under -- this node's
    /// settlement chain and `x402BatchSettlement`'s address -- or `None` if
    /// this node has not opted in to EVM vouchers.
    fn evm_domain(&self) -> Option<BatchSettlementDomain>;

    /// Whether this node has opted in to Solana vouchers.
    fn accepts_solana(&self) -> bool;

    /// The EVM channel `channel_id`, if this connector admits it.
    /// `presented_config` is the client's `ChannelConfig` when the voucher
    /// carried one, already checked by the gate to hash to `channel_id`; a
    /// channel's first voucher carries one, and an implementation that has
    /// never seen the channel and is given `None` has nothing to admit.
    async fn evm(
        &self,
        channel_id: &[u8; 32],
        presented_config: Option<&BatchChannelConfig>,
    ) -> Result<Option<AdmittedEvmVoucherChannel>, ChannelResolutionError>;

    /// The Solana channel account `channel_account`, if this connector
    /// admits it.
    async fn solana(
        &self,
        channel_account: &[u8; 32],
    ) -> Result<Option<AdmittedSolanaVoucherChannel>, ChannelResolutionError>;
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

#[cfg(test)]
mod tests {
    use super::*;

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
