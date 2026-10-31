//! The receiving half of the batch-settlement port over the ledger's own
//! `PayChannel` (ADR 0087, issue #1612); the paying half is [`crate::pay`].
//!
//! **Nothing is deployed and nothing is held.** A channel is a ledger entry
//! its id names; every answer here is read from a *validated* ledger, and
//! the only local memory is which channels have been admitted or restored,
//! because the port requires one of the two before
//! [`channel_state`](BatchSettlementBackend::channel_state) and
//! [`land`](BatchSettlementBackend::land).
//!
//! **A channel deleted when it ends is not found** (ADR 0075, "Update (issue
//! #1565)"): an XRPL `ledger_entry` read answers `entryNotFound` both for
//! one that never existed and for one that closed, so the answer is
//! [`ChannelNotFound`](BatchSettlementError::ChannelNotFound) and no copy is
//! kept.
//!
//! **Landing is judged from metadata.** A claim on an expired channel
//! answers `tesSUCCESS` and delivers nothing, so [`delivered`] reads the
//! channel's `Balance` out of the transaction's own metadata.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use connector_chain_rpc::xrpl::{TxFate, TxWait, XrplRpc, XrplRpcError};
use connector_chain_rpc::{AsRefused, RpcTransport};
use connector_settlement::batch::{
    AdmissionRefusal, BatchChannelState, BatchChannelStatus, BatchSettlementBackend,
    BatchSettlementError, ChannelPresentation, Landed, Voucher, VoucherSigner,
};
use connector_settlement::ChannelId;
use connector_signer::{verify_xrpl_claim, XrplAlgorithm, XrplSigner, XRPL_MAX_DROPS};
use serde_json::Value;

use crate::tx::LandClaim;

/// The chain this backend answers for, in the port's spelling.
pub(crate) const CHAIN: &str = "xrpl";

/// The fee ceiling a backend starts with, until the table's own is applied:
/// 0.1 XRP. Mirrors `connector_config::DEFAULT_XRPL_MAX_FEE_DROPS`, which this
/// crate cannot name.
const DEFAULT_MAX_FEE_DROPS: u64 = 100_000;

/// How many ledgers past the open one a landing may still be included in:
/// `LastLedgerSequence`, which bounds every wait (ADR 0087 decision 9).
pub(crate) const LAST_LEDGER_WINDOW: u32 = 20;

/// How many ledgers a prepared open may wait to be sent: longer than a
/// landing's, since the caller journals between preparing and sending.
const OPEN_LAST_LEDGER_WINDOW: u32 = 100;

/// The receiving half of the settlement port on one XRPL network.
///
/// **One key, one account.** The key is this node's `[settlement.xrpl.<id>]`
/// settlement key. An admitted channel must name its account as its
/// `Destination`, and it signs the `PaymentChannelClaim` that lands a
/// voucher. The account is never the account of another network (ADR 0087
/// decision 4: a claim binds no network).
pub struct XrplBatchSettlement {
    pub(crate) rpc: XrplRpc,
    pub(crate) signer: XrplSigner,
    pub(crate) address: String,
    pub(crate) network_id: u32,
    min_settle_delay_secs: u64,
    pub(crate) wait: TxWait,
    admitted: Mutex<HashSet<String>>,
    // The one sequence allocator of this node's account. A transaction signs
    // with the account's next `Sequence` and waits for the ledger to
    // validate it, and **every** transaction this node sends from the
    // account -- an open, a top-up, a landing, a close -- does so under this
    // lock, one at a time, so none races another's (the counterpart of EVM
    // nonce handling).
    pub(crate) sequencer: tokio::sync::Mutex<()>,
    // The `Sequence`s of opens prepared and not yet sent (ADR 0075 decision
    // 8 journals a prepared open before it is sent). Other transactions wait
    // for one rather than take it, since taking it would lapse the open.
    pub(crate) reserved: Mutex<BTreeSet<u32>>,
    // The channels this node opened as payer.
    pub(crate) outbound: Mutex<HashMap<String, crate::pay::Outbound>>,
    // The most a transaction's fee may be, in drops, before it is refused.
    pub(crate) max_fee_drops: u64,
    // How many ledgers past the current one a prepared open may still land.
    pub(crate) open_window: u32,
}

impl XrplBatchSettlement {
    /// Bind to the network `transport` reaches, admitting channels whose
    /// `SettleDelay` is at least `min_settle_delay_secs`, under `signer`'s
    /// account.
    ///
    /// Refuses, in order: an endpoint that cannot be read; a `server_info`
    /// `network_id` that is not `network_id` (ADR 0087 decision 5), which
    /// would mean this table's key names another network than the endpoint
    /// serves; and a settlement account the ledger does not hold. It never
    /// checks the PayChan amendment, which testnet reports disabled while
    /// channels work.
    ///
    /// # Errors
    /// [`BatchSettlementError::Backend`] naming which of these it was.
    pub async fn connect(
        transport: RpcTransport,
        network_id: u32,
        signer: XrplSigner,
        min_settle_delay_secs: u64,
    ) -> Result<XrplBatchSettlement, BatchSettlementError> {
        let rpc = XrplRpc::new(transport);
        let address = signer.classic_address();
        let info = rpc.server_info().await.map_err(rpc_error)?;
        if info.network_id.unwrap_or(0) != network_id {
            return Err(BatchSettlementError::Backend(format!(
                "the endpoint serves XRPL network {} but the table is for network {network_id}",
                info.network_id
                    .map_or("with no network_id (mainnet is 0)".to_string(), |id| id
                        .to_string())
            )));
        }
        let Some(account) = rpc.account_info(&address).await.map_err(rpc_error)? else {
            return Err(BatchSettlementError::Backend(format!(
                "the settlement account {address} does not exist on XRPL network {network_id}: \
                 fund it with at least the base reserve"
            )));
        };
        let Some(reserve) = info.reserve_base_drops else {
            return Err(BatchSettlementError::Backend(format!(
                "XRPL network {network_id}'s endpoint reports no validated ledger, so the \
                 settlement account's base reserve cannot be checked"
            )));
        };
        if account.balance_drops < reserve {
            return Err(BatchSettlementError::Backend(format!(
                "the settlement account {address} holds {} drops, below the base reserve of \
                 {reserve} on XRPL network {network_id}: fund it",
                account.balance_drops
            )));
        }
        Ok(XrplBatchSettlement {
            rpc,
            signer,
            address,
            network_id,
            min_settle_delay_secs,
            wait: TxWait {
                poll: Duration::from_millis(500),
                give_up_after: Duration::from_secs(60),
            },
            admitted: Mutex::new(HashSet::new()),
            sequencer: tokio::sync::Mutex::new(()),
            reserved: Mutex::new(BTreeSet::new()),
            outbound: Mutex::new(HashMap::new()),
            max_fee_drops: DEFAULT_MAX_FEE_DROPS,
            open_window: OPEN_LAST_LEDGER_WINDOW,
        })
    }

    /// The most a transaction's fee may be, in drops. A ledger whose open
    /// ledger fee has escalated past it refuses the send, naming the fee,
    /// rather than spending it. The default is 100,000 drops (0.1 XRP).
    #[must_use]
    pub fn with_max_fee_drops(mut self, max_fee_drops: u64) -> XrplBatchSettlement {
        self.max_fee_drops = max_fee_drops;
        self
    }

    /// How many ledgers past the current one a prepared open may still be
    /// included in (`LastLedgerSequence`). The default is 100: about six
    /// minutes on a network.
    #[must_use]
    pub fn with_open_window(mut self, ledgers: u32) -> XrplBatchSettlement {
        self.open_window = ledgers;
        self
    }

    /// How a landing paces its wait for validation. The default polls every
    /// 500 ms and gives up on an unreadable endpoint after 60 s.
    #[must_use]
    pub fn with_wait(mut self, wait: TxWait) -> XrplBatchSettlement {
        self.wait = wait;
        self
    }

    /// The settlement key's public key as the ledger spells a channel
    /// `PublicKey`'s peer: 33 bytes, lowercase hex, secp256k1 compressed or
    /// `ED`-prefixed Ed25519. What the self-description's `voucherSigners`
    /// publishes for this network (ADR 0087 decision 9).
    pub fn public_key_hex(&self) -> String {
        hex::encode(self.signer.public_key())
    }

    /// The smallest `SettleDelay` this backend admits, in seconds.
    pub fn min_settle_delay_secs(&self) -> u64 {
        self.min_settle_delay_secs
    }

    /// This node's settlement account, the `payTo` the greeting publishes.
    pub fn address(&self) -> &str {
        &self.address
    }

    /// The network this backend settles on (`server_info.network_id`).
    pub fn network_id(&self) -> u32 {
        self.network_id
    }

    /// The channel `presentation` names, as the port keeps it, and its
    /// ledger index. A string that is not 64 hex characters names nothing.
    pub(crate) fn parse(
        channel: &ChannelId,
    ) -> Result<(ChannelId, [u8; 32]), BatchSettlementError> {
        let bytes = hex::decode(&channel.0)
            .ok()
            .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
            .ok_or_else(|| BatchSettlementError::ChannelNotFound(channel.clone()))?;
        Ok((ChannelId(hex::encode(bytes)), bytes))
    }

    /// Every channel this node has admitted or restored in this process.
    pub(crate) fn admitted_channels(&self) -> Vec<ChannelId> {
        self.admitted
            .lock()
            .expect("admitted")
            .iter()
            .cloned()
            .map(ChannelId)
            .collect()
    }

    fn require_admitted(
        &self,
        channel: &ChannelId,
    ) -> Result<(ChannelId, [u8; 32]), BatchSettlementError> {
        let (id, bytes) = Self::parse(channel).map_err(|_| {
            // A string that names no channel was never admitted.
            BatchSettlementError::ChannelNotAdmitted(channel.clone())
        })?;
        if self.admitted.lock().expect("admitted").contains(&id.0) {
            Ok((id, bytes))
        } else {
            Err(BatchSettlementError::ChannelNotAdmitted(channel.clone()))
        }
    }

    /// The channel from the validated ledger, or `ChannelNotFound`: absent,
    /// or an entry of another type.
    pub(crate) async fn read(&self, id: &ChannelId) -> Result<Channel, BatchSettlementError> {
        let node = self
            .rpc
            .ledger_entry(&id.0)
            .await
            .map_err(rpc_error)?
            .ok_or_else(|| BatchSettlementError::ChannelNotFound(id.clone()))?;
        Channel::from_entry(&node).ok_or_else(|| BatchSettlementError::ChannelNotFound(id.clone()))
    }

    pub(crate) fn state_of(
        id: &ChannelId,
        channel: &Channel,
    ) -> Result<BatchChannelState, BatchSettlementError> {
        let key = channel.key.ok_or_else(|| {
            BatchSettlementError::Backend(format!(
                "channel {id} has a PublicKey that is not a 33-byte channel key"
            ))
        })?;
        let (status, collateral) = if channel.expiration.is_some() {
            // The payer has begun to leave: nothing backs a new voucher.
            (BatchChannelStatus::Closing, 0)
        } else {
            (
                BatchChannelStatus::Open,
                u128::from(channel.amount.saturating_sub(channel.balance)),
            )
        };
        Ok(BatchChannelState {
            id: id.clone(),
            status,
            voucher_signer: VoucherSigner::Xrpl(key),
            landed: u128::from(channel.balance),
            collateral,
        })
    }

    /// The admission rules of ADR 0087 decision 9, in order.
    fn judge(&self, id: &ChannelId, channel: &Channel) -> Result<(), BatchSettlementError> {
        let refuse = |refusal| {
            Err(BatchSettlementError::NotAdmissible {
                channel: id.clone(),
                refusal,
            })
        };
        if channel.destination != self.address {
            return refuse(AdmissionRefusal::NotPayableToThisNode {
                field: "Destination",
            });
        }
        if channel.settle_delay < self.min_settle_delay_secs {
            return refuse(AdmissionRefusal::DelayBelowMinimum {
                delay_secs: channel.settle_delay,
                minimum_secs: self.min_settle_delay_secs,
            });
        }
        if channel.expiration.is_some() {
            return refuse(AdmissionRefusal::NotOpen);
        }
        if channel.cancel_after.is_some() {
            return refuse(AdmissionRefusal::CancelAfter);
        }
        if channel
            .key
            .and_then(|key| XrplAlgorithm::of_public_key(&key))
            .is_none()
        {
            return refuse(AdmissionRefusal::InvalidChannelKey);
        }
        Ok(())
    }

    fn presented(
        presentation: &ChannelPresentation,
    ) -> Result<(ChannelId, [u8; 32]), BatchSettlementError> {
        match presentation {
            ChannelPresentation::Xrpl { channel } => Self::parse(channel),
            other => Err(BatchSettlementError::WrongChain {
                presented: other.chain(),
                backend: CHAIN,
            }),
        }
    }
}

#[async_trait]
impl BatchSettlementBackend for XrplBatchSettlement {
    async fn admit(
        &self,
        presentation: ChannelPresentation,
    ) -> Result<BatchChannelState, BatchSettlementError> {
        let (id, _) = Self::presented(&presentation)?;
        let channel = self.read(&id).await?;
        self.judge(&id, &channel)?;
        let state = Self::state_of(&id, &channel)?;
        self.admitted.lock().expect("admitted").insert(id.0);
        Ok(state)
    }

    async fn restore(
        &self,
        presentation: ChannelPresentation,
    ) -> Result<BatchChannelState, BatchSettlementError> {
        let (id, _) = Self::presented(&presentation)?;
        let channel = self.read(&id).await?;
        let state = Self::state_of(&id, &channel)?;
        self.admitted.lock().expect("admitted").insert(id.0);
        Ok(state)
    }

    async fn channel_state(
        &self,
        channel: &ChannelId,
    ) -> Result<BatchChannelState, BatchSettlementError> {
        let (id, _) = self.require_admitted(channel)?;
        let read = self.read(&id).await?;
        Self::state_of(&id, &read)
    }

    /// Land `voucher` with a destination `PaymentChannelClaim` whose
    /// `Balance` and `Amount` are the voucher's cumulative amount, **never**
    /// `tfClose`: the channel stays open. Works on a channel whose payer has
    /// begun to leave, until the ledger's close time reaches its
    /// `Expiration`.
    ///
    /// The voucher's signature is checked against the channel's `PublicKey`
    /// before anything is sent, so a voucher the ledger would refuse costs
    /// no fee. Success is read from the transaction's metadata: the
    /// channel's `Balance` advanced to the voucher's amount. `tesSUCCESS`
    /// alone is not a landing, since a claim on an expired channel succeeds
    /// by deleting it and delivers nothing.
    async fn land(
        &self,
        channel: &ChannelId,
        voucher: Voucher,
    ) -> Result<Landed, BatchSettlementError> {
        let (id, id_bytes) = self.require_admitted(channel)?;
        let amount = voucher.cumulative_amount;
        // Wider than the chain's supply is refused by name, never truncated.
        let drops = match u64::try_from(amount) {
            Ok(drops) if drops <= XRPL_MAX_DROPS => drops,
            _ => {
                return Err(BatchSettlementError::AmountExceedsChain {
                    amount,
                    max: XRPL_MAX_DROPS,
                })
            }
        };
        // Held from the read to the landing's fate, so a second land judges
        // staleness against the first one's outcome, not the ledger before it.
        let landing = self.sequence_turn().await;
        let read = self.read(&id).await?;
        let landed = u128::from(read.balance);
        if amount <= landed {
            return Err(BatchSettlementError::StaleVoucher { amount, landed });
        }
        let deposited = u128::from(read.amount);
        if amount > deposited {
            return Err(BatchSettlementError::VoucherExceedsDeposit { amount, deposited });
        }
        let key = read.key.ok_or_else(|| {
            BatchSettlementError::Backend(format!(
                "channel {id} has a PublicKey that is not a 33-byte channel key"
            ))
        })?;
        if !verify_xrpl_claim(&key, &id_bytes, amount, &voucher.signature) {
            return Err(BatchSettlementError::InvalidVoucherSignature(format!(
                "not the channel key's signature over a claim for {drops} drops"
            )));
        }

        let account = self
            .rpc
            .account_info(&self.address)
            .await
            .map_err(rpc_error)?
            .ok_or_else(|| {
                BatchSettlementError::Backend(format!(
                    "the settlement account {} does not exist",
                    self.address
                ))
            })?;
        let fee = self.rpc.fee().await.map_err(rpc_error)?;
        let fee_drops = fee.open_ledger_fee_drops.max(fee.base_fee_drops);
        if fee_drops > self.max_fee_drops {
            return Err(BatchSettlementError::Backend(format!(
                "the ledger asks {fee_drops} drops to land a PaymentChannelClaim, above this \
                 table's max_fee_drops of {}: nothing was signed",
                self.max_fee_drops
            )));
        }
        let last_ledger_sequence = fee.ledger_current_index + LAST_LEDGER_WINDOW;
        let signed = LandClaim {
            account: self.signer.account_id(),
            channel: id_bytes,
            drops,
            claim_signature: voucher.signature.clone(),
            channel_key: key,
            close: false,
            fee_drops,
            sequence: account.sequence,
            last_ledger_sequence,
        }
        .sign(&self.signer);

        let submitted = self.rpc.submit(&signed.blob_hex).await.map_err(rpc_error)?;
        // A definitive refusal at submission (`tem`, `tef`, `tel`) never
        // lands; anything else is settled by the bounded wait.
        if ["tem", "tef", "tel"]
            .iter()
            .any(|class| submitted.engine_result.starts_with(class))
        {
            return Err(BatchSettlementError::Backend(format!(
                "the ledger refused the PaymentChannelClaim: {}",
                submitted.engine_result
            )));
        }
        let fate = self
            .rpc
            .wait_for_tx(&signed.hash, last_ledger_sequence, self.wait)
            .await
            .map_err(rpc_error)?;
        drop(landing);
        match fate {
            TxFate::KnownNotLanded {
                last_ledger_sequence,
            } => Err(BatchSettlementError::Backend(format!(
                "the PaymentChannelClaim {} did not land by ledger {last_ledger_sequence} and \
                 never can: nothing was spent",
                signed.hash
            ))),
            TxFate::Landed {
                transaction_result,
                meta,
                ..
            } => {
                if transaction_result != "tesSUCCESS" {
                    return Err(BatchSettlementError::Backend(format!(
                        "the PaymentChannelClaim {} landed as {transaction_result} and delivered \
                         nothing",
                        signed.hash
                    )));
                }
                if !delivered(&meta, &id.0, drops) {
                    return Err(BatchSettlementError::Backend(format!(
                        "the PaymentChannelClaim {} answered tesSUCCESS but delivered nothing: \
                         the channel's Balance did not advance to {drops} (an expired channel is \
                         deleted by a claim)",
                        signed.hash
                    )));
                }
                let state = Self::state_of(&id, &self.read(&id).await?)?;
                Ok(Landed {
                    state,
                    transaction: signed.hash,
                })
            }
        }
    }
}

/// A `PayChannel` ledger entry, as far as admission and landing read it.
pub(crate) struct Channel {
    /// The source: the account that opened the channel.
    pub(crate) account: String,
    pub(crate) destination: String,
    /// Drops funded.
    pub(crate) amount: u64,
    /// Drops delivered.
    pub(crate) balance: u64,
    settle_delay: u64,
    pub(crate) expiration: Option<u64>,
    cancel_after: Option<u64>,
    /// `None` when the entry's `PublicKey` is not 33 bytes of hex.
    pub(crate) key: Option<[u8; 33]>,
}

impl Channel {
    /// `None` when `node` is not a `PayChannel` or lacks a field the ledger
    /// always sets.
    fn from_entry(node: &Value) -> Option<Channel> {
        if node["LedgerEntryType"].as_str()? != "PayChannel" {
            return None;
        }
        let drops = |name: &str| node[name].as_str()?.parse::<u64>().ok();
        Some(Channel {
            account: node["Account"].as_str()?.to_string(),
            destination: node["Destination"].as_str()?.to_string(),
            amount: drops("Amount")?,
            balance: drops("Balance")?,
            settle_delay: node["SettleDelay"].as_u64()?,
            expiration: node["Expiration"].as_u64(),
            cancel_after: node["CancelAfter"].as_u64(),
            key: node["PublicKey"]
                .as_str()
                .and_then(|text| hex::decode(text).ok())
                .and_then(|bytes| <[u8; 33]>::try_from(bytes).ok()),
        })
    }
}

/// Whether `meta` records the channel `channel_hex` modified, its `Balance`
/// advanced to exactly `drops`. A channel deleted instead (an expired one,
/// or one a `tfClose` ended) is a `DeletedNode` and delivers nothing here.
pub(crate) fn delivered(meta: &Value, channel_hex: &str, drops: u64) -> bool {
    let Some(nodes) = meta["AffectedNodes"].as_array() else {
        return false;
    };
    let drops_of = |fields: &Value| fields["Balance"].as_str()?.parse::<u64>().ok();
    nodes.iter().any(|node| {
        let modified = &node["ModifiedNode"];
        modified["LedgerEntryType"].as_str() == Some("PayChannel")
            && modified["LedgerIndex"]
                .as_str()
                .is_some_and(|index| index.eq_ignore_ascii_case(channel_hex))
            && drops_of(&modified["FinalFields"]) == Some(drops)
            && drops_of(&modified["PreviousFields"]).is_some_and(|before| before < drops)
    })
}

/// The drops a closing claim delivered on `channel_hex`, read from the
/// channel's `DeletedNode`: its final `Balance` less the one before the
/// transaction, or nothing when the transaction did not change it, which is
/// how a claim on a channel already past its `Expiration` reads. `None` when
/// the metadata deletes no such channel.
pub(crate) fn closed_delivering(meta: &Value, channel_hex: &str) -> Option<u64> {
    let drops_of = |fields: &Value| fields["Balance"].as_str()?.parse::<u64>().ok();
    meta["AffectedNodes"].as_array()?.iter().find_map(|node| {
        let deleted = &node["DeletedNode"];
        if deleted["LedgerEntryType"].as_str() != Some("PayChannel")
            || !deleted["LedgerIndex"]
                .as_str()
                .is_some_and(|index| index.eq_ignore_ascii_case(channel_hex))
        {
            return None;
        }
        let after = drops_of(&deleted["FinalFields"])?;
        let before = drops_of(&deleted["PreviousFields"]).unwrap_or(after);
        Some(after.saturating_sub(before))
    })
}

pub(crate) fn rpc_error(error: XrplRpcError) -> BatchSettlementError {
    match error.refusal() {
        Some(refused) => BatchSettlementError::Refused {
            endpoint: refused.endpoint,
            status: refused.status,
        },
        None => BatchSettlementError::Backend(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const CHANNEL: &str = "5DB01B7FFED6B67E6B0414DED11E051D2EE2B7619CE0EAA6286D67A3A4D5BDB3";

    fn entry(extra: Value) -> Value {
        let mut node = json!({
            "LedgerEntryType": "PayChannel",
            "Account": "rSource",
            "Destination": "rDest",
            "Amount": "1000",
            "Balance": "0",
            "SettleDelay": 3600,
            "PublicKey": format!("02{}", "ab".repeat(32)),
        });
        for (name, value) in extra.as_object().unwrap() {
            node[name] = value.clone();
        }
        node
    }

    #[test]
    fn a_pay_channel_entry_reads_and_another_entry_does_not() {
        let channel = Channel::from_entry(&entry(json!({"Expiration": 7}))).expect("reads");
        assert_eq!(channel.amount, 1_000);
        assert_eq!(channel.balance, 0);
        assert_eq!(channel.expiration, Some(7));
        assert_eq!(channel.cancel_after, None);
        assert_eq!(channel.key.map(|key| key[0]), Some(0x02));

        assert!(Channel::from_entry(&json!({"LedgerEntryType": "AccountRoot"})).is_none());
    }

    #[test]
    fn a_key_that_is_not_33_bytes_reads_as_none() {
        let channel = Channel::from_entry(&entry(json!({"PublicKey": "ABCD"}))).expect("reads");
        assert!(channel.key.is_none());
    }

    #[test]
    fn only_a_modified_channel_whose_balance_advanced_is_delivered() {
        let modified = |balance_before: &str, balance_after: &str| {
            json!({"AffectedNodes": [
                {"ModifiedNode": {
                    "LedgerEntryType": "AccountRoot",
                    "LedgerIndex": "AAAA",
                }},
                {"ModifiedNode": {
                    "LedgerEntryType": "PayChannel",
                    "LedgerIndex": CHANNEL,
                    "FinalFields": {"Balance": balance_after},
                    "PreviousFields": {"Balance": balance_before},
                }},
            ]})
        };
        let lower = CHANNEL.to_ascii_lowercase();
        assert!(delivered(&modified("0", "300"), &lower, 300));
        // Not the amount asked for, or no advance at all.
        assert!(!delivered(&modified("0", "200"), &lower, 300));
        assert!(!delivered(&modified("300", "300"), &lower, 300));
        // Another channel's metadata is not this one's.
        assert!(!delivered(&modified("0", "300"), &"11".repeat(32), 300));
        // An expired channel is deleted, and a deletion delivers nothing.
        let deleted = json!({"AffectedNodes": [
            {"DeletedNode": {
                "LedgerEntryType": "PayChannel",
                "LedgerIndex": CHANNEL,
                "FinalFields": {"Balance": "300"},
            }},
        ]});
        assert!(!delivered(&deleted, &lower, 300));
        assert!(!delivered(&json!({}), &lower, 300));
    }

    #[test]
    fn a_closing_claim_delivers_only_what_its_deletion_advanced() {
        let deleted = |previous: Value| {
            json!({"AffectedNodes": [
                {"DeletedNode": {
                    "LedgerEntryType": "PayChannel",
                    "LedgerIndex": CHANNEL,
                    "FinalFields": {"Balance": "300"},
                    "PreviousFields": previous,
                }},
            ]})
        };
        let lower = CHANNEL.to_ascii_lowercase();
        assert_eq!(
            closed_delivering(&deleted(json!({"Balance": "100"})), &lower),
            Some(200)
        );
        // A channel already past its Expiration is deleted untouched.
        assert_eq!(closed_delivering(&deleted(json!({})), &lower), Some(0));
        // Another channel's deletion, or none at all, is not this close.
        assert_eq!(
            closed_delivering(&deleted(json!({})), &"11".repeat(32)),
            None
        );
        assert_eq!(closed_delivering(&json!({}), &lower), None);
    }

    /// A backend over an endpoint nothing listens on: `judge` reads no chain.
    fn offline(address: &str) -> XrplBatchSettlement {
        XrplBatchSettlement {
            rpc: XrplRpc::new(RpcTransport::direct("http://127.0.0.1:9").expect("a url")),
            signer: XrplSigner::from_family_seed("snoPBrXtMeMyMHUVTgbuqAfg1SUTb").expect("seed"),
            address: address.to_string(),
            network_id: 1000,
            min_settle_delay_secs: 3600,
            wait: TxWait {
                poll: Duration::from_millis(10),
                give_up_after: Duration::from_secs(1),
            },
            admitted: Mutex::new(HashSet::new()),
            sequencer: tokio::sync::Mutex::new(()),
            reserved: Mutex::new(BTreeSet::new()),
            outbound: Mutex::new(HashMap::new()),
            max_fee_drops: DEFAULT_MAX_FEE_DROPS,
            open_window: OPEN_LAST_LEDGER_WINDOW,
        }
    }

    fn refusal_of(backend: &XrplBatchSettlement, channel: &Channel) -> Option<AdmissionRefusal> {
        match backend.judge(&ChannelId("aa".repeat(32)), channel) {
            Err(BatchSettlementError::NotAdmissible { refusal, .. }) => Some(refusal),
            Ok(()) => None,
            Err(other) => panic!("{other:?}"),
        }
    }

    /// Each rule refuses by its own name, a channel breaking none is
    /// admitted, and a `PublicKey` that is no channel key is refused. A real
    /// ledger never holds that last one -- it checks the key at creation --
    /// so only this test can reach it.
    #[test]
    fn every_admission_rule_refuses_by_name() {
        let backend = offline("rNode");
        let admissible = Channel {
            account: "rSource".to_string(),
            destination: "rNode".to_string(),
            amount: 1_000,
            balance: 0,
            settle_delay: 3600,
            expiration: None,
            cancel_after: None,
            key: Some([0x02; 33]),
        };
        assert_eq!(refusal_of(&backend, &admissible), None);

        let broken = |change: fn(&mut Channel)| {
            let mut channel = Channel {
                account: admissible.account.clone(),
                destination: admissible.destination.clone(),
                ..admissible
            };
            change(&mut channel);
            refusal_of(&backend, &channel)
        };
        assert_eq!(
            broken(|c| c.destination = "rSomeoneElse".to_string()),
            Some(AdmissionRefusal::NotPayableToThisNode {
                field: "Destination"
            })
        );
        assert_eq!(
            broken(|c| c.settle_delay = 3599),
            Some(AdmissionRefusal::DelayBelowMinimum {
                delay_secs: 3599,
                minimum_secs: 3600
            })
        );
        assert_eq!(
            broken(|c| c.expiration = Some(1)),
            Some(AdmissionRefusal::NotOpen)
        );
        assert_eq!(
            broken(|c| c.cancel_after = Some(u64::MAX)),
            Some(AdmissionRefusal::CancelAfter)
        );
        assert_eq!(
            broken(|c| c.key = None),
            Some(AdmissionRefusal::InvalidChannelKey)
        );
        assert_eq!(
            broken(|c| c.key = Some([0x05; 33])),
            Some(AdmissionRefusal::InvalidChannelKey),
            "a prefix that is neither a secp256k1 point's nor 0xED"
        );
    }
}
