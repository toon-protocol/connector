//! The EVM paying half of the batch-settlement port (ADR 0075 decisions 2
//! and 3, issue #1374): this node as the **payer** on an
//! `x402BatchSettlement` channel toward a counterparty. It is the same
//! [`EvmBatchSettlementBackend`] as the receiving half, over the same RPC
//! client and the same [`Sender`](crate::send::Sender), so every write here
//! takes its nonce from the settlement key's one sequence (ADR 0073).
//!
//! # The channel it opens
//!
//! [`BatchSettlementPayer::open`] builds a `ChannelConfig` in which
//!
//! - `payer` **and** `payerAuthorizer` are this node's settlement address.
//!   The settlement key signs every voucher (ADR 0075 decision 3), and a
//!   nonzero `payerAuthorizer` is what the counterparty's admission rule
//!   asks for (ADR 0074 decision 2). `[signer]` is identity only and never
//!   appears;
//! - `receiver` and `receiverAuthorizer` are both the counterparty's
//!   published settlement address;
//! - `token` is the one both settle in;
//! - `withdrawDelay` is the counterparty's published minimum, raised to the
//!   contract's own floor of 15 minutes when it is lower, so it is always at
//!   least what the counterparty asks and never a delay the contract
//!   refuses;
//! - `salt` is 32 fresh random bytes.
//!
//! # Deposits
//!
//! The contract takes value only through a deposit collector, and this node
//! sends `deposit` itself and pays its own gas. Which collector is decided
//! once, from the token: one that answers EIP-3009's `authorizationState`
//! goes through `ERC3009DepositCollector`, with a `receiveWithAuthorization`
//! signed by the settlement key and a fresh collector salt per deposit
//! (it makes the ERC-3009 nonce); any other goes through
//! `Permit2DepositCollector`, after a one-time `approve` of Permit2, with a
//! `permitWitnessTransferFrom` bound to the channel. A top-up is another
//! `deposit` into the same config.
//!
//! # What is remembered
//!
//! For the process lifetime, each channel this node opened: its config, the
//! highest amount it has signed a voucher for, and how much the channel
//! backs. Backing moves only by this node's own writes -- its deposits, its
//! withdrawals -- so it is re-read from the chain after each of them and
//! [`sign_voucher`](BatchSettlementPayer::sign_voucher) answers from it with
//! no RPC. It is lowered **before** a withdrawal is sent and raised only
//! after a deposit is confirmed, so a voucher is never signed against
//! backing that is on its way out.
//!
//! Journaling an outbound config before its opening deposit is sent, and
//! bringing a channel back after a restart with its watermark from the
//! receiver's `POST /ilp/claim-state`, are the operator surface's and the
//! peering's (issues #1376, #1378).

use async_trait::async_trait;
use connector_settlement::batch::{
    BatchSettlementError, BatchSettlementPayer, ChannelPresentation, EvmChannelConfig,
    OpenedChannel, OutboundChannelState, ReceiverTerms, Voucher, VoucherSigner,
};
use connector_settlement::ChannelId;
use connector_signer::evm_voucher_digest;
use ethers::abi::{encode, Token};
use ethers::core::rand::random;
use ethers::middleware::Middleware;
use ethers::types::{Address, BlockNumber, Bytes, U256};
use ethers::utils::keccak256;

use crate::batch_settlement::{
    admitted_id, backend_error, chain_config, signer_config, EvmBatchSettlementBackend, Snapshot,
};
use crate::bindings::deposit::DepositToken;
use crate::channel_id::format_channel_id;
use crate::send::confirm;

/// `ERC3009DepositCollector`'s canonical address, the same on every
/// network x402 is deployed to (ADR 0074, Sources).
pub const ERC3009_DEPOSIT_COLLECTOR_ADDRESS: [u8; 20] = [
    0x40, 0x20, 0x80, 0x60, 0x89, 0x47, 0x0a, 0x89, 0x82, 0x6c, 0xb9, 0xfb, 0x1f, 0x40, 0x59, 0x15,
    0x0b, 0x55, 0x00, 0x04,
];

/// `Permit2DepositCollector`'s canonical address.
pub const PERMIT2_DEPOSIT_COLLECTOR_ADDRESS: [u8; 20] = [
    0x40, 0x20, 0x42, 0x5f, 0xaf, 0x3b, 0x74, 0x6c, 0x08, 0x2c, 0x2f, 0x94, 0x2b, 0x4e, 0x51, 0x59,
    0x88, 0x7b, 0x00, 0x05,
];

/// Uniswap's Permit2, at the address it holds on every chain.
pub const PERMIT2_ADDRESS: [u8; 20] = [
    0x00, 0x00, 0x00, 0x00, 0x00, 0x22, 0xd4, 0x73, 0x03, 0x0f, 0x11, 0x6d, 0xde, 0xe9, 0xf6, 0xb4,
    0x3a, 0xc7, 0x8b, 0xa3,
];

/// `x402BatchSettlement`'s `MIN_WITHDRAW_DELAY` and `MAX_WITHDRAW_DELAY`:
/// a `deposit` whose config names a delay outside them reverts.
const CONTRACT_MIN_WITHDRAW_DELAY_SECS: u64 = 15 * 60;
const CONTRACT_MAX_WITHDRAW_DELAY_SECS: u64 = 30 * 24 * 60 * 60;

/// EIP-3009's `ReceiveWithAuthorization` struct, which an ERC-3009 deposit
/// authorisation is signed over.
const RECEIVE_WITH_AUTHORIZATION_TYPE: &[u8] = b"ReceiveWithAuthorization(address from,address to,uint256 value,uint256 validAfter,uint256 validBefore,bytes32 nonce)";

/// Permit2's `TokenPermissions` struct.
const TOKEN_PERMISSIONS_TYPE: &[u8] = b"TokenPermissions(address token,uint256 amount)";

/// Permit2's witness-transfer struct as `Permit2DepositCollector` completes
/// it: its `DEPOSIT_WITNESS_TYPE_STRING` appended to Permit2's stub.
const PERMIT_WITNESS_TRANSFER_FROM_TYPE: &[u8] = b"PermitWitnessTransferFrom(TokenPermissions permitted,address spender,uint256 nonce,uint256 deadline,DepositWitness witness)DepositWitness(bytes32 channelId)TokenPermissions(address token,uint256 amount)";

/// `Permit2DepositCollector`'s `DEPOSIT_WITNESS_TYPEHASH` preimage.
const DEPOSIT_WITNESS_TYPE: &[u8] = b"DepositWitness(bytes32 channelId)";

/// Which collector a deposit goes through. See the module doc.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepositRoute {
    /// The token has EIP-3009: `ERC3009DepositCollector`.
    Erc3009,
    /// It does not: `Permit2DepositCollector`, after a one-time `approve`.
    Permit2,
}

/// A channel this node opened, as its paying half remembers it.
#[derive(Debug, Clone)]
pub(crate) struct Outbound {
    config: EvmChannelConfig,
    /// The highest cumulative amount signed on it: the next voucher must
    /// exceed it.
    signed: u128,
    /// The highest cumulative amount a voucher on it may name: `balance −
    /// pendingWithdrawal` as of this node's last write. See the module doc.
    backed: u128,
}

/// What a channel backs, from one reading of it. A voucher above what is
/// already landed is covered only by `balance` less what a pending
/// withdrawal will take; one at or below `landed` is never signed, since
/// this node signs only above its own watermark, which the receiver's
/// landing never passes.
fn backing(snapshot: &Snapshot) -> u128 {
    snapshot.balance.saturating_sub(snapshot.pending_withdrawal)
}

/// A deposit of nothing, which `x402BatchSettlement` reverts on: refused
/// here, before anything is signed or sent.
fn zero_deposit() -> BatchSettlementError {
    BatchSettlementError::Backend("x402BatchSettlement refuses a deposit of zero".to_string())
}

impl EvmBatchSettlementBackend {
    /// Which collector this node's deposits go through, asked of the token
    /// once. A revert on `authorizationState` is the answer "no EIP-3009";
    /// any other failure is the chain not answering, and is an error rather
    /// than a guess.
    pub async fn deposit_route(&self) -> Result<DepositRoute, BatchSettlementError> {
        self.deposit_route
            .get_or_try_init(|| async {
                let token = DepositToken::new(self.token, std::sync::Arc::clone(&self.client));
                connector_chain_rpc::retry_read(|| async {
                    match token
                        .authorization_state(self.own_address, [0u8; 32])
                        .call()
                        .await
                    {
                        Ok(_) => Ok(DepositRoute::Erc3009),
                        Err(error) if error.is_revert() => Ok(DepositRoute::Permit2),
                        Err(error) => Err(error),
                    }
                })
                .await
                .map_err(|error| {
                    BatchSettlementError::Backend(format!(
                        "could not ask token {:?} whether it has EIP-3009: {error}",
                        self.token
                    ))
                })
            })
            .await
            .copied()
    }

    /// The config this node opens a channel toward `terms` under. See the
    /// module doc for each field.
    fn outbound_config(
        &self,
        terms: ReceiverTerms,
    ) -> Result<EvmChannelConfig, BatchSettlementError> {
        let ReceiverTerms::Evm(terms) = terms else {
            return Err(BatchSettlementError::WrongChain {
                presented: terms.chain(),
                backend: "evm",
            });
        };
        if terms.token != self.token.to_fixed_bytes() {
            return Err(BatchSettlementError::TokenNotShared);
        }
        let withdraw_delay = terms
            .min_withdraw_delay_secs
            .max(CONTRACT_MIN_WITHDRAW_DELAY_SECS);
        if withdraw_delay > CONTRACT_MAX_WITHDRAW_DELAY_SECS {
            return Err(BatchSettlementError::Backend(format!(
                "the counterparty's minimum withdrawDelay of {withdraw_delay}s is above \
                 x402BatchSettlement's maximum of {CONTRACT_MAX_WITHDRAW_DELAY_SECS}s, so no \
                 channel it would admit can exist"
            )));
        }
        let own = self.own_address.to_fixed_bytes();
        Ok(EvmChannelConfig {
            payer: own,
            payer_authorizer: own,
            receiver: terms.receiver,
            receiver_authorizer: terms.receiver,
            token: terms.token,
            withdraw_delay,
            salt: random(),
        })
    }

    fn require_outbound(&self, channel: &ChannelId) -> Result<Outbound, BatchSettlementError> {
        self.outbound_record()
            .get(channel)
            .cloned()
            .ok_or_else(|| BatchSettlementError::NotOutbound(channel.clone()))
    }

    fn outbound_record(
        &self,
    ) -> std::sync::MutexGuard<'_, std::collections::HashMap<ChannelId, Outbound>> {
        self.outbound
            .lock()
            .expect("EvmBatchSettlementBackend outbound lock poisoned")
    }

    /// Set what `channel` backs, as this node's own write left it.
    fn record_backing(&self, channel: &ChannelId, backed: u128) {
        if let Some(outbound) = self.outbound_record().get_mut(channel) {
            outbound.backed = backed;
        }
    }

    /// Read `channel` now, record what it backs, and report it.
    async fn read_outbound(
        &self,
        channel: &ChannelId,
        config: &EvmChannelConfig,
    ) -> Result<OutboundChannelState, BatchSettlementError> {
        let snapshot = self.snapshot(admitted_id(channel)?).await?;
        self.record_backing(channel, backing(&snapshot));
        let signed = self.require_outbound(channel)?.signed;
        Ok(OutboundChannelState {
            on_chain: self.state(channel, config, &snapshot),
            signed,
        })
    }

    /// Send `transaction` from the settlement key and wait for it to land.
    async fn transact(
        &self,
        transaction: ethers::types::transaction::eip2718::TypedTransaction,
    ) -> Result<(), BatchSettlementError> {
        let hash = self.sender.send(transaction).await.map_err(backend_error)?;
        confirm(&self.client, hash, self.confirm)
            .await
            .map_err(backend_error)?;
        Ok(())
    }

    /// `deposit(config, amount, collector, collectorData)`, sent and paid
    /// for by this node, through whichever collector the token takes.
    async fn deposit(
        &self,
        config: &EvmChannelConfig,
        amount: u128,
    ) -> Result<(), BatchSettlementError> {
        if amount == 0 {
            return Err(zero_deposit());
        }
        let channel_id =
            connector_signer::evm_batch_channel_id(&self.domain(), &signer_config(config));
        let (collector, collector_data) = match self.deposit_route().await? {
            DepositRoute::Erc3009 => self.erc3009_authorization(channel_id, amount).await?,
            DepositRoute::Permit2 => self.permit2_transfer(channel_id, amount).await?,
        };
        self.transact(
            self.contract
                .deposit(chain_config(config), amount, collector, collector_data)
                .tx,
        )
        .await
    }

    /// `ERC3009DepositCollector`'s `collectorData`: a
    /// `receiveWithAuthorization` from this node to the collector, whose
    /// nonce is `keccak256(channelId, salt)` under a fresh salt.
    async fn erc3009_authorization(
        &self,
        channel_id: [u8; 32],
        amount: u128,
    ) -> Result<(Address, Bytes), BatchSettlementError> {
        let collector = Address::from(ERC3009_DEPOSIT_COLLECTOR_ADDRESS);
        let token = DepositToken::new(self.token, std::sync::Arc::clone(&self.client));
        let separator =
            connector_chain_rpc::retry_read(|| async { token.domain_separator().call().await })
                .await
                .map_err(backend_error)?;
        let salt = U256::from_big_endian(&random::<[u8; 32]>());
        let nonce = keccak256(encode(&[
            Token::FixedBytes(channel_id.to_vec()),
            Token::Uint(salt),
        ]));
        let (valid_after, valid_before) = (U256::zero(), U256::MAX);
        let struct_hash = keccak256(encode(&[
            Token::FixedBytes(keccak256(RECEIVE_WITH_AUTHORIZATION_TYPE).to_vec()),
            Token::Address(self.own_address),
            Token::Address(collector),
            Token::Uint(U256::from(amount)),
            Token::Uint(valid_after),
            Token::Uint(valid_before),
            Token::FixedBytes(nonce.to_vec()),
        ]));
        let signature = self.sign_typed(separator, struct_hash)?;
        Ok((
            collector,
            Bytes::from(encode(&[
                Token::Uint(valid_after),
                Token::Uint(valid_before),
                Token::Uint(salt),
                Token::Bytes(signature.to_vec()),
            ])),
        ))
    }

    /// `Permit2DepositCollector`'s `collectorData`: a Permit2
    /// `permitWitnessTransferFrom` to the collector, its witness the
    /// channel, under a fresh unordered nonce. Approves Permit2 for the
    /// token first, once, if it is not already approved for `amount`.
    async fn permit2_transfer(
        &self,
        channel_id: [u8; 32],
        amount: u128,
    ) -> Result<(Address, Bytes), BatchSettlementError> {
        let collector = Address::from(PERMIT2_DEPOSIT_COLLECTOR_ADDRESS);
        let permit2 = Address::from(PERMIT2_ADDRESS);
        let token = DepositToken::new(self.token, std::sync::Arc::clone(&self.client));
        let allowance = connector_chain_rpc::retry_read(|| async {
            token.allowance(self.own_address, permit2).call().await
        })
        .await
        .map_err(backend_error)?;
        if allowance < U256::from(amount) {
            self.transact(token.approve(permit2, U256::MAX).tx).await?;
        }
        let permit2_domain = DepositToken::new(permit2, std::sync::Arc::clone(&self.client));
        let separator = connector_chain_rpc::retry_read(|| async {
            permit2_domain.domain_separator().call().await
        })
        .await
        .map_err(backend_error)?;

        let nonce = U256::from_big_endian(&random::<[u8; 32]>());
        let deadline = U256::MAX;
        let permitted = keccak256(encode(&[
            Token::FixedBytes(keccak256(TOKEN_PERMISSIONS_TYPE).to_vec()),
            Token::Address(self.token),
            Token::Uint(U256::from(amount)),
        ]));
        let witness = keccak256(encode(&[
            Token::FixedBytes(keccak256(DEPOSIT_WITNESS_TYPE).to_vec()),
            Token::FixedBytes(channel_id.to_vec()),
        ]));
        let struct_hash = keccak256(encode(&[
            Token::FixedBytes(keccak256(PERMIT_WITNESS_TRANSFER_FROM_TYPE).to_vec()),
            Token::FixedBytes(permitted.to_vec()),
            Token::Address(collector),
            Token::Uint(nonce),
            Token::Uint(deadline),
            Token::FixedBytes(witness.to_vec()),
        ]));
        let signature = self.sign_typed(separator, struct_hash)?;
        Ok((
            collector,
            Bytes::from(encode(&[
                Token::Uint(nonce),
                Token::Uint(deadline),
                Token::Bytes(signature.to_vec()),
                Token::Bytes(Vec::new()),
            ])),
        ))
    }

    /// The settlement key's EIP-712 signature over `struct_hash` under the
    /// domain whose separator is `separator`.
    fn sign_typed(
        &self,
        separator: [u8; 32],
        struct_hash: [u8; 32],
    ) -> Result<[u8; 65], BatchSettlementError> {
        let mut preimage = Vec::with_capacity(66);
        preimage.extend_from_slice(&[0x19, 0x01]);
        preimage.extend_from_slice(&separator);
        preimage.extend_from_slice(&struct_hash);
        self.sender
            .sign_digest(keccak256(preimage))
            .map_err(backend_error)
    }

    /// The latest block's timestamp: the clock a withdrawal's delay is
    /// measured against.
    async fn chain_now(&self) -> Result<u64, BatchSettlementError> {
        let block = connector_chain_rpc::retry_read(|| self.client.get_block(BlockNumber::Latest))
            .await
            .map_err(backend_error)?
            .ok_or_else(|| {
                BatchSettlementError::Backend("the chain has no latest block".to_string())
            })?;
        Ok(block.timestamp.as_u64())
    }
}

#[async_trait]
impl BatchSettlementPayer for EvmBatchSettlementBackend {
    async fn open(
        &self,
        terms: ReceiverTerms,
        deposit: u128,
    ) -> Result<OpenedChannel, BatchSettlementError> {
        let config = self.outbound_config(terms)?;
        let id = connector_signer::evm_batch_channel_id(&self.domain(), &signer_config(&config));
        let channel = format_channel_id(id);
        if deposit == 0 {
            return Err(zero_deposit());
        }
        let _paying = self.paying.lock().await;
        // Recorded before the deposit is sent, backing nothing yet, so a
        // deposit whose confirmation is lost still leaves a channel this
        // node can read, top up and withdraw from.
        self.outbound_record().insert(
            channel.clone(),
            Outbound {
                config: config.clone(),
                signed: 0,
                backed: 0,
            },
        );
        let deposited = self.deposit(&config, deposit).await;
        match (deposited, self.snapshot(id).await) {
            // Landed, whatever the confirmation said: the salt is fresh, so
            // nothing but this deposit can have put a balance there.
            (_, Ok(snapshot)) if snapshot.balance > 0 => {
                self.record_backing(&channel, backing(&snapshot));
            }
            (Err(error), Ok(_)) => {
                self.outbound_record().remove(&channel);
                return Err(error);
            }
            (Ok(()), Ok(_)) => {
                return Err(BatchSettlementError::Backend(format!(
                    "the opening deposit into '{channel}' was confirmed, but the channel \
                     holds nothing"
                )));
            }
            (Err(error), Err(_)) | (Ok(()), Err(error)) => {
                return Err(BatchSettlementError::Backend(format!(
                    "whether the opening deposit into '{channel}' landed is unknown; this \
                     node keeps it as its own channel, so its state can be read again: {error}"
                )));
            }
        }
        Ok(OpenedChannel {
            presentation: ChannelPresentation::Evm { channel, config },
            voucher_signer: VoucherSigner::Evm(self.own_address.to_fixed_bytes()),
        })
    }

    async fn top_up(
        &self,
        channel: &ChannelId,
        increment: u128,
    ) -> Result<OutboundChannelState, BatchSettlementError> {
        let config = self.require_outbound(channel)?.config;
        let _paying = self.paying.lock().await;
        self.deposit(&config, increment).await?;
        self.read_outbound(channel, &config).await
    }

    /// Answered from this node's own record, with no RPC: see the module
    /// doc. The digest is `x402BatchSettlement`'s `getVoucherDigest`, and
    /// the settlement key signs it.
    async fn sign_voucher(
        &self,
        channel: &ChannelId,
        cumulative_amount: u128,
    ) -> Result<Voucher, BatchSettlementError> {
        let id = admitted_id(channel)?;
        let mut record = self.outbound_record();
        let outbound = record
            .get_mut(channel)
            .ok_or_else(|| BatchSettlementError::NotOutbound(channel.clone()))?;
        if cumulative_amount <= outbound.signed {
            return Err(BatchSettlementError::VoucherNotAdvancing {
                amount: cumulative_amount,
                signed: outbound.signed,
            });
        }
        if cumulative_amount > outbound.backed {
            return Err(BatchSettlementError::VoucherUnbacked {
                amount: cumulative_amount,
                backed: outbound.backed,
            });
        }
        let digest = evm_voucher_digest(&self.domain(), &id, cumulative_amount);
        let signature = self.sender.sign_digest(digest).map_err(backend_error)?;
        outbound.signed = cumulative_amount;
        Ok(Voucher {
            cumulative_amount,
            signature: signature.to_vec(),
        })
    }

    /// `initiateWithdraw` for `balance − totalClaimed`. What the channel
    /// backs drops to what is landed before anything is sent, so no voucher
    /// is signed against value on its way out; it is re-read from the chain
    /// whatever the outcome.
    async fn start_withdrawal(
        &self,
        channel: &ChannelId,
    ) -> Result<OutboundChannelState, BatchSettlementError> {
        let config = self.require_outbound(channel)?.config;
        let _paying = self.paying.lock().await;
        let before = self.snapshot(admitted_id(channel)?).await?;
        let unlanded = before.balance.saturating_sub(before.total_claimed);
        if !before.withdrawal_pending && unlanded > 0 {
            self.record_backing(channel, before.total_claimed);
            let sent = self
                .transact(
                    self.contract
                        .initiate_withdraw(chain_config(&config), unlanded)
                        .tx,
                )
                .await;
            if let Err(error) = sent {
                self.read_outbound(channel, &config).await?;
                return Err(error);
            }
        }
        self.read_outbound(channel, &config).await
    }

    /// `finalizeWithdraw`, once the chain's clock has passed
    /// `initiatedAt + withdrawDelay`. The contract caps what it returns at
    /// `balance − totalClaimed` as it stands then, so a voucher the
    /// receiver landed inside the delay stays the receiver's.
    async fn finish_withdrawal(
        &self,
        channel: &ChannelId,
    ) -> Result<OutboundChannelState, BatchSettlementError> {
        let config = self.require_outbound(channel)?.config;
        let _paying = self.paying.lock().await;
        let before = self.snapshot(admitted_id(channel)?).await?;
        if !before.withdrawal_pending {
            return Err(BatchSettlementError::NoWithdrawalPending(channel.clone()));
        }
        let due_at = before.initiated_at.saturating_add(config.withdraw_delay);
        let now = self.chain_now().await?;
        if now < due_at {
            return Err(BatchSettlementError::WithdrawalNotDue {
                channel: channel.clone(),
                remaining_secs: due_at - now,
            });
        }
        self.transact(self.contract.finalize_withdraw(chain_config(&config)).tx)
            .await?;
        self.read_outbound(channel, &config).await
    }

    async fn outbound_state(
        &self,
        channel: &ChannelId,
    ) -> Result<OutboundChannelState, BatchSettlementError> {
        let config = self.require_outbound(channel)?.config;
        self.read_outbound(channel, &config).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_collector_and_permit2_constants_are_the_canonical_addresses() {
        let spelled = |bytes: [u8; 20]| format!("{:?}", Address::from(bytes));
        assert_eq!(
            spelled(ERC3009_DEPOSIT_COLLECTOR_ADDRESS),
            "0x4020806089470a89826cb9fb1f4059150b550004"
        );
        assert_eq!(
            spelled(PERMIT2_DEPOSIT_COLLECTOR_ADDRESS),
            "0x4020425faf3b746c082c2f942b4e5159887b0005"
        );
        assert_eq!(
            spelled(PERMIT2_ADDRESS),
            "0x000000000022d473030f116ddee9f6b43ac78ba3"
        );
    }

    #[test]
    fn a_withdrawal_pending_takes_its_amount_out_of_what_backs_a_voucher() {
        let snapshot = Snapshot {
            balance: 1_500,
            total_claimed: 300,
            pending_withdrawal: 1_200,
            withdrawal_pending: true,
            initiated_at: 1,
        };
        assert_eq!(backing(&snapshot), 300);
        let open = Snapshot {
            pending_withdrawal: 0,
            withdrawal_pending: false,
            initiated_at: 0,
            ..snapshot
        };
        assert_eq!(backing(&open), 1_500);
    }
}
