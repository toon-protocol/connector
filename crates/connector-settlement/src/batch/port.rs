use async_trait::async_trait;
use thiserror::Error;

use crate::port::ChannelId;

/// Where a batch-settlement channel stands in its payer's lifecycle, as the
/// chain reports it (ADR 0074 decision 5). The union of both chains' states,
/// so the port can name each one; a chain reports only the states it has.
///
/// | status        | EVM (`x402BatchSettlement`)                      | Solana (`payment-channels`)                          |
/// | ------------- | ------------------------------------------------ | ---------------------------------------------------- |
/// | `Open`        | holds a balance, no withdrawal pending           | `status` Open                                        |
/// | `Withdrawing` | `pendingWithdrawals(id)` is set                  | never                                                |
/// | `Closing`     | never                                            | the payer called `request_close`                     |
/// | `Sealed`      | never                                            | sealed by `settle_and_seal`, or distributed          |
///
/// EVM has no terminal state: a channel whose payer finalised a withdrawal
/// is `Open` again with a smaller balance, and one drained to nothing is
/// `Open` with no collateral.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchChannelStatus {
    Open,
    /// EVM only: the payer has called `initiateWithdraw`. The pending amount
    /// is no longer collateral, and the connector must `claim` its latest
    /// voucher before `finalizeWithdraw` can run.
    Withdrawing,
    /// Solana only: the payer has called `request_close`. No new voucher is
    /// accepted, and only this node, as `payee`, can still land one, with
    /// `settle_and_seal`, before the grace period ends.
    Closing,
    /// Solana only: terminal. Nothing more can be landed.
    Sealed,
}

impl BatchChannelStatus {
    /// Whether a **new** voucher may be accepted on a channel in this state
    /// (ADR 0074 decisions 3 and 5). `Withdrawing` still accepts, up to the
    /// collateral the pending withdrawal leaves; `Closing` and `Sealed`
    /// accept nothing.
    ///
    /// Whether a voucher already accepted can still be **landed** is a
    /// different question, answered by
    /// [`BatchSettlementBackend::land`]: it can on every status but
    /// `Sealed`.
    pub fn accepts_vouchers(self) -> bool {
        matches!(
            self,
            BatchChannelStatus::Open | BatchChannelStatus::Withdrawing
        )
    }
}

/// Who a channel's vouchers must be signed by, as the chain fixes it at
/// open (ADR 0074 decision 4). Taken from the chain, never from a voucher.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VoucherSigner {
    /// An ECDSA address: `payerAuthorizer` when it is nonzero, otherwise
    /// `payer`. A channel whose `payer` is a contract wallet and whose
    /// `payerAuthorizer` is zero is never admitted
    /// ([`AdmissionRefusal::ContractWalletPayerWithoutAuthorizer`]), so this
    /// is always a key a packet's voucher can be checked against with no
    /// RPC.
    Evm([u8; 20]),
    /// The channel's `authorized_signer`, an Ed25519 public key.
    Solana([u8; 32]),
}

/// A snapshot of a batch-settlement channel, read from its chain.
///
/// `landed` and `collateral` are both cumulative-amount figures in the
/// token's base units, and together they bound what a new voucher may
/// claim: see [`voucher_ceiling`](Self::voucher_ceiling).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchChannelState {
    /// The channel, in its chain's canonical spelling: on EVM the
    /// `channelId` as `0x` and 64 lowercase hex digits, on Solana the
    /// channel account in base58. Always the id the channel was admitted
    /// under, so it can key the journal's watermark (ADR 0074 decision 3).
    pub id: ChannelId,
    pub status: BatchChannelStatus,
    pub voucher_signer: VoucherSigner,
    /// The cumulative amount recorded on chain in this node's favour: EVM
    /// `totalClaimed`, Solana `settled`. Only this protects the connector
    /// from a payer's exit; a voucher accepted but not landed does not
    /// (ADR 0074 decision 5).
    pub landed: u128,
    /// What still backs a voucher above `landed`, and so what a new
    /// voucher may add:
    ///
    /// - EVM: `balance − totalClaimed − pendingWithdrawal.amount`,
    ///   saturating at zero. It **falls** when a withdrawal is initiated,
    ///   which is why nothing may cache it as a lower bound.
    /// - Solana: `deposit − settled` while the channel is `Open`, and zero
    ///   in every other status.
    ///
    /// Zero whenever [`status`](Self::status) does not
    /// [accept vouchers](BatchChannelStatus::accepts_vouchers).
    pub collateral: u128,
}

impl BatchChannelState {
    /// The highest cumulative amount a new voucher on this channel may name
    /// right now: `landed + collateral`. A voucher above it is not backed
    /// (ADR 0074 decision 5). On a channel that accepts no vouchers it is
    /// exactly `landed`, so nothing new passes.
    pub fn voucher_ceiling(&self) -> u128 {
        self.landed.saturating_add(self.collateral)
    }
}

/// An EVM `ChannelConfig`, as `x402BatchSettlement` hashes it into a
/// `channelId` (`getChannelId`, an EIP-712 struct hash). Plain bytes: this
/// port neither hashes nor checks it; an implementation does.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EvmChannelConfig {
    pub payer: [u8; 20],
    pub payer_authorizer: [u8; 20],
    pub receiver: [u8; 20],
    pub receiver_authorizer: [u8; 20],
    pub token: [u8; 20],
    /// `uint40` on chain, seconds.
    pub withdraw_delay: u64,
    pub salt: [u8; 32],
}

/// What a client hands over so this node can find its channel on chain and
/// judge it (ADR 0074 decision 2). The channel is named by the voucher,
/// never derived from the pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelPresentation {
    /// EVM stores channels by id and the config is not readable back from
    /// the chain, so the client presents it with its first voucher. The
    /// implementation recomputes `getChannelId(config)` off chain and
    /// refuses a mismatch with [`BatchSettlementError::ChannelIdMismatch`]
    /// before it reads anything.
    Evm {
        channel: ChannelId,
        config: EvmChannelConfig,
    },
    /// Solana: the channel account alone. Every field the admission rules
    /// judge is read from it, and the implementation re-derives the PDA from
    /// the account's own seed fields before trusting any of them.
    Solana { channel: ChannelId },
}

impl ChannelPresentation {
    /// The channel this presentation names.
    pub fn channel(&self) -> &ChannelId {
        match self {
            ChannelPresentation::Evm { channel, .. } | ChannelPresentation::Solana { channel } => {
                channel
            }
        }
    }

    /// The chain this presentation is for: `"evm"` or `"solana"`, the same
    /// spelling as the config's `[settlement.<chain>]` table.
    pub fn chain(&self) -> &'static str {
        match self {
            ChannelPresentation::Evm { .. } => "evm",
            ChannelPresentation::Solana { .. } => "solana",
        }
    }
}

/// A voucher, as this port lands it: a signed cumulative amount on one
/// channel (ADR 0074 decision 4). Deliberately minimal. Parsing the wire
/// claim, checking its signature against [`BatchChannelState::voucher_signer`]
/// and its freshness against the journal's watermark all happen before a
/// voucher reaches this port, in `connector-signer` and `connector-domain`.
///
/// On Solana the signed message also carries `expires_at`. It is always
/// zero here: a voucher with a nonzero one is refused structurally before it
/// is accepted, so an implementation rebuilds the message with zero.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Voucher {
    /// The cumulative amount the voucher signs: EVM `maxClaimableAmount`
    /// (a `uint128`), Solana `cumulative` (a `u64`). Landing it records
    /// exactly this much; an EVM implementation claims `totalClaimed` equal
    /// to it.
    pub cumulative_amount: u128,
    /// EVM: the 65-byte ECDSA signature over the voucher digest. Solana:
    /// the 64-byte Ed25519 signature over the 50-byte message.
    pub signature: Vec<u8>,
}

/// Why an otherwise real channel is not one this node will accept vouchers
/// on (ADR 0074 decisions 2, 4 and 5). Each names the rule it breaks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmissionRefusal {
    /// The channel does not pay this node, or gives a seat that must be this
    /// node's to someone else. `field` is the chain's own name for the seat:
    /// on EVM `"receiver"` or `"receiverAuthorizer"` (both must be this
    /// node's settlement address); on Solana `"payee"`, `"rent_payer"` (both
    /// must be the sponsor key) or `"distribution_hash"` (it must commit to
    /// exactly one recipient, this node's receiving account, at 10000 bps).
    NotPayableToThisNode { field: &'static str },
    /// EVM `token` or Solana `mint` is not the token this node settles in.
    TokenNotSettled,
    /// EVM `withdrawDelay` or Solana `grace_period` is below this node's
    /// published minimum.
    DelayBelowMinimum { delay_secs: u64, minimum_secs: u64 },
    /// Solana: the channel is not Open. A channel already closing or sealed
    /// can back no new voucher.
    NotOpen,
    /// EVM: `payerAuthorizer` is zero and `payer` is a contract, so every
    /// voucher would need an ERC-1271 `eth_call` to verify. The client
    /// names a `payerAuthorizer` instead (ADR 0074 decision 4).
    ContractWalletPayerWithoutAuthorizer,
}

impl std::fmt::Display for AdmissionRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AdmissionRefusal::NotPayableToThisNode { field } => {
                write!(f, "its {field} is not this node")
            }
            AdmissionRefusal::TokenNotSettled => {
                write!(f, "it holds a token this node does not settle in")
            }
            AdmissionRefusal::DelayBelowMinimum {
                delay_secs,
                minimum_secs,
            } => write!(
                f,
                "its delay of {delay_secs}s is below this node's minimum of {minimum_secs}s"
            ),
            AdmissionRefusal::NotOpen => write!(f, "it is not open"),
            AdmissionRefusal::ContractWalletPayerWithoutAuthorizer => write!(
                f,
                "its payer is a contract wallet and it names no payerAuthorizer"
            ),
        }
    }
}

/// Errors a [`BatchSettlementBackend`] reports. Every variant but
/// [`Backend`](BatchSettlementError::Backend) is a rule of the port; that
/// one is an I/O failure specific to how an implementation reaches its
/// chain.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BatchSettlementError {
    /// Nothing exists on chain under this id: no EVM channel has ever held a
    /// balance there, or no Solana account of the program lives there.
    #[error("batch-settlement channel '{0}' not found")]
    ChannelNotFound(ChannelId),

    /// The presentation names one channel and describes another: the EVM
    /// config hashes to `derived`, not the id presented, or the Solana
    /// account's own seed fields derive another address.
    #[error("batch-settlement channel '{presented}' does not match its own configuration, which derives '{derived}'")]
    ChannelIdMismatch {
        presented: ChannelId,
        derived: ChannelId,
    },

    /// The presentation is for a chain this backend does not settle on.
    #[error("a {presented} channel was presented to the {backend} batch-settlement backend")]
    WrongChain {
        presented: &'static str,
        backend: &'static str,
    },

    /// The channel exists and is not one this node accepts vouchers on.
    #[error("batch-settlement channel '{channel}' is not admitted: {refusal}")]
    NotAdmissible {
        channel: ChannelId,
        refusal: AdmissionRefusal,
    },

    /// [`BatchSettlementBackend::land`] or
    /// [`BatchSettlementBackend::channel_state`] on a channel this backend
    /// has not admitted. On EVM `claim` needs the full config, which only
    /// admission supplies, so this is structural rather than a policy.
    #[error("batch-settlement channel '{0}' has not been admitted")]
    ChannelNotAdmitted(ChannelId),

    /// A voucher that does not exceed what is already landed. On EVM the
    /// contract would silently ignore it; this port refuses it by name
    /// instead, so a caller can tell "already recorded" from "recorded now".
    #[error("voucher for {amount} does not exceed the {landed} already landed")]
    StaleVoucher { amount: u128, landed: u128 },

    /// A voucher above what the chain will let land: EVM `balance`, Solana
    /// `deposit`. Refused before anything is submitted, and retryable: the
    /// same voucher lands once the payer deposits enough.
    #[error("voucher for {amount} exceeds the channel's deposit of {deposited}")]
    VoucherExceedsDeposit { amount: u128, deposited: u128 },

    /// Solana: the channel is sealed, and nothing more can land on it.
    #[error("batch-settlement channel '{0}' is sealed")]
    ChannelSealed(ChannelId),

    /// The signature could not be put in the form the chain verifies, found
    /// before submission.
    #[error("voucher signature is invalid: {0}")]
    InvalidVoucherSignature(String),

    #[error("batch-settlement backend error: {0}")]
    Backend(String),
}

/// The receive-only settlement port for x402 `batch-settlement` channels
/// (ADR 0074 decision 9).
///
/// It is **not** [`SettlementBackend`](crate::SettlementBackend) bent to fit.
/// A batch-settlement channel has no `open` this node calls, no side of its
/// own and a different lifecycle on each chain, so `own_deposited`, `fund`
/// and `close` would mean nothing here. This node never opens, funds or signs
/// on such a channel; it admits one a client opened, reads it, and lands the
/// client's vouchers on it.
///
/// Implementations live in `connector-settlement-evm` (issue #1342) and
/// `connector-settlement-solana` (issue #1343), as modules beside the
/// existing backends, reusing their RPC clients and keys. Each is built from
/// its `[settlement.<chain>.batch_settlement]` table and the enclosing
/// settlement table: this node's receiving identity is that table's
/// settlement key, its token that table's `token_address`, and its minimum
/// delay the batch table's. [`InMemoryBatchSettlement`](super::InMemoryBatchSettlement)
/// is the fake, and [`super::contract`] is the suite every implementation
/// must pass unmodified (ADR 0007).
///
/// **What is not here.** Voucher signature verification and the amount-only
/// watermark (issue #1341) happen before a voucher reaches this port. The
/// watchers and sweeps (issue #1344) are callers of it: they re-read
/// [`channel_state`](Self::channel_state) when a channel's payer moves and
/// [`land`](Self::land) the latest voucher. The EVM `settle` sweep and the
/// Solana `distribute` / `reclaim` move money already landed and belong to
/// those sweeps, not to this port.
#[async_trait]
pub trait BatchSettlementBackend: Send + Sync {
    /// Find the presented channel on chain and judge it against this node's
    /// rules (ADR 0074 decision 2). On success the channel is admitted:
    /// [`channel_state`](Self::channel_state) and [`land`](Self::land) then
    /// work on it, and the state returned is its current one.
    ///
    /// The checks, in order: the presentation is for this backend's chain
    /// ([`WrongChain`](BatchSettlementError::WrongChain)); it names itself
    /// consistently ([`ChannelIdMismatch`](BatchSettlementError::ChannelIdMismatch));
    /// the channel exists ([`ChannelNotFound`](BatchSettlementError::ChannelNotFound));
    /// it passes every admission rule
    /// ([`NotAdmissible`](BatchSettlementError::NotAdmissible)).
    ///
    /// Idempotent: admitting an admitted channel again re-reads it and
    /// returns its state. A second channel from a payer this node already
    /// holds one from is admitted on its own merits, never refused for the
    /// pair (ADR 0074 decision 2's exception to ADR 0059).
    async fn admit(
        &self,
        presentation: ChannelPresentation,
    ) -> Result<BatchChannelState, BatchSettlementError>;

    /// The admitted channel's state, read from the chain now. Never answered
    /// from a cache: collateral can fall on EVM, and a watcher that has just
    /// seen `WithdrawInitiated` or a Solana close relies on this to see it.
    async fn channel_state(
        &self,
        channel: &ChannelId,
    ) -> Result<BatchChannelState, BatchSettlementError>;

    /// Land `voucher` on the admitted `channel`, so its amount is recorded
    /// on chain in this node's favour: EVM `claim`; Solana `settle` while
    /// Open and `settle_and_seal` while Closing. Returns the state after.
    ///
    /// Works on every status but `Sealed`, and that is the point: landing is
    /// what protects value already accepted once a payer begins to exit.
    /// Refuses, leaving the channel as it was, a voucher that does not
    /// exceed [`BatchChannelState::landed`]
    /// ([`StaleVoucher`](BatchSettlementError::StaleVoucher)) and one
    /// above the channel's deposit
    /// ([`VoucherExceedsDeposit`](BatchSettlementError::VoucherExceedsDeposit)).
    ///
    /// On Solana, landing on a `Closing` channel seals it: `settle_and_seal`
    /// is the only instruction that can land there, and it is terminal.
    async fn land(
        &self,
        channel: &ChannelId,
        voucher: Voucher,
    ) -> Result<BatchChannelState, BatchSettlementError>;
}
