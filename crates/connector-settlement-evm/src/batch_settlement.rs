//! The EVM implementation of the receive-only batch-settlement port (ADR
//! 0074 decisions 2 and 5, issue #1342): a client's x402 `batch-settlement`
//! channel in `x402BatchSettlement`, which this node admits, reads and
//! `claim`s on, and never opens, funds or signs for.
//!
//! # Admission
//!
//! The contract stores a channel by id only, so a client presents its
//! `ChannelConfig` with its first voucher. [`EvmBatchSettlementBackend::admit`]
//! recomputes the id off chain with `connector_signer::evm_batch_channel_id`
//! -- the one implementation of `getChannelId` -- and refuses a mismatch
//! before it reads anything. It then reads `channels(id)` and
//! `pendingWithdrawals(id)` and admits the channel only if:
//!
//! - `receiver` **and** `receiverAuthorizer` are this node's settlement
//!   address. `receiverAuthorizer` can refund to the payer anything earned
//!   but not yet claimed, so it is never anyone else's (decision 5);
//! - `token` is the token this node settles in;
//! - `withdrawDelay` is at least this node's published minimum;
//! - `payerAuthorizer` is nonzero, or `payer` has no code. The contract
//!   checks a zero-`payerAuthorizer` voucher with OpenZeppelin's
//!   `SignatureChecker`, which asks ERC-1271 of any `payer` with code
//!   (an EIP-7702-delegated account included), and a packet must never wait
//!   on that `eth_call` (decision 4).
//!
//! `payer`, `payerAuthorizer` and `salt` are the client's, and a second
//! channel from one payer is admitted on its own merits (decision 2's
//! exception to ADR 0059).
//!
//! # What is remembered
//!
//! Admission is the only place a channel's config is learned, and `claim`
//! needs the whole config, so this backend keeps it -- in memory, for the
//! process lifetime. Nothing else is cached: every figure a
//! [`BatchChannelState`] reports is read from the chain when asked, because
//! collateral **falls** when a payer initiates a withdrawal (decision 5).
//! After a restart a channel must be presented again before it can be landed
//! on: the client edge journals each channel's config with its first
//! accepted voucher, and the runtime **restores** every journaled channel at
//! boot -- recomputing its id and reading it, but judging no admission rule,
//! so a policy tightened across the restart never strands a voucher already
//! accepted (ADR 0074 decision 5). New vouchers still need `admit`.
//!
//! # Reads are one snapshot
//!
//! `balance`, `totalClaimed` and the pending withdrawal are read in a single
//! `eth_call` through the contract's own `multicall`, so collateral is never
//! computed from two different blocks. Read as two calls, a
//! `finalizeWithdraw` landing between them would pair the pre-withdrawal
//! balance with the cleared withdrawal and overstate collateral by the whole
//! amount withdrawn.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;
use connector_settlement::batch::{
    AdmissionRefusal, BatchChannelState, BatchChannelStatus, BatchSettlementBackend,
    BatchSettlementError, ChannelPresentation, EvmChannelConfig, Voucher, VoucherSigner,
};
use connector_settlement::ChannelId;
use connector_signer::{
    evm_batch_channel_id, evm_voucher_signer, verify_evm_voucher, BatchChannelConfig,
    BatchSettlementDomain,
};
use ethers::abi::{AbiDecode, AbiEncode};
use ethers::middleware::Middleware;
use ethers::types::{Address, Bytes};

use crate::bindings::x402_batch_settlement::{
    ChannelConfig, ChannelsCall, ChannelsReturn, PendingWithdrawalsCall, PendingWithdrawalsReturn,
    Voucher as X402Voucher, VoucherClaim, X402BatchSettlement,
};
use crate::channel_id::format_channel_id;
use crate::send::{confirm, ConfirmPolicy, Sender};
use crate::{EvmClient, EvmSettlementBackend};

/// The receive-only [`BatchSettlementBackend`] over `x402BatchSettlement`
/// (ADR 0074). Built from the node's [`EvmSettlementBackend`] by
/// [`EvmSettlementBackend::batch_settlement`], and sharing its RPC client,
/// its settlement key and that key's nonce count: this node's receiving
/// identity on a batch-settlement channel is its settlement address, and
/// `claim` is sent from it as the channel's `receiverAuthorizer`.
pub struct EvmBatchSettlementBackend {
    pub(crate) contract: X402BatchSettlement<EvmClient>,
    /// The EIP-712 domain every channel id and voucher digest is computed
    /// under: this chain, and the configured contract. Never a claim's.
    domain: BatchSettlementDomain,
    /// This node's settlement address: the `receiver` and
    /// `receiverAuthorizer` an admissible channel must name.
    pub(crate) own_address: Address,
    /// The token this node settles in: the enclosing `[settlement.evm]`
    /// table's `token_address`, never declared twice (CF-26).
    pub(crate) token: Address,
    min_withdraw_delay_secs: u64,
    pub(crate) client: Arc<EvmClient>,
    pub(crate) sender: Arc<Sender>,
    pub(crate) confirm: ConfirmPolicy,
    /// Every channel admitted or restored, by its canonical id, with the
    /// config it was presented under. See the module doc for why this is
    /// the one thing kept.
    admitted: Mutex<HashMap<ChannelId, EvmChannelConfig>>,
}

impl EvmSettlementBackend {
    /// This node's receive-only backend for x402 `batch-settlement`
    /// channels (ADR 0074), over the `x402BatchSettlement` at
    /// `contract_address`, admitting channels whose `withdrawDelay` is at
    /// least `min_withdraw_delay_secs`: what `[settlement.evm.batch_settlement]`
    /// holds. Everything else a channel must name is this backend's: its
    /// settlement address as `receiver` and `receiverAuthorizer`, and its
    /// token.
    ///
    /// Refuses unless the contract at `contract_address` computes the same
    /// channel id for a probe config as this node does, which checks in one
    /// `eth_call` that something is deployed there and that it is
    /// `x402BatchSettlement` under the domain vouchers will be checked
    /// against: this chain, this address, its EIP-712 name and version, and
    /// the `ChannelConfig` type hash.
    pub async fn batch_settlement(
        &self,
        contract_address: Address,
        min_withdraw_delay_secs: u64,
    ) -> Result<EvmBatchSettlementBackend, BatchSettlementError> {
        let contract = X402BatchSettlement::new(contract_address, Arc::clone(&self.client));
        let domain = BatchSettlementDomain {
            chain_id: self.chain_id,
            verifying_contract: contract_address.to_fixed_bytes(),
        };
        let probe = EvmChannelConfig {
            payer: [0x01; 20],
            payer_authorizer: [0x02; 20],
            receiver: self.own_address.to_fixed_bytes(),
            receiver_authorizer: self.own_address.to_fixed_bytes(),
            token: self.token.address().to_fixed_bytes(),
            withdraw_delay: min_withdraw_delay_secs,
            salt: [0x03; 32],
        };
        let expected = evm_batch_channel_id(&domain, &signer_config(&probe));
        let on_chain = connector_chain_rpc::retry_read(|| async {
            contract.get_channel_id(chain_config(&probe)).call().await
        })
        .await
        .map_err(|error| {
            BatchSettlementError::Backend(format!(
                "no x402BatchSettlement answers at {contract_address:?}: {error}"
            ))
        })?;
        if on_chain != expected {
            return Err(BatchSettlementError::Backend(format!(
                "the contract at {contract_address:?} computes channel ids under another \
                 EIP-712 domain than x402BatchSettlement on chain {} at that address; it is not \
                 the contract vouchers here are signed for",
                self.chain_id
            )));
        }
        Ok(EvmBatchSettlementBackend {
            contract,
            domain,
            own_address: self.own_address,
            token: self.token.address(),
            min_withdraw_delay_secs,
            client: Arc::clone(&self.client),
            sender: Arc::clone(&self.sender),
            confirm: self.confirm,
            admitted: Mutex::new(HashMap::new()),
        })
    }
}

/// One consistent reading of a channel: `channels(id)` and
/// `pendingWithdrawals(id)` from the same block.
struct Snapshot {
    balance: u128,
    total_claimed: u128,
    pending_withdrawal: u128,
    withdrawal_pending: bool,
}

impl EvmBatchSettlementBackend {
    /// The EIP-712 domain vouchers on this backend's channels are signed
    /// under: this chain and the configured `x402BatchSettlement`.
    pub fn domain(&self) -> BatchSettlementDomain {
        self.domain
    }

    /// This node's receiving identity: the address an admissible channel
    /// names as both `receiver` and `receiverAuthorizer`, and the `payTo` and
    /// `receiverAuthorizer` a greeting publishes (ADR 0074 decision 8).
    pub fn own_address(&self) -> Address {
        self.own_address
    }

    /// The shortest `withdrawDelay` this backend admits, in seconds.
    pub fn min_withdraw_delay_secs(&self) -> u64 {
        self.min_withdraw_delay_secs
    }

    /// The config `channel` was admitted or restored under, if it has been
    /// in this process. The voucher signer is
    /// `connector_signer::evm_voucher_signer` of it.
    pub fn admitted_config(&self, channel: &ChannelId) -> Option<EvmChannelConfig> {
        self.admitted().get(channel).cloned()
    }

    fn admitted(&self) -> MutexGuard<'_, HashMap<ChannelId, EvmChannelConfig>> {
        self.admitted
            .lock()
            .expect("EvmBatchSettlementBackend lock poisoned")
    }

    fn require_admitted(
        &self,
        channel: &ChannelId,
    ) -> Result<EvmChannelConfig, BatchSettlementError> {
        self.admitted_config(channel)
            .ok_or_else(|| BatchSettlementError::ChannelNotAdmitted(channel.clone()))
    }

    /// `channels(id)` and `pendingWithdrawals(id)` in one `eth_call`, through
    /// the contract's own `multicall`. See the module doc for why.
    async fn snapshot(&self, id: [u8; 32]) -> Result<Snapshot, BatchSettlementError> {
        let calls = vec![
            Bytes::from(ChannelsCall { channel_id: id }.encode()),
            Bytes::from(PendingWithdrawalsCall { channel_id: id }.encode()),
        ];
        let answers = self
            .contract
            .multicall(calls)
            .call()
            .await
            .map_err(backend_error)?;
        let [channel, pending] = answers.as_slice() else {
            return Err(BatchSettlementError::Backend(format!(
                "x402BatchSettlement's multicall answered {} results for 2 calls",
                answers.len()
            )));
        };
        let channel = ChannelsReturn::decode(channel).map_err(backend_error)?;
        let pending = PendingWithdrawalsReturn::decode(pending).map_err(backend_error)?;
        Ok(Snapshot {
            balance: channel.balance,
            total_claimed: channel.total_claimed,
            pending_withdrawal: pending.amount,
            withdrawal_pending: pending.initiated_at != 0,
        })
    }

    fn state(
        &self,
        channel: &ChannelId,
        config: &EvmChannelConfig,
        snapshot: &Snapshot,
    ) -> BatchChannelState {
        BatchChannelState {
            id: channel.clone(),
            status: if snapshot.withdrawal_pending {
                BatchChannelStatus::Withdrawing
            } else {
                BatchChannelStatus::Open
            },
            voucher_signer: VoucherSigner::Evm(evm_voucher_signer(&signer_config(config))),
            landed: snapshot.total_claimed,
            collateral: snapshot
                .balance
                .saturating_sub(snapshot.total_claimed)
                .saturating_sub(snapshot.pending_withdrawal),
        }
    }

    /// The channel `presentation` names, once it is shown to be an EVM
    /// presentation whose config derives the id it came with, and to exist
    /// on chain: its canonical id, its config and one reading of it. What
    /// both `admit` and `restore` check before anything else.
    async fn locate(
        &self,
        presentation: ChannelPresentation,
    ) -> Result<(ChannelId, EvmChannelConfig, Snapshot), BatchSettlementError> {
        let ChannelPresentation::Evm { channel, config } = presentation else {
            return Err(BatchSettlementError::WrongChain {
                presented: presentation.chain(),
                backend: "evm",
            });
        };
        let id = evm_batch_channel_id(&self.domain, &signer_config(&config));
        let canonical = format_channel_id(id);
        if parse_id(&channel) != Some(id) {
            return Err(BatchSettlementError::ChannelIdMismatch {
                presented: channel,
                derived: canonical,
            });
        }

        let snapshot = self.snapshot(id).await?;
        // A channel is created by its first deposit and holds a balance
        // until everything unclaimed is withdrawn; one that never held
        // anything, or was emptied with nothing ever claimed, is nothing
        // this node could be paid on.
        if snapshot.balance == 0 && snapshot.total_claimed == 0 {
            return Err(BatchSettlementError::ChannelNotFound(canonical));
        }
        Ok((canonical, config, snapshot))
    }

    /// The rules of ADR 0074 decision 2 this node fixes, in the order the
    /// record lists them; the first broken one is the refusal.
    async fn judge(
        &self,
        config: &EvmChannelConfig,
    ) -> Result<Option<AdmissionRefusal>, BatchSettlementError> {
        let own = self.own_address.to_fixed_bytes();
        if config.receiver != own {
            return Ok(Some(AdmissionRefusal::NotPayableToThisNode {
                field: "receiver",
            }));
        }
        if config.receiver_authorizer != own {
            return Ok(Some(AdmissionRefusal::NotPayableToThisNode {
                field: "receiverAuthorizer",
            }));
        }
        if config.token != self.token.to_fixed_bytes() {
            return Ok(Some(AdmissionRefusal::TokenNotSettled));
        }
        if config.withdraw_delay < self.min_withdraw_delay_secs {
            return Ok(Some(AdmissionRefusal::DelayBelowMinimum {
                delay_secs: config.withdraw_delay,
                minimum_secs: self.min_withdraw_delay_secs,
            }));
        }
        if config.payer_authorizer == [0u8; 20] {
            let code = self
                .client
                .get_code(Address::from(config.payer), None)
                .await
                .map_err(backend_error)?;
            if !code.is_empty() {
                return Ok(Some(AdmissionRefusal::ContractWalletPayerWithoutAuthorizer));
            }
        }
        Ok(None)
    }
}

#[async_trait]
impl BatchSettlementBackend for EvmBatchSettlementBackend {
    async fn admit(
        &self,
        presentation: ChannelPresentation,
    ) -> Result<BatchChannelState, BatchSettlementError> {
        let (canonical, config, snapshot) = self.locate(presentation).await?;
        if let Some(refusal) = self.judge(&config).await? {
            return Err(BatchSettlementError::NotAdmissible {
                channel: canonical,
                refusal,
            });
        }
        let state = self.state(&canonical, &config, &snapshot);
        self.admitted().insert(canonical, config);
        Ok(state)
    }

    /// [`admit`](Self::admit) without [`judge`](Self::judge): the id is
    /// still recomputed from the presented config and the channel still read
    /// from the chain, because `claim` sends that config and a wrong one
    /// reverts.
    async fn restore(
        &self,
        presentation: ChannelPresentation,
    ) -> Result<BatchChannelState, BatchSettlementError> {
        let (canonical, config, snapshot) = self.locate(presentation).await?;
        let state = self.state(&canonical, &config, &snapshot);
        self.admitted().insert(canonical, config);
        Ok(state)
    }

    async fn channel_state(
        &self,
        channel: &ChannelId,
    ) -> Result<BatchChannelState, BatchSettlementError> {
        let config = self.require_admitted(channel)?;
        let id = admitted_id(channel)?;
        let snapshot = self.snapshot(id).await?;
        Ok(self.state(channel, &config, &snapshot))
    }

    /// `claim` with one row, for exactly the voucher's amount: the row's
    /// `maxClaimableAmount` and `totalClaimed` are both
    /// [`Voucher::cumulative_amount`], since that is what the payer signed
    /// and what this node was paid.
    ///
    /// The signature is checked here before anything is sent, against the
    /// signer the admitted config names: the contract would revert on a bad
    /// one, and a revert still costs gas.
    async fn land(
        &self,
        channel: &ChannelId,
        voucher: Voucher,
    ) -> Result<BatchChannelState, BatchSettlementError> {
        let config = self.require_admitted(channel)?;
        let id = admitted_id(channel)?;
        let amount = voucher.cumulative_amount;

        let signature: [u8; 65] = voucher.signature.as_slice().try_into().map_err(|_| {
            BatchSettlementError::InvalidVoucherSignature(format!(
                "an EVM voucher signature is 65 bytes, not {}",
                voucher.signature.len()
            ))
        })?;
        let signer = evm_voucher_signer(&signer_config(&config));
        if !verify_evm_voucher(&self.domain, &id, amount, &signature, &signer) {
            return Err(BatchSettlementError::InvalidVoucherSignature(format!(
                "it is not a voucher for {amount} on '{channel}' by 0x{}",
                hex(&signer)
            )));
        }

        let before = self.snapshot(id).await?;
        if amount <= before.total_claimed {
            return Err(BatchSettlementError::StaleVoucher {
                amount,
                landed: before.total_claimed,
            });
        }
        if amount > before.balance {
            return Err(BatchSettlementError::VoucherExceedsDeposit {
                amount,
                deposited: before.balance,
            });
        }

        let row = VoucherClaim {
            voucher: X402Voucher {
                channel: chain_config(&config),
                max_claimable_amount: amount,
            },
            signature: Bytes::from(signature.to_vec()),
            total_claimed: amount,
        };
        let hash = self
            .sender
            .send(self.contract.claim(vec![row]).tx)
            .await
            .map_err(backend_error)?;
        confirm(&self.client, hash, self.confirm)
            .await
            .map_err(backend_error)?;

        let after = self.snapshot(id).await?;
        Ok(self.state(channel, &config, &after))
    }
}

/// The port's config as `connector-signer` hashes it: the same seven fields.
pub(crate) fn signer_config(config: &EvmChannelConfig) -> BatchChannelConfig {
    BatchChannelConfig {
        payer: config.payer,
        payer_authorizer: config.payer_authorizer,
        receiver: config.receiver,
        receiver_authorizer: config.receiver_authorizer,
        token: config.token,
        withdraw_delay: config.withdraw_delay,
        salt: config.salt,
    }
}

/// The port's config as the contract's ABI takes it.
pub(crate) fn chain_config(config: &EvmChannelConfig) -> ChannelConfig {
    ChannelConfig {
        payer: Address::from(config.payer),
        payer_authorizer: Address::from(config.payer_authorizer),
        receiver: Address::from(config.receiver),
        receiver_authorizer: Address::from(config.receiver_authorizer),
        token: Address::from(config.token),
        withdraw_delay: config.withdraw_delay,
        salt: config.salt,
    }
}

/// A presented channel id's 32 bytes, in any hex case, with or without
/// `0x`; `None` for anything that is not 32 bytes of hex. A presentation
/// whose id does not parse cannot match the id its config derives.
pub(crate) fn parse_id(channel: &ChannelId) -> Option<[u8; 32]> {
    let digits = channel.0.strip_prefix("0x").unwrap_or(&channel.0);
    if digits.len() != 64 || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let mut id = [0u8; 32];
    for (i, byte) in id.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&digits[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(id)
}

/// An admitted channel's id: admission stores only canonical ids, so this
/// parses by construction.
fn admitted_id(channel: &ChannelId) -> Result<[u8; 32], BatchSettlementError> {
    parse_id(channel).ok_or_else(|| BatchSettlementError::ChannelNotAdmitted(channel.clone()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(crate) fn backend_error<E: std::fmt::Display>(error: E) -> BatchSettlementError {
    BatchSettlementError::Backend(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_presented_id_parses_in_either_case_and_with_or_without_its_prefix() {
        let id = [0xabu8; 32];
        let lower = format_channel_id(id);
        assert_eq!(parse_id(&lower), Some(id));
        assert_eq!(
            parse_id(&ChannelId(lower.0.to_uppercase().replace("0X", "0x"))),
            Some(id)
        );
        assert_eq!(parse_id(&ChannelId(lower.0[2..].to_string())), Some(id));
    }

    #[test]
    fn a_presented_id_that_is_not_32_bytes_of_hex_parses_to_nothing() {
        for bad in [
            "",
            "0x",
            "0x1234",
            "batch-channel-0",
            &format!("0x{}", "zz".repeat(32)),
        ] {
            assert_eq!(parse_id(&ChannelId(bad.to_string())), None, "{bad}");
        }
    }

    #[test]
    fn the_signer_and_chain_views_of_a_config_carry_the_same_seven_fields() {
        let config = EvmChannelConfig {
            payer: [1; 20],
            payer_authorizer: [2; 20],
            receiver: [3; 20],
            receiver_authorizer: [4; 20],
            token: [5; 20],
            withdraw_delay: 86_400,
            salt: [6; 32],
        };
        let signer = signer_config(&config);
        let chain = chain_config(&config);
        assert_eq!(chain.payer.to_fixed_bytes(), signer.payer);
        assert_eq!(
            chain.payer_authorizer.to_fixed_bytes(),
            signer.payer_authorizer
        );
        assert_eq!(chain.receiver.to_fixed_bytes(), signer.receiver);
        assert_eq!(
            chain.receiver_authorizer.to_fixed_bytes(),
            signer.receiver_authorizer
        );
        assert_eq!(chain.token.to_fixed_bytes(), signer.token);
        assert_eq!(chain.withdraw_delay, signer.withdraw_delay);
        assert_eq!(chain.salt, signer.salt);
    }
}
