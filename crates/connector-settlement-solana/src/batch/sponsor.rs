//! The Solana sponsor (ADR 0074 decisions 5 and 9, issue #1346): this node
//! co-signs a client-built `payment-channels` `open` as transaction fee
//! payer and as `rent_payer`, submits it, and admits the channel it made.
//!
//! **A fee payer that signs arbitrary transactions is a wallet drain.** The
//! sponsor key is this node's `[settlement.solana]` settlement key, and the
//! endpoint in front of this is public (ADR 0052: a buyer this node has never
//! heard of must be able to reach it). So the sponsor signs exactly one
//! shape of transaction -- x402's SVM batch-settlement acceptance policy
//! for a client-supplied `open` (X402 SVM spec `#L1008-L1150`, pinned at
//! x402 `0cb1a1f0`) -- and refuses everything else **by name**, before
//! anything is signed:
//!
//! - an optional Compute Budget prefix (at most one `SetComputeUnitLimit`,
//!   then at most one `SetComputeUnitPrice`, each under this node's cap),
//!   exactly one canonical `open` of the configured program, and at most one
//!   account-less Memo -- no other instruction, no other program, no address
//!   lookup table;
//! - the required signers are exactly the fee payer (this node) and the
//!   open's `payer`, whose signature must already verify;
//! - the sponsor key appears in the open's `rent_payer` and `payee` slots
//!   and as fee payer, **nowhere else** -- not in another instruction, not as
//!   a program, not as `payer` or `authorized_signer`;
//! - no account is writable but the five the open writes;
//! - every field decision 2 fixes, the published minimums, and every
//!   account the open names is the one [`OpenChannel::instruction`] would
//!   put there.
//!
//! What the static rules cannot see, the chain is asked before signing is
//! used: that the node's receiving account and the payer's canonical ATA
//! exist and are usable (Cantina 3.1.4: an unusable one forfeits to the
//! program's treasury), and a simulation of the exact co-signed transaction
//! (X402 SVM spec `#L1135-L1142`). Only then is it sent.
//!
//! **Submitted, not returned.** x402's facilitator validates, co-signs *and
//! broadcasts* a client's `open` (X402 SVM spec `#L379`, `deposit.transaction`:
//! "for the facilitator to validate, co-sign, and broadcast"), and a stock
//! client's `buildOpenPaymentChannelTransaction` hands its bytes to that
//! facilitator already payer-signed, leaving only the fee-payer slot empty.
//! Handing the co-signed bytes back instead would also hand the client the
//! choice of when (and whether) this node's rent is spent, and leave this
//! node unable to re-read and admit the channel it paid for. So the sponsor
//! submits, waits for the outcome, and admits the channel -- the facilitator's
//! own "re-read the channel and verify" step (`#L1143-L1146`).
//!
//! **Rent on a pre-SIMD-0194 cluster is the client's problem, not this
//! node's.** `payment-channels` computes the channel's rent ignoring
//! `exemption_threshold` (see [`crate::test_support::BatchPayer::open`]), so
//! on a cluster whose Rent sysvar still carries a threshold of 2 -- the
//! v2.1.21 `solana-test-validator` the Rust Workspace Gate pins, and no
//! public cluster -- an `open` alone leaves the channel short and the
//! simulation fails. The sponsor could top the channel up in a transaction
//! of its own first, but that transaction is not atomic with the client's
//! `open`: a client could have the node prefund a PDA and then make its
//! `open` fail, stranding the lamports at an address nobody can sign for.
//! That is a drain with no bound. Nor can it top the channel up *inside*
//! the client's transaction: the payer signed the message before this node
//! saw it, so any added instruction voids that signature, and a transfer
//! out of the sponsor key is exactly what [`SponsorRefusal::SponsorMisused`]
//! forbids. So before signing, the sponsor reads the cluster's Rent sysvar
//! (once per process) and, where its threshold is not 1, the channel's
//! balance, and refuses an `open` that would come up short as
//! [`SponsorRefusal::ClusterRentThresholdUnsupported`] -- by name, rather
//! than leaving the client to read a simulator log (issue #1356). A channel
//! already holding the cluster's real minimum opens even there, because the
//! program tops up only a shortfall; a test on such a validator prefunds
//! the channel from a key of its own. Sponsored opens are supported on
//! clusters whose threshold is 1: mainnet-beta, devnet and v3+ validators.

use std::collections::HashSet;
use std::str::FromStr;

use base64::Engine as _;
use bincode::Options as _;
use connector_chain_rpc::retry_read;
use connector_settlement::batch::{BatchSettlementBackend, ChannelPresentation};
use connector_settlement::ChannelId;
use solana_rpc_client_api::config::RpcSimulateTransactionConfig;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::instruction::{CompiledInstruction, Instruction};
use solana_sdk::message::{MessageHeader, VersionedMessage};
use solana_sdk::program_pack::Pack;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::rent::Rent;
use solana_sdk::signature::{Signature, Signer};
use solana_sdk::transaction::VersionedTransaction;
use solana_transaction_status_client_types::UiTransactionEncoding;

use super::wire::{self, DistributionEntry, OpenChannel};
use super::SolanaBatchSettlement;
use crate::submit::send_and_confirm;

/// The largest transaction a Solana node accepts: an IPv6 MTU less its
/// headers (`solana-packet`'s `PACKET_DATA_SIZE`, which `solana-sdk`
/// re-exports only under a deprecation).
pub const MAX_TRANSACTION_BYTES: usize = 1280 - 40 - 8;

/// The highest `SetComputeUnitLimit` signed for: x402's own ceiling (X402
/// SVM spec `#L1064`). An observed `open` uses about 51,000.
pub const MAX_COMPUTE_UNIT_LIMIT: u32 = 400_000;

/// The highest `SetComputeUnitPrice` signed for, in microlamports per
/// compute unit. **Fifty times stricter than x402's 5,000,000** (`#L1065`),
/// which the spec permits (`#L1068`). The priority fee is this node's, and
/// it is spent even by an `open` that fails on chain -- which a client can
/// arrange after the simulation passes, by moving its tokens away first, for
/// the price of its own 5,000-lamport transfer. At the limit above, x402's
/// cap would let each such attempt cost this node 2,000,000 lamports; this
/// bounds it at 40,000. A stock client's default is 1.
pub const MAX_COMPUTE_UNIT_PRICE_MICROLAMPORTS: u64 = 100_000;

/// The longest Memo signed for, in bytes: x402's own `MAX_MEMO_BYTES`. A
/// stock client's uniqueness memo is 32 hex characters.
pub const MAX_MEMO_BYTES: usize = 256;

/// SPL Memo v2, the one program allowed after the `open` (X402 SVM spec
/// `#L1047-L1049`).
pub const MEMO_PROGRAM_ID: &str = "MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr";

/// `ComputeBudgetInstruction` discriminators the prefix may carry.
const SET_COMPUTE_UNIT_LIMIT: u8 = 2;
const SET_COMPUTE_UNIT_PRICE: u8 = 3;

/// The `open`'s fourteen account roles, in the program's order (PC
/// `instructions/open.rs`; X402 SVM spec `#L1083-L1098`).
const OPEN_ROLES: [&str; 14] = [
    "payer",
    "rent_payer",
    "payee",
    "mint",
    "authorized_signer",
    "channel",
    "payer_token_account",
    "channel_token_account",
    "token_program",
    "system_program",
    "rent",
    "associated_token_program",
    "event_authority",
    "self_program",
];
const PAYER: usize = 0;
const RENT_PAYER: usize = 1;
const PAYEE: usize = 2;
const MINT: usize = 3;
const AUTHORIZED_SIGNER: usize = 4;
const CHANNEL: usize = 5;
const PAYER_TOKEN_ACCOUNT: usize = 6;
const CHANNEL_TOKEN_ACCOUNT: usize = 7;
const TOKEN_PROGRAM: usize = 8;

/// The `open`'s fixed data: discriminator, salt, deposit, grace period,
/// open slot and the distribution's entry count.
const OPEN_HEADER_LEN: usize = 1 + 8 + 8 + 4 + 8 + 4;
const DISTRIBUTION_ENTRY_LEN: usize = 32 + 2;

/// What the sponsor will co-sign for: this node's facts and published
/// minimums. The same facts [`SolanaBatchSettlement`] admits channels by,
/// plus the minimum sponsored deposit, which bounds only this surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SponsorTerms {
    /// The configured `payment-channels` program.
    pub program_id: Pubkey,
    /// Fee payer, `rent_payer` and `payee`: the settlement key.
    pub sponsor: Pubkey,
    /// The one distribution recipient, at 10000 bps.
    pub receiver: Pubkey,
    /// The mint this node settles in.
    pub mint: Pubkey,
    pub min_grace_period_secs: u64,
    /// `[settlement.solana.batch_settlement] min_sponsored_deposit`.
    pub min_sponsored_deposit: u64,
}

/// Why the sponsor would not co-sign, or could not finish. Each has a
/// [`name`](Self::name), which is what the endpoint answers with, and a
/// [`class`](Self::class), which decides its HTTP status.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SponsorRefusal {
    #[error("the transaction is not standard base64: {0}")]
    NotBase64(String),
    #[error(
        "the transaction is {len} bytes; a Solana transaction is at most {MAX_TRANSACTION_BYTES}"
    )]
    TooLarge { len: usize },
    #[error("the transaction does not decode as a Solana transaction: {0}")]
    Malformed(String),
    #[error("the transaction uses an address lookup table; an open fits in static keys")]
    AddressLookupTables,
    #[error("the transaction's fee payer is {found}, not this node's sponsor key {sponsor}")]
    FeePayerNotSponsor { found: Pubkey, sponsor: Pubkey },
    #[error("instruction {index}: {detail}")]
    UnexpectedInstruction { index: usize, detail: String },
    #[error("compute budget instruction {index}: {detail}")]
    ComputeBudgetRefused { index: usize, detail: String },
    #[error("memo instruction {index}: {detail}")]
    MemoRefused { index: usize, detail: String },
    #[error("the open instruction is malformed: {0}")]
    OpenMalformed(String),
    #[error("the sponsor key may appear only as fee payer, rent_payer and payee: {0}")]
    SponsorMisused(String),
    #[error(
        "the required signers must be exactly this node's sponsor key (fee payer) and the open's \
         payer, both writable: {0}"
    )]
    UnexpectedSigners(String),
    #[error("the payer's signature is missing or does not verify over the message")]
    PayerSignatureInvalid,
    #[error(
        "payee is {found}; it must be this node's sponsor key {sponsor} (ADR 0074 decision 5)"
    )]
    PayeeNotSponsor { found: Pubkey, sponsor: Pubkey },
    #[error(
        "rent_payer is {found}; it must be this node's sponsor key {sponsor} (ADR 0074 decision 5)"
    )]
    RentPayerNotSponsor { found: Pubkey, sponsor: Pubkey },
    #[error("mint is {found}; this node settles in {expected}")]
    MintNotSettled { found: Pubkey, expected: Pubkey },
    #[error(
        "the distribution must be exactly one entry, this node's receiver {receiver} at 10000 bps; \
         found {found:?}"
    )]
    DistributionNotSoleReceiver {
        found: Vec<(Pubkey, u16)>,
        receiver: Pubkey,
    },
    #[error("grace_period is {grace_period}s; this node's minimum is {minimum}s")]
    GracePeriodBelowMinimum { grace_period: u64, minimum: u64 },
    #[error(
        "deposit is {deposit}; this node sponsors an open only at or above {minimum}, which bounds \
         the rent it floats"
    )]
    DepositBelowMinimum { deposit: u64, minimum: u64 },
    #[error("token_program is {found}; this node sponsors only SPL Token ({expected}) mints")]
    TokenProgramUnsupported { found: Pubkey, expected: Pubkey },
    #[error("{role} is {found}; the canonical open names {expected} there")]
    OpenAccountMismatch {
        role: &'static str,
        found: Pubkey,
        expected: Pubkey,
    },
    #[error("{0} is writable, and is none of the five accounts an open writes")]
    UnexpectedWritable(Pubkey),
    #[error(
        "this node's receiving account {account} is unusable: {reason}. A payout to it would \
         forfeit to the program's treasury (Cantina 3.1.4)"
    )]
    ReceivingAccountUnusable { account: Pubkey, reason: String },
    #[error(
        "the payer's token account {account} is unusable: {reason}. A refund to it would forfeit \
         to the program's treasury (Cantina 3.1.4)"
    )]
    PayerTokenAccountUnusable { account: Pubkey, reason: String },
    #[error(
        "this cluster's rent exemption_threshold is {threshold}, not 1, and the channel {channel} \
         holds {lamports} lamports, short of the cluster's rent-exempt minimum of {minimum}. \
         payment-channels computes rent as if the threshold were 1, so the open would fail \
         simulation; this node sponsors an open on such a cluster only for a channel that already \
         holds its rent"
    )]
    ClusterRentThresholdUnsupported {
        /// The cluster's Rent sysvar `exemption_threshold`, as it prints:
        /// an `f64`, which this enum's `Eq` cannot hold.
        threshold: String,
        channel: Pubkey,
        lamports: u64,
        minimum: u64,
    },
    #[error("the co-signed transaction failed simulation: {0}")]
    SimulationFailed(String),
    #[error("the co-signed open was submitted and did not land: {0}")]
    SubmissionFailed(String),
    #[error("the chain could not be read: {0}")]
    ChainUnavailable(String),
    #[error("the open landed, but the channel it made is not one this node admits: {0}")]
    NotAdmitted(String),
}

/// How a [`SponsorRefusal`] reads to the endpoint's caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefusalClass {
    /// The request is not a transaction at all.
    Malformed,
    /// A transaction the sponsor will not sign. Nothing was signed or sent.
    Refused,
    /// The chain could not be read; nothing was sent, and a retry may work.
    Unavailable,
    /// The transaction was signed and sent, and did not produce a channel
    /// this node admits.
    Failed,
}

impl SponsorRefusal {
    /// The refusal's name on the wire.
    pub fn name(&self) -> &'static str {
        match self {
            SponsorRefusal::NotBase64(_) => "transaction_not_base64",
            SponsorRefusal::TooLarge { .. } => "transaction_too_large",
            SponsorRefusal::Malformed(_) => "transaction_malformed",
            SponsorRefusal::AddressLookupTables => "address_lookup_tables_refused",
            SponsorRefusal::FeePayerNotSponsor { .. } => "fee_payer_not_sponsor",
            SponsorRefusal::UnexpectedInstruction { .. } => "unexpected_instruction",
            SponsorRefusal::ComputeBudgetRefused { .. } => "compute_budget_refused",
            SponsorRefusal::MemoRefused { .. } => "memo_refused",
            SponsorRefusal::OpenMalformed(_) => "open_malformed",
            SponsorRefusal::SponsorMisused(_) => "sponsor_misused",
            SponsorRefusal::UnexpectedSigners(_) => "unexpected_signers",
            SponsorRefusal::PayerSignatureInvalid => "payer_signature_invalid",
            SponsorRefusal::PayeeNotSponsor { .. } => "payee_not_sponsor",
            SponsorRefusal::RentPayerNotSponsor { .. } => "rent_payer_not_sponsor",
            SponsorRefusal::MintNotSettled { .. } => "mint_not_settled",
            SponsorRefusal::DistributionNotSoleReceiver { .. } => "distribution_not_sole_receiver",
            SponsorRefusal::GracePeriodBelowMinimum { .. } => "grace_period_below_minimum",
            SponsorRefusal::DepositBelowMinimum { .. } => "deposit_below_minimum",
            SponsorRefusal::TokenProgramUnsupported { .. } => "token_program_unsupported",
            SponsorRefusal::OpenAccountMismatch { .. } => "open_account_mismatch",
            SponsorRefusal::UnexpectedWritable(_) => "unexpected_writable_account",
            SponsorRefusal::ReceivingAccountUnusable { .. } => "receiving_account_unusable",
            SponsorRefusal::PayerTokenAccountUnusable { .. } => "payer_token_account_unusable",
            SponsorRefusal::ClusterRentThresholdUnsupported { .. } => {
                "cluster_rent_threshold_unsupported"
            }
            SponsorRefusal::SimulationFailed(_) => "simulation_failed",
            SponsorRefusal::SubmissionFailed(_) => "submission_failed",
            SponsorRefusal::ChainUnavailable(_) => "chain_unavailable",
            SponsorRefusal::NotAdmitted(_) => "not_admitted",
        }
    }

    pub fn class(&self) -> RefusalClass {
        match self {
            SponsorRefusal::NotBase64(_)
            | SponsorRefusal::TooLarge { .. }
            | SponsorRefusal::Malformed(_) => RefusalClass::Malformed,
            SponsorRefusal::ChainUnavailable(_) => RefusalClass::Unavailable,
            SponsorRefusal::SubmissionFailed(_) | SponsorRefusal::NotAdmitted(_) => {
                RefusalClass::Failed
            }
            _ => RefusalClass::Refused,
        }
    }
}

/// A client's `open` that passed every static rule: the transaction as the
/// client signed it, and the `open` it carries, decoded.
#[derive(Debug, Clone)]
pub struct VettedOpen {
    pub transaction: VersionedTransaction,
    pub open: OpenChannel,
    /// The channel account the `open` creates.
    pub channel: Pubkey,
}

/// What a sponsored `open` produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SponsoredOpen {
    /// The channel account, base58 on the wire: the id every voucher names.
    pub channel: Pubkey,
    /// The confirmed transaction.
    pub signature: Signature,
    pub payer: Pubkey,
    pub deposit: u64,
}

/// Judge `transaction_base64` -- a client's payer-signed `open` -- against
/// every rule the sponsor can check without the chain. Pure: no I/O, no
/// clock, no key. `Ok` is not permission to sign on its own: the chain
/// checks in [`SolanaBatchSettlement::sponsor_vetted`] still follow.
pub fn vet_open(
    transaction_base64: &str,
    terms: &SponsorTerms,
) -> Result<VettedOpen, SponsorRefusal> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(transaction_base64.trim())
        .map_err(|error| SponsorRefusal::NotBase64(error.to_string()))?;
    if bytes.len() > MAX_TRANSACTION_BYTES {
        return Err(SponsorRefusal::TooLarge { len: bytes.len() });
    }
    let transaction: VersionedTransaction = bincode::options()
        .with_limit(MAX_TRANSACTION_BYTES as u64)
        .with_fixint_encoding()
        .reject_trailing_bytes()
        .deserialize(&bytes)
        .map_err(|error| SponsorRefusal::Malformed(error.to_string()))?;
    transaction
        .sanitize()
        .map_err(|error| SponsorRefusal::Malformed(error.to_string()))?;
    let message = &transaction.message;
    let keys = message.static_account_keys();
    if keys.iter().collect::<HashSet<_>>().len() != keys.len() {
        return Err(SponsorRefusal::Malformed(
            "an account key is listed twice".to_string(),
        ));
    }
    if message
        .address_table_lookups()
        .is_some_and(|lookups| !lookups.is_empty())
    {
        return Err(SponsorRefusal::AddressLookupTables);
    }
    // `sanitize` guarantees at least one signer, so key 0 exists.
    if keys[0] != terms.sponsor {
        return Err(SponsorRefusal::FeePayerNotSponsor {
            found: keys[0],
            sponsor: terms.sponsor,
        });
    }

    let open_index = vet_layout(message, terms)?;
    let open_instruction = &message.instructions()[open_index];
    let open = decode_open(open_instruction, keys)?;
    vet_sponsor_isolation(message, open_index, &open, terms)?;
    vet_signers(message, &open)?;
    vet_terms(&open, terms)?;
    let canonical = vet_accounts(open_instruction, keys, &open, terms)?;
    vet_writable(message, &canonical)?;
    let channel = canonical.accounts[CHANNEL].pubkey;

    // The payer is key 1: `vet_signers` put it there.
    let message_bytes = message.serialize();
    if !transaction.signatures[1].verify(open.payer.as_ref(), &message_bytes) {
        return Err(SponsorRefusal::PayerSignatureInvalid);
    }

    Ok(VettedOpen {
        transaction,
        open,
        channel,
    })
}

/// The top-level layout (X402 SVM spec `#L1037-L1073`): an optional Compute
/// Budget prefix, exactly one `open` of the configured program, then at most
/// one Memo. Returns the `open`'s index. Lighthouse assertions, which x402
/// lets a sponsor refuse, are refused: they are nothing an `open` needs.
fn vet_layout(message: &VersionedMessage, terms: &SponsorTerms) -> Result<usize, SponsorRefusal> {
    let keys = message.static_account_keys();
    let instructions = message.instructions();
    let compute_budget = solana_sdk::compute_budget::id();
    let memo = Pubkey::from_str(MEMO_PROGRAM_ID).expect("a base58 constant");
    let program_of =
        |instruction: &CompiledInstruction| keys[instruction.program_id_index as usize];

    let mut index = 0;
    let mut seen_limit = false;
    let mut seen_price = false;
    while let Some(instruction) = instructions.get(index) {
        if program_of(instruction) != compute_budget {
            break;
        }
        let refuse = |detail: String| SponsorRefusal::ComputeBudgetRefused { index, detail };
        if !instruction.accounts.is_empty() {
            return Err(refuse("names accounts".to_string()));
        }
        match instruction.data.first() {
            Some(&SET_COMPUTE_UNIT_LIMIT) => {
                if seen_limit || seen_price {
                    return Err(refuse(
                        "at most one SetComputeUnitLimit, before any SetComputeUnitPrice"
                            .to_string(),
                    ));
                }
                let units = u32::from_le_bytes(
                    exact::<4>(&instruction.data)
                        .ok_or_else(|| refuse("SetComputeUnitLimit is 5 bytes".to_string()))?,
                );
                if units > MAX_COMPUTE_UNIT_LIMIT {
                    return Err(refuse(format!(
                        "SetComputeUnitLimit {units} is above {MAX_COMPUTE_UNIT_LIMIT}"
                    )));
                }
                seen_limit = true;
            }
            Some(&SET_COMPUTE_UNIT_PRICE) => {
                if seen_price {
                    return Err(refuse("at most one SetComputeUnitPrice".to_string()));
                }
                let price = u64::from_le_bytes(
                    exact::<8>(&instruction.data)
                        .ok_or_else(|| refuse("SetComputeUnitPrice is 9 bytes".to_string()))?,
                );
                if price > MAX_COMPUTE_UNIT_PRICE_MICROLAMPORTS {
                    return Err(refuse(format!(
                        "SetComputeUnitPrice {price} microlamports is above \
                         {MAX_COMPUTE_UNIT_PRICE_MICROLAMPORTS}"
                    )));
                }
                seen_price = true;
            }
            _ => {
                return Err(refuse(
                    "only SetComputeUnitLimit and SetComputeUnitPrice are signed for".to_string(),
                ))
            }
        }
        index += 1;
    }

    let open_index = index;
    match instructions.get(open_index) {
        Some(instruction)
            if program_of(instruction) == terms.program_id
                && instruction.data.first() == Some(&wire::OPEN) => {}
        Some(instruction) => {
            return Err(SponsorRefusal::UnexpectedInstruction {
                index: open_index,
                detail: format!(
                    "expected payment-channels ({}) `open` after the compute budget prefix, found \
                     a call to {}",
                    terms.program_id,
                    program_of(instruction)
                ),
            })
        }
        None => {
            return Err(SponsorRefusal::UnexpectedInstruction {
                index: open_index,
                detail: "the transaction carries no payment-channels `open`".to_string(),
            })
        }
    }

    let mut memos = 0;
    for (index, instruction) in instructions.iter().enumerate().skip(open_index + 1) {
        if program_of(instruction) != memo {
            return Err(SponsorRefusal::UnexpectedInstruction {
                index,
                detail: format!(
                    "only one Memo may follow the `open`; found a call to {}",
                    program_of(instruction)
                ),
            });
        }
        let refuse = |detail: &str| SponsorRefusal::MemoRefused {
            index,
            detail: detail.to_string(),
        };
        memos += 1;
        if memos > 1 {
            return Err(refuse("at most one Memo"));
        }
        if !instruction.accounts.is_empty() {
            return Err(refuse("a Memo here names no account"));
        }
        if instruction.data.len() > MAX_MEMO_BYTES {
            return Err(refuse("longer than 256 bytes"));
        }
        if std::str::from_utf8(&instruction.data).is_err() {
            // The Memo program fails on anything else, and the fee is still
            // this node's.
            return Err(refuse("not UTF-8"));
        }
    }
    Ok(open_index)
}

/// `data[1..]` as exactly `N` bytes: a Compute Budget argument.
fn exact<const N: usize>(data: &[u8]) -> Option<[u8; N]> {
    (data.len() == N + 1).then(|| data[1..].try_into().expect("N bytes"))
}

/// Decode the `open`: fourteen accounts, and data of exactly the length its
/// own entry count implies -- nothing truncated, nothing trailing.
fn decode_open(
    instruction: &CompiledInstruction,
    keys: &[Pubkey],
) -> Result<OpenChannel, SponsorRefusal> {
    if instruction.accounts.len() != OPEN_ROLES.len() {
        return Err(SponsorRefusal::OpenMalformed(format!(
            "{} accounts; an open names exactly {}",
            instruction.accounts.len(),
            OPEN_ROLES.len()
        )));
    }
    let data = &instruction.data;
    if data.len() < OPEN_HEADER_LEN {
        return Err(SponsorRefusal::OpenMalformed(format!(
            "{} bytes of data; the header alone is {OPEN_HEADER_LEN}",
            data.len()
        )));
    }
    let u64_at = |offset: usize| {
        u64::from_le_bytes(data[offset..offset + 8].try_into().expect("eight bytes"))
    };
    let u32_at = |offset: usize| {
        u32::from_le_bytes(data[offset..offset + 4].try_into().expect("four bytes"))
    };
    let count = u32_at(29) as usize;
    let expected_len = count
        .checked_mul(DISTRIBUTION_ENTRY_LEN)
        .and_then(|entries| entries.checked_add(OPEN_HEADER_LEN));
    if expected_len != Some(data.len()) {
        return Err(SponsorRefusal::OpenMalformed(format!(
            "{} bytes of data for {count} distribution entries",
            data.len()
        )));
    }
    let recipients = data[OPEN_HEADER_LEN..]
        .as_chunks::<DISTRIBUTION_ENTRY_LEN>()
        .0
        .iter()
        .map(|entry| DistributionEntry {
            recipient: Pubkey::new_from_array(entry[..32].try_into().expect("32 bytes")),
            bps: u16::from_le_bytes([entry[32], entry[33]]),
        })
        .collect();
    let account = |slot: usize| keys[instruction.accounts[slot] as usize];
    Ok(OpenChannel {
        payer: account(PAYER),
        rent_payer: account(RENT_PAYER),
        payee: account(PAYEE),
        mint: account(MINT),
        token_program: account(TOKEN_PROGRAM),
        authorized_signer: account(AUTHORIZED_SIGNER),
        salt: u64_at(1),
        deposit: u64_at(9),
        grace_period: u32_at(17),
        open_slot: u64_at(21),
        recipients,
    })
}

/// The sponsor key is fee payer, `rent_payer` and `payee`, and nothing else
/// (X402 SVM spec `#L1026-L1031`): not the payer, not the voucher signer,
/// not an account of any other instruction or of any other `open` slot, and
/// never a program.
fn vet_sponsor_isolation(
    message: &VersionedMessage,
    open_index: usize,
    open: &OpenChannel,
    terms: &SponsorTerms,
) -> Result<(), SponsorRefusal> {
    let misused = |detail: String| Err(SponsorRefusal::SponsorMisused(detail));
    if open.payer == terms.sponsor {
        return misused("it is the open's payer".to_string());
    }
    if open.authorized_signer == terms.sponsor {
        return misused("it is the open's authorized_signer".to_string());
    }
    // Key 0 is the sponsor: `vet_open` checked the fee payer first.
    for (index, instruction) in message.instructions().iter().enumerate() {
        if instruction.program_id_index == 0 {
            return misused(format!("instruction {index} invokes it as a program"));
        }
        for (slot, &account) in instruction.accounts.iter().enumerate() {
            let allowed = index == open_index && (slot == RENT_PAYER || slot == PAYEE);
            if account == 0 && !allowed {
                let role = if index == open_index {
                    OPEN_ROLES[slot].to_string()
                } else {
                    format!("account {slot} of instruction {index}")
                };
                return misused(format!("it is named as {role}"));
            }
        }
    }
    Ok(())
}

/// Exactly two signatures are required, both writable: key 0, the sponsor,
/// and key 1, the open's payer (X402 SVM spec `#L1023-L1025`). A third
/// signer would be a third party whose presence this node cannot account
/// for, and a read-only signer seat is not one an `open` has.
fn vet_signers(message: &VersionedMessage, open: &OpenChannel) -> Result<(), SponsorRefusal> {
    let header = message.header();
    let keys = message.static_account_keys();
    if header.num_required_signatures != 2 {
        return Err(SponsorRefusal::UnexpectedSigners(format!(
            "{} required signatures",
            header.num_required_signatures
        )));
    }
    if header.num_readonly_signed_accounts != 0 {
        return Err(SponsorRefusal::UnexpectedSigners(
            "a read-only signer".to_string(),
        ));
    }
    if keys[1] != open.payer {
        return Err(SponsorRefusal::UnexpectedSigners(format!(
            "{} signs, and is not the open's payer {}",
            keys[1], open.payer
        )));
    }
    Ok(())
}

/// Every field ADR 0074 decision 2 fixes, and this node's published
/// minimums, in the record's order.
fn vet_terms(open: &OpenChannel, terms: &SponsorTerms) -> Result<(), SponsorRefusal> {
    if open.payee != terms.sponsor {
        return Err(SponsorRefusal::PayeeNotSponsor {
            found: open.payee,
            sponsor: terms.sponsor,
        });
    }
    if open.rent_payer != terms.sponsor {
        return Err(SponsorRefusal::RentPayerNotSponsor {
            found: open.rent_payer,
            sponsor: terms.sponsor,
        });
    }
    if open.mint != terms.mint {
        return Err(SponsorRefusal::MintNotSettled {
            found: open.mint,
            expected: terms.mint,
        });
    }
    if open.recipients != wire::sole_recipient(&terms.receiver) {
        return Err(SponsorRefusal::DistributionNotSoleReceiver {
            found: open
                .recipients
                .iter()
                .map(|entry| (entry.recipient, entry.bps))
                .collect(),
            receiver: terms.receiver,
        });
    }
    let grace_period = u64::from(open.grace_period);
    if grace_period < terms.min_grace_period_secs {
        return Err(SponsorRefusal::GracePeriodBelowMinimum {
            grace_period,
            minimum: terms.min_grace_period_secs,
        });
    }
    if open.deposit < terms.min_sponsored_deposit {
        return Err(SponsorRefusal::DepositBelowMinimum {
            deposit: open.deposit,
            minimum: terms.min_sponsored_deposit,
        });
    }
    // The connected backend has already proved the configured mint is owned
    // by the SPL Token program, so the right token program is known without
    // a read. Token-2022 is refused: its account extensions (a required
    // incoming-transfer memo, a CPI guard) can make a payout fail, and a
    // failed payout forfeits.
    if open.token_program != spl_token::id() {
        return Err(SponsorRefusal::TokenProgramUnsupported {
            found: open.token_program,
            expected: spl_token::id(),
        });
    }
    Ok(())
}

/// Every account the `open` names is the one the canonical `open` for these
/// fields names -- the channel PDA, both ATAs, the programs, the sysvar and
/// the event authority. Returns that canonical `open`.
fn vet_accounts(
    instruction: &CompiledInstruction,
    keys: &[Pubkey],
    open: &OpenChannel,
    terms: &SponsorTerms,
) -> Result<Instruction, SponsorRefusal> {
    let canonical = open.instruction(&terms.program_id);
    for (slot, (meta, &index)) in canonical
        .accounts
        .iter()
        .zip(&instruction.accounts)
        .enumerate()
    {
        let found = keys[index as usize];
        if found != meta.pubkey {
            return Err(SponsorRefusal::OpenAccountMismatch {
                role: OPEN_ROLES[slot],
                found,
                expected: meta.pubkey,
            });
        }
    }
    Ok(canonical)
}

/// No account is writable but the five an `open` writes -- payer, rent
/// payer, channel, and the two token accounts -- and those five are (X402
/// SVM spec `#L1083-L1098`, `#L1128-L1133`). `canonical` is the `open`
/// [`vet_accounts`] matched the transaction's against.
fn vet_writable(message: &VersionedMessage, canonical: &Instruction) -> Result<(), SponsorRefusal> {
    let writes = [
        PAYER,
        RENT_PAYER,
        CHANNEL,
        PAYER_TOKEN_ACCOUNT,
        CHANNEL_TOKEN_ACCOUNT,
    ]
    .map(|slot| canonical.accounts[slot].pubkey);
    let keys = message.static_account_keys();
    for (index, key) in keys.iter().enumerate() {
        let writable = is_writable(message.header(), keys.len(), index);
        if writable && !writes.contains(key) {
            return Err(SponsorRefusal::UnexpectedWritable(*key));
        }
        if !writable && writes.contains(key) {
            return Err(SponsorRefusal::OpenMalformed(format!(
                "{key} is read-only, and the open writes it"
            )));
        }
    }
    Ok(())
}

/// Whether the message header marks static key `index` writable: the
/// header partitions the keys as writable signers, read-only signers,
/// writable non-signers, read-only non-signers.
fn is_writable(header: &MessageHeader, key_count: usize, index: usize) -> bool {
    let signers = usize::from(header.num_required_signatures);
    if index < signers {
        index < signers - usize::from(header.num_readonly_signed_accounts)
    } else {
        index < key_count - usize::from(header.num_readonly_unsigned_accounts)
    }
}

/// Why a token account the program will pay into is not usable, or `None`
/// if it is: it must exist, belong to the SPL Token program, be an
/// initialized account of `mint` owned by `owner`, and not be frozen.
pub fn token_account_problem(
    account: Option<&solana_sdk::account::Account>,
    owner: &Pubkey,
    mint: &Pubkey,
) -> Option<String> {
    let Some(account) = account else {
        return Some("it does not exist".to_string());
    };
    if account.owner != spl_token::id() {
        return Some(format!(
            "it is owned by {}, not the SPL Token program",
            account.owner
        ));
    }
    let Ok(token) = spl_token::state::Account::unpack(&account.data) else {
        return Some("it is not an initialized SPL token account".to_string());
    };
    if token.mint != *mint {
        return Some(format!("it holds {}, not {mint}", token.mint));
    }
    if token.owner != *owner {
        return Some(format!("it belongs to {}, not {owner}", token.owner));
    }
    if token.is_frozen() {
        return Some("it is frozen".to_string());
    }
    None
}

/// Whether an `open` of `channel`, which holds `lamports` now, can pay its
/// rent on a cluster whose Rent sysvar is `rent`.
///
/// `payment-channels` tops the channel up to `(128 + len) ×
/// lamports_per_byte_year` -- pinocchio 0.11's figure, which ignores
/// `exemption_threshold` -- and only by the shortfall against it. Where
/// SIMD-0194 has set the threshold to 1 that is the real minimum, whatever
/// the channel holds. Anywhere else it is short, and the `open` fails
/// unless the channel already holds the cluster's real minimum, in which
/// case the program tops up nothing (issue #1356).
pub fn vet_channel_rent(
    rent: &Rent,
    channel: &Pubkey,
    lamports: u64,
) -> Result<(), SponsorRefusal> {
    if threshold_is_one(rent) {
        return Ok(());
    }
    let minimum = rent.minimum_balance(wire::CHANNEL_ACCOUNT_LEN);
    if lamports >= minimum {
        return Ok(());
    }
    Err(SponsorRefusal::ClusterRentThresholdUnsupported {
        threshold: rent.exemption_threshold.to_string(),
        channel: *channel,
        lamports,
        minimum,
    })
}

/// SIMD-0194's threshold, the one `payment-channels`' rent figure assumes.
fn threshold_is_one(rent: &Rent) -> bool {
    rent.exemption_threshold == 1.0
}

impl SolanaBatchSettlement {
    /// The terms this backend's sponsor co-signs under: its own admission
    /// facts and its minimum sponsored deposit.
    pub fn sponsor_terms(&self) -> SponsorTerms {
        SponsorTerms {
            program_id: self.program_id,
            sponsor: self.sponsor(),
            receiver: self.receiver(),
            mint: self.mint,
            min_grace_period_secs: self.min_grace_period_secs,
            min_sponsored_deposit: self.min_sponsored_deposit,
        }
    }

    /// [`vet_open`] under this backend's [`sponsor_terms`](Self::sponsor_terms):
    /// the first, pure half of a sponsorship. Split from
    /// [`sponsor_vetted`](Self::sponsor_vetted) so the endpoint can hold a
    /// guard on the payer it names before any chain work starts.
    pub fn vet_sponsored_open(
        &self,
        transaction_base64: &str,
    ) -> Result<VettedOpen, SponsorRefusal> {
        vet_open(transaction_base64, &self.sponsor_terms())
    }

    /// Co-sign a [`vet_sponsored_open`](Self::vet_sponsored_open)-ed `open` as fee payer and `rent_payer`,
    /// submit it, and admit the channel it made. See this module's doc for
    /// every rule, and for why it submits rather than returning the bytes.
    ///
    /// Nothing is signed until both token accounts and the cluster's rent
    /// pass; nothing is sent until the co-signed bytes simulate cleanly.
    /// The account reads are at `processed`, the freshest state there is,
    /// so a client that has already moved its tokens away is caught here
    /// rather than on chain, where the failure would cost this node the fee.
    /// What a client does after the simulation is the endpoint's to bound
    /// (its failure budget).
    ///
    /// The simulation hands the co-signed bytes to the settlement RPC
    /// endpoint whether or not they are then sent, so that endpoint could
    /// land them. It can land only this vetted `open`, which is what the
    /// node was about to send anyway -- the same trust ADR 0073 already
    /// places in it.
    pub async fn sponsor_vetted(
        &self,
        vetted: VettedOpen,
    ) -> Result<SponsoredOpen, SponsorRefusal> {
        let VettedOpen {
            mut transaction,
            open,
            channel,
        } = vetted;

        self.vet_token_accounts(&open).await?;
        self.vet_cluster_rent(&channel).await?;

        transaction.signatures[0] = self.sponsor.sign_message(&transaction.message.serialize());
        let simulated = retry_read(|| {
            self.rpc.simulate_transaction_with_config(
                &transaction,
                RpcSimulateTransactionConfig {
                    sig_verify: true,
                    replace_recent_blockhash: false,
                    commitment: Some(CommitmentConfig::processed()),
                    encoding: Some(UiTransactionEncoding::Base64),
                    ..RpcSimulateTransactionConfig::default()
                },
            )
        })
        .await
        .map_err(|error| SponsorRefusal::ChainUnavailable(error.to_string()))?;
        if let Some(error) = simulated.value.err {
            let logs = simulated.value.logs.unwrap_or_default();
            let tail = logs[logs.len().saturating_sub(4)..].join(" | ");
            return Err(SponsorRefusal::SimulationFailed(format!("{error}: {tail}")));
        }

        // The client chose the blockhash, and it is no newer than the
        // latest: the latest's deadline is a bound on its own, so the
        // confirm loop never calls a transaction expired too early.
        let (_, last_valid_block_height) = retry_read(|| {
            self.rpc
                .get_latest_blockhash_with_commitment(CommitmentConfig::confirmed())
        })
        .await
        .map_err(|error| SponsorRefusal::ChainUnavailable(error.to_string()))?;
        let signature = send_and_confirm(
            &self.rpc,
            &transaction,
            last_valid_block_height,
            self.confirm,
        )
        .await
        .map_err(|error| SponsorRefusal::SubmissionFailed(error.to_string()))?;

        self.admit(ChannelPresentation::Solana {
            channel: ChannelId(channel.to_string()),
        })
        .await
        .map_err(|error| SponsorRefusal::NotAdmitted(error.to_string()))?;

        Ok(SponsoredOpen {
            channel,
            signature,
            payer: open.payer,
            deposit: open.deposit,
        })
    }

    /// [`vet_channel_rent`] against this cluster's Rent sysvar. The sysvar is
    /// read once per process (a cluster's threshold does not change under a
    /// running node), and the channel only where the threshold is not 1, so
    /// on mainnet-beta, devnet and a v3+ validator this costs no read at all
    /// after the first.
    async fn vet_cluster_rent(&self, channel: &Pubkey) -> Result<(), SponsorRefusal> {
        let rent = match self.cluster_rent.get() {
            Some(rent) => rent.clone(),
            None => {
                let sysvar = solana_sdk::sysvar::rent::id();
                let account = retry_read(|| self.rpc.get_account(&sysvar))
                    .await
                    .map_err(|error| SponsorRefusal::ChainUnavailable(error.to_string()))?;
                let rent: Rent = bincode::deserialize(&account.data).map_err(|error| {
                    SponsorRefusal::ChainUnavailable(format!(
                        "the Rent sysvar does not decode: {error}"
                    ))
                })?;
                // A racing request may have set it first, to the same value.
                let _ = self.cluster_rent.set(rent.clone());
                rent
            }
        };
        if threshold_is_one(&rent) {
            return Ok(());
        }
        let lamports = retry_read(|| {
            self.rpc
                .get_account_with_commitment(channel, CommitmentConfig::processed())
        })
        .await
        .map_err(|error| SponsorRefusal::ChainUnavailable(error.to_string()))?
        .value
        .map_or(0, |account| account.lamports);
        vet_channel_rent(&rent, channel, lamports)
    }

    /// Both token accounts `distribute` and a refund pay into must be
    /// usable before this node floats a channel's rent (ADR 0074 decision
    /// 5): its own receiving account, and the payer's canonical ATA, which
    /// must also hold the deposit.
    async fn vet_token_accounts(&self, open: &OpenChannel) -> Result<(), SponsorRefusal> {
        let receiving = spl_associated_token_account::get_associated_token_address(
            &self.receiver(),
            &open.mint,
        );
        let payer_account =
            spl_associated_token_account::get_associated_token_address(&open.payer, &open.mint);
        let addresses = [receiving, payer_account];
        let accounts = retry_read(|| {
            self.rpc
                .get_multiple_accounts_with_commitment(&addresses, CommitmentConfig::processed())
        })
        .await
        .map_err(|error| SponsorRefusal::ChainUnavailable(error.to_string()))?
        .value;
        let [receiving_state, payer_state] =
            [0, 1].map(|index| accounts.get(index).cloned().flatten());

        if let Some(reason) =
            token_account_problem(receiving_state.as_ref(), &self.receiver(), &open.mint)
        {
            return Err(SponsorRefusal::ReceivingAccountUnusable {
                account: receiving,
                reason,
            });
        }
        if let Some(reason) = token_account_problem(payer_state.as_ref(), &open.payer, &open.mint) {
            return Err(SponsorRefusal::PayerTokenAccountUnusable {
                account: payer_account,
                reason,
            });
        }
        let balance = payer_state
            .as_ref()
            .and_then(|account| spl_token::state::Account::unpack(&account.data).ok())
            .map_or(0, |token| token.amount);
        if balance < open.deposit {
            return Err(SponsorRefusal::PayerTokenAccountUnusable {
                account: payer_account,
                reason: format!("it holds {balance}, less than the deposit {}", open.deposit),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_sdk::hash::Hash;
    use solana_sdk::instruction::AccountMeta;
    use solana_sdk::message::{v0, Message};
    use solana_sdk::signature::Keypair;

    const ONE_DAY: u32 = 86_400;
    const MINIMUM_DEPOSIT: u64 = 1_000;

    struct Fixture {
        sponsor: Pubkey,
        payer: Keypair,
        session: Keypair,
        terms: SponsorTerms,
    }

    impl Fixture {
        fn new() -> Fixture {
            let sponsor = Pubkey::new_unique();
            Fixture {
                sponsor,
                payer: Keypair::new(),
                session: Keypair::new(),
                terms: SponsorTerms {
                    program_id: Pubkey::from_str(wire::PAYMENT_CHANNELS_PROGRAM_ID)
                        .expect("base58"),
                    sponsor,
                    receiver: sponsor,
                    mint: Pubkey::new_unique(),
                    min_grace_period_secs: u64::from(ONE_DAY),
                    min_sponsored_deposit: MINIMUM_DEPOSIT,
                },
            }
        }

        /// The `open` a stock client builds for this node from its greeting.
        fn open(&self) -> OpenChannel {
            OpenChannel {
                payer: self.payer.pubkey(),
                rent_payer: self.sponsor,
                payee: self.sponsor,
                mint: self.terms.mint,
                token_program: spl_token::id(),
                authorized_signer: self.session.pubkey(),
                salt: 42,
                deposit: MINIMUM_DEPOSIT,
                grace_period: ONE_DAY,
                open_slot: 1_000,
                recipients: wire::sole_recipient(&self.sponsor).to_vec(),
            }
        }

        /// A stock client's transaction: a version-0 message, fee payer the
        /// sponsor, a compute budget prefix, the open, a uniqueness memo --
        /// signed by the payer, the sponsor's slot left empty.
        fn stock(&self, open: &OpenChannel) -> Vec<Instruction> {
            vec![
                compute_unit_limit(90_000),
                compute_unit_price(1),
                open.instruction(&self.terms.program_id),
                memo(b"00112233445566778899aabbccddeeff"),
            ]
        }

        fn v0(&self, instructions: &[Instruction]) -> String {
            let message =
                v0::Message::try_compile(&self.sponsor, instructions, &[], Hash::new_unique())
                    .expect("compiles");
            self.sign(VersionedMessage::V0(message))
        }

        fn legacy(&self, instructions: &[Instruction]) -> String {
            let message =
                Message::new_with_blockhash(instructions, Some(&self.sponsor), &Hash::new_unique());
            self.sign(VersionedMessage::Legacy(message))
        }

        /// Sign `message` as the payer only, the way a client does.
        fn sign(&self, message: VersionedMessage) -> String {
            let signers = usize::from(message.header().num_required_signatures);
            let bytes = message.serialize();
            let signatures = message.static_account_keys()[..signers]
                .iter()
                .map(|key| {
                    if *key == self.payer.pubkey() {
                        self.payer.sign_message(&bytes)
                    } else {
                        Signature::default()
                    }
                })
                .collect();
            encode(&VersionedTransaction {
                signatures,
                message,
            })
        }

        fn vet(&self, transaction: &str) -> Result<VettedOpen, SponsorRefusal> {
            vet_open(transaction, &self.terms)
        }

        fn refusal(&self, transaction: &str) -> &'static str {
            self.vet(transaction).expect_err("refused").name()
        }
    }

    fn encode(transaction: &VersionedTransaction) -> String {
        base64::engine::general_purpose::STANDARD
            .encode(bincode::serialize(transaction).expect("serializes"))
    }

    fn compute_unit_limit(units: u32) -> Instruction {
        let mut data = vec![SET_COMPUTE_UNIT_LIMIT];
        data.extend_from_slice(&units.to_le_bytes());
        Instruction::new_with_bytes(solana_sdk::compute_budget::id(), &data, vec![])
    }

    fn compute_unit_price(micro_lamports: u64) -> Instruction {
        let mut data = vec![SET_COMPUTE_UNIT_PRICE];
        data.extend_from_slice(&micro_lamports.to_le_bytes());
        Instruction::new_with_bytes(solana_sdk::compute_budget::id(), &data, vec![])
    }

    fn memo(data: &[u8]) -> Instruction {
        Instruction::new_with_bytes(
            Pubkey::from_str(MEMO_PROGRAM_ID).expect("base58"),
            data,
            vec![],
        )
    }

    #[test]
    fn a_stock_clients_open_is_vetted_in_either_message_version() {
        let fixture = Fixture::new();
        let open = fixture.open();
        for transaction in [
            fixture.v0(&fixture.stock(&open)),
            fixture.legacy(&fixture.stock(&open)),
            // The bare minimum: no prefix, no memo.
            fixture.v0(&[open.instruction(&fixture.terms.program_id)]),
        ] {
            let vetted = fixture.vet(&transaction).expect("vetted");
            assert_eq!(vetted.open, open);
            assert_eq!(vetted.channel, open.channel(&fixture.terms.program_id));
        }
    }

    #[test]
    fn what_is_not_a_transaction_is_refused_as_malformed() {
        let fixture = Fixture::new();
        assert_eq!(fixture.refusal("not base64!"), "transaction_not_base64");
        let huge = base64::engine::general_purpose::STANDARD.encode(vec![0u8; 1_300]);
        assert_eq!(fixture.refusal(&huge), "transaction_too_large");
        let garbage = base64::engine::general_purpose::STANDARD.encode([7u8; 64]);
        assert_eq!(fixture.refusal(&garbage), "transaction_malformed");

        // A good transaction with a byte appended does not decode exactly.
        let good = fixture.v0(&fixture.stock(&fixture.open()));
        let mut bytes = base64::engine::general_purpose::STANDARD
            .decode(good)
            .expect("base64");
        bytes.push(0);
        let trailing = base64::engine::general_purpose::STANDARD.encode(bytes);
        assert_eq!(fixture.refusal(&trailing), "transaction_malformed");

        assert_eq!(
            fixture.vet("!").expect_err("refused").class(),
            RefusalClass::Malformed
        );
    }

    #[test]
    fn a_fee_payer_other_than_the_sponsor_is_refused() {
        let fixture = Fixture::new();
        let open = fixture.open();
        let message = v0::Message::try_compile(
            &fixture.payer.pubkey(),
            &[open.instruction(&fixture.terms.program_id)],
            &[],
            Hash::new_unique(),
        )
        .expect("compiles");
        assert_eq!(
            fixture.refusal(&fixture.sign(VersionedMessage::V0(message))),
            "fee_payer_not_sponsor"
        );
    }

    #[test]
    fn an_address_lookup_table_is_refused() {
        let fixture = Fixture::new();
        let VersionedMessage::V0(mut message) = VersionedMessage::V0(
            v0::Message::try_compile(
                &fixture.sponsor,
                &[fixture.open().instruction(&fixture.terms.program_id)],
                &[],
                Hash::new_unique(),
            )
            .expect("compiles"),
        ) else {
            unreachable!()
        };
        message
            .address_table_lookups
            .push(solana_sdk::message::v0::MessageAddressTableLookup {
                account_key: Pubkey::new_unique(),
                writable_indexes: vec![],
                readonly_indexes: vec![0],
            });
        assert_eq!(
            fixture.refusal(&fixture.sign(VersionedMessage::V0(message))),
            "address_lookup_tables_refused"
        );
    }

    /// Anything besides the one `open`, its compute budget prefix and one
    /// memo is refused: a transfer out of the sponsor, a second `open`, an
    /// ATA creation, a Lighthouse assertion, an instruction before the
    /// prefix, a missing `open`.
    #[test]
    fn every_instruction_outside_the_allowlist_is_refused() {
        let fixture = Fixture::new();
        let open = fixture.open();
        let program = fixture.terms.program_id;
        let second = OpenChannel {
            salt: 43,
            ..open.clone()
        };
        let lighthouse = Instruction::new_with_bytes(
            Pubkey::from_str("L2TExMFKdjpN9kozasaurPirfHy9P8sbXoAN1qA3S95").expect("base58"),
            &[0],
            vec![],
        );
        let create_ata =
            spl_associated_token_account::instruction::create_associated_token_account_idempotent(
                &fixture.payer.pubkey(),
                &fixture.payer.pubkey(),
                &fixture.terms.mint,
                &spl_token::id(),
            );
        let a_payer_transfer = solana_sdk::system_instruction::transfer(
            &fixture.payer.pubkey(),
            &Pubkey::new_unique(),
            1,
        );
        for instructions in [
            vec![open.instruction(&program), second.instruction(&program)],
            vec![open.instruction(&program), lighthouse],
            vec![create_ata, open.instruction(&program)],
            vec![a_payer_transfer.clone(), open.instruction(&program)],
            vec![open.instruction(&program), a_payer_transfer],
            vec![memo(b"x"), open.instruction(&program)],
            vec![compute_unit_limit(1)],
            vec![wire::top_up_instruction(
                &program,
                &fixture.payer.pubkey(),
                &open.channel(&program),
                &fixture.terms.mint,
                &spl_token::id(),
                5,
            )],
        ] {
            assert_eq!(
                fixture.refusal(&fixture.v0(&instructions)),
                "unexpected_instruction",
                "{instructions:?}"
            );
        }
        // An `open` of some other program is not this node's `open`.
        let lookalike = Pubkey::new_unique();
        assert_eq!(
            fixture.refusal(&fixture.v0(&[open.instruction(&lookalike)])),
            "unexpected_instruction"
        );
    }

    /// A transfer that debits the sponsor is the drain this module exists
    /// to refuse; it is refused wherever it sits.
    #[test]
    fn a_transfer_out_of_the_sponsor_is_never_signed() {
        let fixture = Fixture::new();
        let open = fixture.open();
        let drain = solana_sdk::system_instruction::transfer(
            &fixture.sponsor,
            &fixture.payer.pubkey(),
            1_000_000_000,
        );
        for instructions in [
            vec![drain.clone(), open.instruction(&fixture.terms.program_id)],
            vec![open.instruction(&fixture.terms.program_id), drain],
        ] {
            assert!(fixture.vet(&fixture.v0(&instructions)).is_err());
        }
    }

    #[test]
    fn the_compute_budget_prefix_is_bounded() {
        let fixture = Fixture::new();
        let open = fixture.open().instruction(&fixture.terms.program_id);
        let heap_frame =
            Instruction::new_with_bytes(solana_sdk::compute_budget::id(), &[1, 0, 0, 1, 0], vec![]);
        for prefix in [
            vec![compute_unit_limit(MAX_COMPUTE_UNIT_LIMIT + 1)],
            vec![compute_unit_price(MAX_COMPUTE_UNIT_PRICE_MICROLAMPORTS + 1)],
            vec![compute_unit_price(1), compute_unit_limit(1)],
            vec![compute_unit_limit(1), compute_unit_limit(1)],
            vec![compute_unit_price(1), compute_unit_price(1)],
            vec![heap_frame],
            vec![Instruction::new_with_bytes(
                solana_sdk::compute_budget::id(),
                &[SET_COMPUTE_UNIT_LIMIT, 1, 0, 0],
                vec![],
            )],
            vec![Instruction::new_with_bytes(
                solana_sdk::compute_budget::id(),
                &[SET_COMPUTE_UNIT_LIMIT, 1, 0, 0, 0],
                vec![AccountMeta::new_readonly(Pubkey::new_unique(), false)],
            )],
        ] {
            let mut instructions = prefix.clone();
            instructions.push(open.clone());
            assert_eq!(
                fixture.refusal(&fixture.v0(&instructions)),
                "compute_budget_refused",
                "{prefix:?}"
            );
        }
        let at_the_caps = vec![
            compute_unit_limit(MAX_COMPUTE_UNIT_LIMIT),
            compute_unit_price(MAX_COMPUTE_UNIT_PRICE_MICROLAMPORTS),
            open,
        ];
        assert!(fixture.vet(&fixture.v0(&at_the_caps)).is_ok());
    }

    #[test]
    fn the_memo_is_one_bounded_account_less_utf8_string() {
        let fixture = Fixture::new();
        let open = fixture.open().instruction(&fixture.terms.program_id);
        let with_account = Instruction::new_with_bytes(
            Pubkey::from_str(MEMO_PROGRAM_ID).expect("base58"),
            b"hi",
            vec![AccountMeta::new_readonly(Pubkey::new_unique(), false)],
        );
        for suffix in [
            vec![memo(b"a"), memo(b"b")],
            vec![memo(&[b'a'; MAX_MEMO_BYTES + 1])],
            vec![memo(&[0xff, 0xfe])],
            vec![with_account],
        ] {
            let mut instructions = vec![open.clone()];
            instructions.extend(suffix.clone());
            assert_eq!(
                fixture.refusal(&fixture.v0(&instructions)),
                "memo_refused",
                "{suffix:?}"
            );
        }
        assert!(fixture
            .vet(&fixture.v0(&[open, memo(&[b'a'; MAX_MEMO_BYTES])]))
            .is_ok());
    }

    /// The open's data is decoded exactly: short, long, or with an entry
    /// count its bytes do not carry, it is refused; so is an account list
    /// that is not fourteen long.
    #[test]
    fn a_malformed_open_is_refused() {
        let fixture = Fixture::new();
        let canonical = fixture.open().instruction(&fixture.terms.program_id);
        let mut truncated = canonical.clone();
        truncated.data.pop();
        let mut trailing = canonical.clone();
        trailing.data.push(0);
        let mut miscounted = canonical.clone();
        miscounted.data[29] = 2;
        let mut header_only = canonical.clone();
        header_only.data.truncate(OPEN_HEADER_LEN - 1);
        let mut remaining_account = canonical.clone();
        remaining_account
            .accounts
            .push(AccountMeta::new_readonly(Pubkey::new_unique(), false));
        let mut short_accounts = canonical.clone();
        short_accounts.accounts.pop();
        for open in [
            truncated,
            trailing,
            miscounted,
            header_only,
            remaining_account,
            short_accounts,
        ] {
            assert_eq!(fixture.refusal(&fixture.v0(&[open])), "open_malformed");
        }
    }

    /// Each field ADR 0074 decision 2 fixes, and each published minimum,
    /// refuses by its own name.
    #[test]
    fn every_field_decision_2_fixes_refuses_by_name() {
        let fixture = Fixture::new();
        let someone = Pubkey::new_unique();
        let good = fixture.open();
        let cases: Vec<(OpenChannel, &str)> = vec![
            (
                OpenChannel {
                    payee: someone,
                    ..good.clone()
                },
                "payee_not_sponsor",
            ),
            (
                OpenChannel {
                    mint: someone,
                    ..good.clone()
                },
                "mint_not_settled",
            ),
            (
                OpenChannel {
                    recipients: wire::sole_recipient(&someone).to_vec(),
                    ..good.clone()
                },
                "distribution_not_sole_receiver",
            ),
            (
                OpenChannel {
                    recipients: vec![DistributionEntry {
                        recipient: fixture.sponsor,
                        bps: 9_999,
                    }],
                    ..good.clone()
                },
                "distribution_not_sole_receiver",
            ),
            (
                OpenChannel {
                    recipients: vec![
                        DistributionEntry {
                            recipient: fixture.sponsor,
                            bps: 10_000,
                        },
                        DistributionEntry {
                            recipient: someone,
                            bps: 0,
                        },
                    ],
                    ..good.clone()
                },
                "distribution_not_sole_receiver",
            ),
            (
                OpenChannel {
                    recipients: vec![],
                    ..good.clone()
                },
                "distribution_not_sole_receiver",
            ),
            (
                OpenChannel {
                    grace_period: ONE_DAY - 1,
                    ..good.clone()
                },
                "grace_period_below_minimum",
            ),
            (
                OpenChannel {
                    deposit: MINIMUM_DEPOSIT - 1,
                    ..good.clone()
                },
                "deposit_below_minimum",
            ),
            (
                OpenChannel {
                    token_program: spl_associated_token_account::id(),
                    ..good.clone()
                },
                "token_program_unsupported",
            ),
        ];
        for (open, name) in cases {
            let instructions = [open.instruction(&fixture.terms.program_id)];
            assert_eq!(
                fixture.refusal(&fixture.v0(&instructions)),
                name,
                "{open:?}"
            );
        }
    }

    /// A `rent_payer` other than the sponsor is refused by name. Marked a
    /// signer, it is a third signer, which the signer rule refuses first;
    /// left a non-signer, the transaction's signers are right and the field
    /// rule names it.
    #[test]
    fn a_rent_payer_other_than_the_sponsor_is_refused() {
        let fixture = Fixture::new();
        let third_party = Keypair::new();
        let open = OpenChannel {
            rent_payer: third_party.pubkey(),
            ..fixture.open()
        };
        let transaction = fixture.v0(&[open.instruction(&fixture.terms.program_id)]);
        assert_eq!(fixture.refusal(&transaction), "unexpected_signers");

        let mut instruction = open.instruction(&fixture.terms.program_id);
        instruction.accounts[RENT_PAYER].is_signer = false;
        assert_eq!(
            fixture.refusal(&fixture.v0(&[instruction])),
            "rent_payer_not_sponsor"
        );
    }

    #[test]
    fn the_sponsor_appears_nowhere_but_its_three_seats() {
        let fixture = Fixture::new();
        let program = fixture.terms.program_id;
        let good = fixture.open();

        // As the voucher signer.
        let open = OpenChannel {
            authorized_signer: fixture.sponsor,
            ..good.clone()
        };
        assert_eq!(
            fixture.refusal(&fixture.v0(&[open.instruction(&program)])),
            "sponsor_misused"
        );

        // As the payer: the fee payer would pay the deposit too.
        let open = OpenChannel {
            payer: fixture.sponsor,
            ..good.clone()
        };
        assert_eq!(
            fixture.refusal(&fixture.v0(&[open.instruction(&program)])),
            "sponsor_misused"
        );

        // In another open slot, e.g. as the payer's token account.
        let mut instruction = good.instruction(&program);
        instruction.accounts[PAYER_TOKEN_ACCOUNT] = AccountMeta::new(fixture.sponsor, false);
        assert_eq!(
            fixture.refusal(&fixture.v0(&[instruction])),
            "sponsor_misused"
        );

        // As the mint.
        let mut instruction = good.instruction(&program);
        instruction.accounts[MINT] = AccountMeta::new_readonly(fixture.sponsor, false);
        assert_eq!(
            fixture.refusal(&fixture.v0(&[instruction])),
            "sponsor_misused"
        );

        // Outside the `open`, the allowlist refuses first -- no instruction
        // it admits there names any account -- and this rule stands behind
        // it (`a_transfer_out_of_the_sponsor_is_never_signed`).
    }

    #[test]
    fn the_signers_are_exactly_the_sponsor_and_the_payer() {
        let fixture = Fixture::new();
        let program = fixture.terms.program_id;
        let open = fixture.open();

        // A third signer: the voucher signer marked as one on the open.
        let mut instruction = open.instruction(&program);
        instruction.accounts[AUTHORIZED_SIGNER].is_signer = true;
        assert_eq!(
            fixture.refusal(&fixture.v0(&[instruction])),
            "unexpected_signers"
        );

        // A payer demoted to a read-only signer.
        let mut instruction = open.instruction(&program);
        instruction.accounts[PAYER].is_writable = false;
        assert_eq!(
            fixture.refusal(&fixture.v0(&[instruction])),
            "unexpected_signers"
        );
    }

    #[test]
    fn a_missing_or_wrong_payer_signature_is_refused() {
        let fixture = Fixture::new();
        let transaction = fixture.v0(&fixture.stock(&fixture.open()));
        let mut decoded: VersionedTransaction = bincode::deserialize(
            &base64::engine::general_purpose::STANDARD
                .decode(&transaction)
                .expect("base64"),
        )
        .expect("decodes");
        decoded.signatures[1] = Signature::default();
        assert_eq!(
            fixture.refusal(&encode(&decoded)),
            "payer_signature_invalid"
        );
        decoded.signatures[1] = Keypair::new().sign_message(&decoded.message.serialize());
        assert_eq!(
            fixture.refusal(&encode(&decoded)),
            "payer_signature_invalid"
        );
    }

    /// Every account the canonical open fixes is checked by role: the PDA,
    /// the two ATAs, the programs, the sysvar and the event authority.
    #[test]
    fn a_non_canonical_account_is_refused_by_its_role() {
        let fixture = Fixture::new();
        let program = fixture.terms.program_id;
        for slot in [
            CHANNEL,
            PAYER_TOKEN_ACCOUNT,
            CHANNEL_TOKEN_ACCOUNT,
            9,
            10,
            11,
            12,
            13,
        ] {
            let mut instruction = fixture.open().instruction(&program);
            let meta = &mut instruction.accounts[slot];
            meta.pubkey = Pubkey::new_unique();
            // Named by slot only: the refusal carries account keys, which a
            // failure message has no business printing (rust/cleartext-logging).
            let role = match fixture.vet(&fixture.v0(&[instruction])) {
                Err(SponsorRefusal::OpenAccountMismatch { role, .. }) => role,
                _ => panic!("slot {slot}: not refused as an open-account mismatch"),
            };
            assert!(
                role == OPEN_ROLES[slot],
                "slot {slot}: refused under the wrong role"
            );
        }
    }

    #[test]
    fn no_account_is_writable_but_the_five_an_open_writes() {
        let fixture = Fixture::new();
        let mut instruction = fixture.open().instruction(&fixture.terms.program_id);
        instruction.accounts[MINT].is_writable = true;
        assert_eq!(
            fixture.refusal(&fixture.v0(&[instruction])),
            "unexpected_writable_account"
        );

        let mut instruction = fixture.open().instruction(&fixture.terms.program_id);
        instruction.accounts[CHANNEL].is_writable = false;
        assert_eq!(
            fixture.refusal(&fixture.v0(&[instruction])),
            "open_malformed"
        );
    }

    #[test]
    fn every_static_refusal_is_a_refusal_not_a_failure() {
        let fixture = Fixture::new();
        let open = OpenChannel {
            deposit: 1,
            ..fixture.open()
        };
        let refusal = fixture
            .vet(&fixture.v0(&[open.instruction(&fixture.terms.program_id)]))
            .expect_err("refused");
        assert_eq!(refusal.class(), RefusalClass::Refused);
        assert_eq!(
            SponsorRefusal::ChainUnavailable("x".into()).class(),
            RefusalClass::Unavailable
        );
        assert_eq!(
            SponsorRefusal::SubmissionFailed("x".into()).class(),
            RefusalClass::Failed
        );
    }

    fn rent(exemption_threshold: f64) -> Rent {
        Rent {
            lamports_per_byte_year: 3_480,
            exemption_threshold,
            burn_percent: 50,
        }
    }

    #[test]
    fn a_threshold_of_one_never_refuses_whatever_the_channel_holds() {
        let channel = Pubkey::new_unique();
        for lamports in [0, 1, u64::MAX] {
            assert_eq!(vet_channel_rent(&rent(1.0), &channel, lamports), Ok(()));
        }
    }

    #[test]
    fn another_threshold_refuses_a_channel_short_of_the_clusters_real_minimum() {
        let channel = Pubkey::new_unique();
        let cluster = rent(2.0);
        let minimum = cluster.minimum_balance(wire::CHANNEL_ACCOUNT_LEN);
        // What `payment-channels` would top the channel up to: its own
        // threshold-ignoring figure, half the real one here.
        let programs_figure = rent(1.0).minimum_balance(wire::CHANNEL_ACCOUNT_LEN);
        assert_eq!(programs_figure * 2, minimum);

        for lamports in [0, programs_figure, minimum - 1] {
            let refusal = vet_channel_rent(&cluster, &channel, lamports).expect_err("short");
            assert_eq!(refusal.name(), "cluster_rent_threshold_unsupported");
            assert_eq!(refusal.class(), RefusalClass::Refused);
            let detail = refusal.to_string();
            for fact in [
                "exemption_threshold is 2,".to_string(),
                format!("holds {lamports} lamports"),
                format!("minimum of {minimum}"),
            ] {
                assert!(detail.contains(&fact), "{detail} names {fact}");
            }
            assert!(detail.contains(&channel.to_string()), "{detail}");
        }
        // Prefunded to the real minimum, the program tops up nothing and
        // the open is fine even here.
        for lamports in [minimum, minimum + 1, u64::MAX] {
            assert_eq!(vet_channel_rent(&cluster, &channel, lamports), Ok(()));
        }
    }

    fn token_account(
        owner: Pubkey,
        mint: Pubkey,
        state: spl_token::state::AccountState,
    ) -> solana_sdk::account::Account {
        let mut data = vec![0; spl_token::state::Account::LEN];
        spl_token::state::Account {
            mint,
            owner,
            amount: 5,
            state,
            ..spl_token::state::Account::default()
        }
        .pack_into_slice(&mut data);
        solana_sdk::account::Account {
            lamports: 1,
            data,
            owner: spl_token::id(),
            executable: false,
            rent_epoch: 0,
        }
    }

    #[test]
    fn a_token_account_is_usable_only_if_it_exists_unfrozen_for_this_owner_and_mint() {
        use spl_token::state::AccountState;
        let owner = Pubkey::new_unique();
        let mint = Pubkey::new_unique();
        let good = token_account(owner, mint, AccountState::Initialized);
        assert_eq!(token_account_problem(Some(&good), &owner, &mint), None);

        assert!(token_account_problem(None, &owner, &mint)
            .expect("missing")
            .contains("does not exist"));
        assert!(token_account_problem(
            Some(&token_account(owner, mint, AccountState::Frozen)),
            &owner,
            &mint
        )
        .expect("frozen")
        .contains("frozen"));
        assert!(token_account_problem(
            Some(&token_account(owner, mint, AccountState::Uninitialized)),
            &owner,
            &mint
        )
        .expect("uninitialized")
        .contains("initialized"));
        assert!(token_account_problem(
            Some(&token_account(
                Pubkey::new_unique(),
                mint,
                AccountState::Initialized
            )),
            &owner,
            &mint
        )
        .expect("another owner")
        .contains("belongs to"));
        assert!(token_account_problem(
            Some(&token_account(
                owner,
                Pubkey::new_unique(),
                AccountState::Initialized
            )),
            &owner,
            &mint
        )
        .expect("another mint")
        .contains("holds"));
        let foreign = solana_sdk::account::Account {
            owner: Pubkey::new_unique(),
            ..good
        };
        assert!(token_account_problem(Some(&foreign), &owner, &mint)
            .expect("not a token account")
            .contains("owned by"));
    }
}
