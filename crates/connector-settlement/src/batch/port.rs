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
    /// `payer`. A channel whose `payerAuthorizer` is zero is never admitted
    /// ([`AdmissionRefusal::NoPayerAuthorizer`]), so on an admitted channel
    /// this is always `payerAuthorizer`: a key a packet's voucher can be
    /// checked against with no RPC, and one the contract checks by ECDSA
    /// for the channel's whole life. (A channel only restored may predate
    /// that rule, and name `payer`.)
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

/// What a counterparty publishes about the channels it will receive on (ADR
/// 0075 decision 3): its self-description's `batchSettlements` entry for
/// one chain, in plain bytes. The paying half opens a channel toward it on
/// exactly these terms, so that the counterparty's receiving half admits
/// it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReceiverTerms {
    Evm(EvmReceiverTerms),
    Solana(SolanaReceiverTerms),
}

impl ReceiverTerms {
    /// The chain these terms are for, spelled as
    /// [`ChannelPresentation::chain`] spells it.
    pub fn chain(&self) -> &'static str {
        match self {
            ReceiverTerms::Evm(_) => "evm",
            ReceiverTerms::Solana(_) => "solana",
        }
    }

    /// The shortest delay the counterparty admits a channel with, in
    /// seconds: EVM `withdrawDelay`, Solana `grace_period`. A channel this
    /// node opens carries exactly this, which is how long winding it down
    /// takes (ADR 0075, Consequences).
    pub fn min_delay_secs(&self) -> u64 {
        match self {
            ReceiverTerms::Evm(terms) => terms.min_withdraw_delay_secs,
            ReceiverTerms::Solana(terms) => terms.min_grace_period_secs,
        }
    }
}

/// An EVM counterparty's terms: what the `ChannelConfig` this node builds
/// toward it must name (ADR 0075 decision 3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvmReceiverTerms {
    /// `payTo`: the counterparty's settlement address, which the config
    /// names as both `receiver` and `receiverAuthorizer`.
    pub receiver: [u8; 20],
    /// `asset`: the token the counterparty settles in, which must be this
    /// node's too.
    pub token: [u8; 20],
    /// `withdrawDelay`: the counterparty's published minimum, in seconds.
    pub min_withdraw_delay_secs: u64,
}

/// A Solana counterparty's terms: what the `open` this node builds toward
/// it must name, and where it goes to be co-signed (ADR 0075 decision 3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SolanaReceiverTerms {
    /// `feePayer`: the counterparty's sponsor key, which takes the fee
    /// payer, `rent_payer` and `payee` seats.
    pub sponsor: [u8; 32],
    /// `payTo`: the owner of the counterparty's receiving account, the one
    /// distribution entry at 10000 bps.
    pub receiver: [u8; 32],
    /// `asset`: the mint the counterparty settles in, which must be this
    /// node's too.
    pub mint: [u8; 32],
    /// `withdrawDelay`: the counterparty's minimum `grace_period`, in
    /// seconds.
    pub min_grace_period_secs: u64,
    /// `minDeposit`: the smallest opening deposit the counterparty's
    /// sponsor co-signs, in base units.
    pub min_deposit: u128,
    /// `sponsorEndpoint`, resolved against the counterparty's URL: where
    /// the payer-signed `open` is posted (ADR 0074 decision 9).
    pub sponsor_endpoint: String,
}

/// A channel a payer opened, as the receiver is shown it, and the signer
/// the chain recorded for it. What [`BatchSettlementPayer::open`] returns:
/// on EVM the presentation carries the `ChannelConfig` the first voucher
/// must carry (ADR 0075 decision 3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenedChannel {
    pub presentation: ChannelPresentation,
    pub voucher_signer: VoucherSigner,
}

/// A channel this node pays on, as its paying half sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboundChannelState {
    /// The channel as its chain reports it, in the terms the receiving half
    /// uses: what the receiver has landed, and what still backs a voucher
    /// above that.
    pub on_chain: BatchChannelState,
    /// The highest cumulative amount this node has signed a voucher for on
    /// the channel: its own watermark, which the next voucher it signs must
    /// exceed. It runs ahead of [`BatchChannelState::landed`] until the
    /// receiver lands.
    pub signed: u128,
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
    /// EVM: `payerAuthorizer` is zero, whatever `payer` is (ADR 0074
    /// decision 2, amended 2026-09-25). With none the contract checks a
    /// voucher against `payer` through `SignatureChecker`, which asks
    /// ERC-1271 of a payer with code -- and an EOA payer can gain code at
    /// any time by an EIP-7702 delegation, stranding the ECDSA vouchers this
    /// node already accepted. The client names a `payerAuthorizer` instead
    /// (decision 6).
    NoPayerAuthorizer,
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
            AdmissionRefusal::NoPayerAuthorizer => write!(f, "it names no payerAuthorizer"),
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
    /// has neither admitted nor restored. On EVM `claim` needs the full
    /// config, which only a presentation supplies, so this is structural
    /// rather than a policy.
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

    /// The paying half was asked to act on a channel this node did not open
    /// as payer (ADR 0075 decision 2). A channel it only receives on is not
    /// one it can fund, sign on or withdraw from.
    #[error("batch-settlement channel '{0}' is not one this node opened")]
    NotOutbound(ChannelId),

    /// The counterparty's published terms name a token this node does not
    /// settle in. The two ends of a channel must share one (ADR 0075
    /// decision 4), and nothing was opened.
    #[error("the counterparty settles in a token this node does not")]
    TokenNotShared,

    /// The counterparty declined to take part in the open, and said why by
    /// name: on Solana, its sponsor endpoint's refusal (ADR 0074 decision 9),
    /// such as a deposit below its published minimum. Nothing was opened.
    #[error("the counterparty refused the open: {0}")]
    OpenRefused(String),

    /// A voucher this node would sign that does not exceed the highest it
    /// has already signed on the channel. The receiver's watermark would
    /// refuse it, so it is never signed (ADR 0075 decision 6).
    #[error("a voucher for {amount} does not exceed the {signed} already signed")]
    VoucherNotAdvancing { amount: u128, signed: u128 },

    /// A voucher this node would sign above what its channel still backs:
    /// the deposit, less any withdrawal it has begun. The receiver would
    /// refuse it as unbacked, so it is never signed.
    #[error("a voucher for {amount} exceeds the {backed} the channel backs")]
    VoucherUnbacked { amount: u128, backed: u128 },

    /// Finishing a withdrawal that was never started, or has already
    /// finished.
    #[error("batch-settlement channel '{0}' has no withdrawal to finish")]
    NoWithdrawalPending(ChannelId),

    /// Finishing a withdrawal before the chain lets it finish: the channel's
    /// delay has not run, and on Solana the receiver has not sealed it
    /// either. Retryable once `remaining_secs` have passed.
    #[error(
        "the withdrawal from batch-settlement channel '{channel}' is due in {remaining_secs}s"
    )]
    WithdrawalNotDue {
        channel: ChannelId,
        remaining_secs: u64,
    },

    #[error("batch-settlement backend error: {0}")]
    Backend(String),
}

/// The receiving half of the settlement port for x402 `batch-settlement`
/// channels (ADR 0074 decision 9, ADR 0075 decision 2): this node admits a
/// channel a payer opened toward it, reads it, and lands the payer's
/// vouchers on it. [`BatchSettlementPayer`] is the other half, this node as
/// the payer.
///
/// It is **not** [`SettlementBackend`](crate::SettlementBackend) bent to fit.
/// A batch-settlement channel moves value one way and has a different
/// lifecycle on each chain, so `own_deposited`, `fund` and `close` would
/// mean nothing here. `SettlementBackend` is deleted once nothing calls it
/// (ADR 0075 decision 2, issue #1385).
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

    /// Bring back a channel this node **already holds vouchers on** -- one
    /// admitted before a restart, whose presentation the client edge's
    /// journal kept -- so [`channel_state`](Self::channel_state) and
    /// [`land`](Self::land) work on it again, without judging it against
    /// the admission rules.
    ///
    /// Admission policy decides which channels this node takes **new**
    /// vouchers on. A voucher already accepted was paid for, and landing it
    /// is what protects that value (ADR 0074 decision 5), so a policy
    /// tightened across a restart -- a raised minimum delay -- must not
    /// strand it. What is still checked is what makes landing safe at all:
    /// the presentation is for this backend's chain
    /// ([`WrongChain`](BatchSettlementError::WrongChain)), it names itself
    /// consistently ([`ChannelIdMismatch`](BatchSettlementError::ChannelIdMismatch)),
    /// and the chain can be read and holds the channel
    /// ([`ChannelNotFound`](BatchSettlementError::ChannelNotFound)).
    ///
    /// Restoring is not admitting. Whether a new voucher may be accepted on
    /// the channel is still [`admit`](Self::admit)'s to answer, now, under
    /// this node's current rules; a caller that takes new vouchers asks it.
    /// Restoring a channel already admitted or restored changes nothing but
    /// returns its current state.
    async fn restore(
        &self,
        presentation: ChannelPresentation,
    ) -> Result<BatchChannelState, BatchSettlementError>;

    /// The admitted or restored channel's state, read from the chain now.
    /// Never answered
    /// from a cache: collateral can fall on EVM, and a watcher that has just
    /// seen `WithdrawInitiated` or a Solana close relies on this to see it.
    async fn channel_state(
        &self,
        channel: &ChannelId,
    ) -> Result<BatchChannelState, BatchSettlementError>;

    /// Land `voucher` on the admitted or restored `channel`, so its amount
    /// is recorded
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

/// The paying half of the settlement port (ADR 0075 decision 2): this node
/// as the **payer** on a batch-settlement channel toward a counterparty,
/// where [`BatchSettlementBackend`] is this node as the receiver. Each
/// chain's backend implements both halves (issues #1374, #1375), over its
/// table's one `RpcTransport` and its settlement key's one nonce sequence;
/// [`InMemoryBatchSettlement`](super::InMemoryBatchSettlement) is the fake
/// that implements both, and [`super::contract`] holds each half to its
/// suite (ADR 0007).
///
/// **Which key.** The chain's settlement key pays, signs every transaction
/// here, and signs every voucher: on EVM `payerAuthorizer == payer`, on
/// Solana `authorized_signer` is the payer. `[signer]` is identity only and
/// never appears (ADR 0075 decision 3).
///
/// **What is not here.** Journaling an outbound channel's config before its
/// opening transaction is sent, and bringing an outbound channel back after
/// a restart with its watermark from the receiver's `POST /ilp/claim-state`,
/// belong to the operator surface and the peering (issues #1376, #1378):
/// this half remembers what it opened, and what it signed, for the process
/// lifetime.
#[async_trait]
pub trait BatchSettlementPayer: Send + Sync {
    /// Open a channel toward the counterparty that published `terms`, and
    /// deposit `deposit` into it, paying from this node's settlement
    /// account. The channel names this node's settlement key as payer and
    /// voucher signer, the counterparty in every seat its receiving half
    /// requires, the shared token, the counterparty's minimum delay and a
    /// fresh salt, so the counterparty admits it.
    ///
    /// On EVM a `deposit` this node sends and pays gas for; on Solana an
    /// `open` this node signs and posts to the counterparty's sponsor
    /// endpoint, which co-signs and submits it.
    ///
    /// Refuses, opening nothing, terms for another chain
    /// ([`WrongChain`](BatchSettlementError::WrongChain)), terms in a token
    /// this node does not settle in
    /// ([`TokenNotShared`](BatchSettlementError::TokenNotShared)), and an
    /// open the counterparty declines
    /// ([`OpenRefused`](BatchSettlementError::OpenRefused)).
    ///
    /// Never idempotent: two opens on the same terms are two channels, both
    /// live, as several channels to one receiver may be.
    async fn open(
        &self,
        terms: ReceiverTerms,
        deposit: u128,
    ) -> Result<OpenedChannel, BatchSettlementError>;

    /// Add `increment` to the deposit of a channel this node opened: EVM
    /// another `deposit` into the same config, Solana `top_up`. An
    /// increment, never a total. Returns the state after. Refuses a sealed
    /// channel ([`ChannelSealed`](BatchSettlementError::ChannelSealed)).
    async fn top_up(
        &self,
        channel: &ChannelId,
        increment: u128,
    ) -> Result<OutboundChannelState, BatchSettlementError>;

    /// Sign a voucher for `cumulative_amount` on a channel this node opened,
    /// with the key the chain records as its voucher signer, and raise this
    /// node's watermark on the channel to it. EVM: EIP-712
    /// `Voucher(channelId, maxClaimableAmount)` under the
    /// `x402BatchSettlement` domain; Solana: the 50-byte message with
    /// `expires_at = 0`.
    ///
    /// Refuses, signing nothing, an amount that does not exceed the highest
    /// already signed ([`VoucherNotAdvancing`](BatchSettlementError::VoucherNotAdvancing))
    /// and one above what the channel backs
    /// ([`VoucherUnbacked`](BatchSettlementError::VoucherUnbacked)): the
    /// receiver would refuse either. What the channel backs moves only by
    /// this node's own top-ups and withdrawals, so an implementation may
    /// answer from its own record rather than the chain.
    async fn sign_voucher(
        &self,
        channel: &ChannelId,
        cumulative_amount: u128,
    ) -> Result<Voucher, BatchSettlementError>;

    /// Begin taking back everything on a channel this node opened that the
    /// receiver has not landed: EVM `initiateWithdraw` for `balance −
    /// totalClaimed`, Solana `request_close`. From here the channel backs
    /// no new voucher, and the receiver still has the delay in which to
    /// land the latest one it holds. Returns the state after.
    ///
    /// Starting a withdrawal already started changes nothing, and returns
    /// the state. A sealed channel has nothing left to withdraw
    /// ([`ChannelSealed`](BatchSettlementError::ChannelSealed)).
    async fn start_withdrawal(
        &self,
        channel: &ChannelId,
    ) -> Result<OutboundChannelState, BatchSettlementError>;

    /// Finish a withdrawal this node started, returning to its settlement
    /// account everything the receiver had not landed by then: EVM
    /// `finalizeWithdraw`, Solana `distribute`. A voucher the receiver
    /// landed inside the delay is the receiver's, whatever the withdrawal
    /// asked for. Returns the state after.
    ///
    /// On Solana `distribute` is due once the receiver has sealed the
    /// channel or the grace period has run. The `reclaim` that follows it
    /// returns rent to the channel's `rent_payer`, which is the receiver's
    /// sponsor key, so it is the receiver's sweep, not this node's
    /// (ADR 0074 decision 5).
    ///
    /// Refuses a withdrawal never started
    /// ([`NoWithdrawalPending`](BatchSettlementError::NoWithdrawalPending))
    /// and one the chain does not yet let finish
    /// ([`WithdrawalNotDue`](BatchSettlementError::WithdrawalNotDue)).
    async fn finish_withdrawal(
        &self,
        channel: &ChannelId,
    ) -> Result<OutboundChannelState, BatchSettlementError>;

    /// A channel this node opened, read from the chain now, with the
    /// highest amount it has signed on it.
    async fn outbound_state(
        &self,
        channel: &ChannelId,
    ) -> Result<OutboundChannelState, BatchSettlementError>;
}
