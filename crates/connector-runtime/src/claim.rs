//! Per-peering-relation claim exchange (ADR 0004, ADR 0005, ADR 0024,
//! `docs/protocol/peer-semantics-pre-868.md` §3, issue #423): signing and tracking
//! the claim this connector owes a peer on fulfilment, and verifying and
//! watermarking a claim a peer sends back. The nonce/watermark rule itself
//! lives in `connector_domain::validate_claim`; this module is the
//! in-memory bookkeeping and wire shape around it, plus the chain-specific
//! digest a claim's signature actually covers (issue #575:
//! `connector_signer::evm_balance_proof_digest`, the same EIP-712
//! `BalanceProof` digest `packages/contracts/src/TokenNetwork.sol` verifies
//! on redemption -- not a connector-internal SHA-256 tuple nothing on chain
//! ever checks). Durable persistence of this state (ADR 0005's journal) is
//! issue #424's job -- [`ClaimBook`] holds it only for the lifetime of the
//! process, exactly like `Connector`'s `leased_routes`.

use std::collections::{HashMap, HashSet};
use std::sync::{mpsc, Arc, RwLock};
use std::thread;

use arc_swap::ArcSwap;
use chrono::{DateTime, Utc};

use connector_domain::{
    advance_watermark, validate_claim, ClaimError, JournalEntry, Projection, Watermark,
};
use connector_signer::{
    evm_balance_proof_digest, solana_balance_proof_message, verify_evm_balance_proof,
    verify_solana_balance_proof, Address, Ed25519Signer, EvmBalanceProof, Signature, Signer,
};
use thiserror::Error;

use crate::journal::{InMemoryJournal, Journal, JournalError};
use crate::operator_view::ClaimView;

/// A claim as it travels the wire (peer-semantics-pre-868.md §3.5): a channel
/// identifier, a nonce, a cumulative amount, and a signature. `channel_id`
/// is expected to already name the channel's on-chain `bytes32` (see
/// [`ClaimBook::set_channel_domain`]) -- this type itself carries it as an
/// opaque `String` (as it always has) so the wire encoding below is
/// unchanged; it is [`ClaimBook`] that refuses to sign or accept a claim
/// whose `channel_id` was never registered as one. Distinct from
/// `connector_settlement::Claim` -- that is the on-chain redemption claim
/// (issue #425); this is the per-peering-relation claim exchanged before
/// any redemption happens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireClaim {
    pub channel_id: String,
    pub nonce: u64,
    pub cumulative_amount: u64,
    pub signature: ClaimSignature,
}

const EVM_SIGNATURE_LEN: usize = 65; // r(32) + s(32) + recovery_id(1)
const SOLANA_SIGNATURE_LEN: usize = 64; // ed25519 R(32) + S(32)

/// The scheme discriminator [`WireClaim::encode`] writes ahead of a
/// signature. Present so the in-process binary form stays decodable now
/// that a signature has two lengths (issue #732); neither carriage puts
/// these bytes on a wire (`connector_peer_btp::claim_json`'s own module
/// doc), so no deployed peer ever parses them.
const EVM_SCHEME: u8 = 0;
const SOLANA_SCHEME: u8 = 1;

impl WireClaim {
    /// Length-prefixed `channel_id` (so no two distinct tuples can ever
    /// collide on the same byte string) followed by `nonce`,
    /// `cumulative_amount`, a signature-scheme byte and the raw signature
    /// -- the ad hoc encoding for fields RFC-0027 has no concept of. ADR
    /// 0027 re-hosts this same byte string as a `payment-channel-claim`
    /// BTP protocolData entry or a `Payment-Channel-Claim` HTTP header;
    /// only the carriage moves.
    pub fn encode(&self) -> Vec<u8> {
        let channel_id_bytes = self.channel_id.as_bytes();
        let mut out =
            Vec::with_capacity(2 + channel_id_bytes.len() + 8 + 8 + 1 + EVM_SIGNATURE_LEN);
        out.extend_from_slice(&(channel_id_bytes.len() as u16).to_be_bytes());
        out.extend_from_slice(channel_id_bytes);
        out.extend_from_slice(&self.nonce.to_be_bytes());
        out.extend_from_slice(&self.cumulative_amount.to_be_bytes());
        match &self.signature {
            ClaimSignature::Evm(signature) => {
                out.push(EVM_SCHEME);
                out.extend_from_slice(&signature.r);
                out.extend_from_slice(&signature.s);
                out.push(signature.recovery_id);
            }
            ClaimSignature::Solana(signature) => {
                out.push(SOLANA_SCHEME);
                out.extend_from_slice(signature);
            }
        }
        out
    }

    /// Decode one [`WireClaim`] from the front of `bytes`, returning it
    /// alongside how many bytes it consumed so a caller can decode
    /// whatever follows (a `WireClaim` never appears alone on the wire --
    /// it always rides a PREPARE or stands as the whole of a FLUSH).
    pub fn decode(bytes: &[u8]) -> Option<(WireClaim, usize)> {
        let channel_id_len = u16::from_be_bytes(bytes.get(0..2)?.try_into().ok()?) as usize;
        let mut offset = 2;
        let channel_id =
            String::from_utf8(bytes.get(offset..offset + channel_id_len)?.to_vec()).ok()?;
        offset += channel_id_len;
        let nonce = u64::from_be_bytes(bytes.get(offset..offset + 8)?.try_into().ok()?);
        offset += 8;
        let cumulative_amount = u64::from_be_bytes(bytes.get(offset..offset + 8)?.try_into().ok()?);
        offset += 8;
        let scheme = *bytes.get(offset)?;
        offset += 1;
        let signature = match scheme {
            EVM_SCHEME => {
                let raw: [u8; EVM_SIGNATURE_LEN] = bytes
                    .get(offset..offset + EVM_SIGNATURE_LEN)?
                    .try_into()
                    .ok()?;
                offset += EVM_SIGNATURE_LEN;
                ClaimSignature::Evm(Signature::from_bytes(&raw)?)
            }
            SOLANA_SCHEME => {
                let raw: [u8; SOLANA_SIGNATURE_LEN] = bytes
                    .get(offset..offset + SOLANA_SIGNATURE_LEN)?
                    .try_into()
                    .ok()?;
                offset += SOLANA_SIGNATURE_LEN;
                ClaimSignature::Solana(raw)
            }
            _ => return None,
        };
        Some((
            WireClaim {
                channel_id,
                nonce,
                cumulative_amount,
                signature,
            },
            offset,
        ))
    }
}

/// Why a claim was rejected (peer-semantics-pre-868.md §3.4's CLAIM_ACK reasons).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimRejectReason {
    SignatureInvalid,
    NonceNotAdvancing,
    AmountNotAdvancing,
    UnknownChannel,
}

impl ClaimRejectReason {
    fn to_wire(self) -> u8 {
        match self {
            ClaimRejectReason::SignatureInvalid => 0,
            ClaimRejectReason::NonceNotAdvancing => 1,
            ClaimRejectReason::AmountNotAdvancing => 2,
            ClaimRejectReason::UnknownChannel => 3,
        }
    }

    fn from_wire(byte: u8) -> Option<ClaimRejectReason> {
        match byte {
            0 => Some(ClaimRejectReason::SignatureInvalid),
            1 => Some(ClaimRejectReason::NonceNotAdvancing),
            2 => Some(ClaimRejectReason::AmountNotAdvancing),
            3 => Some(ClaimRejectReason::UnknownChannel),
            _ => None,
        }
    }
}

/// The outcome of sending a claim (peer-semantics-pre-868.md §3.4): [`ClaimAckOutcome::NotSent`]
/// when no claim rode this frame at all, distinct from a claim that rode it
/// and was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimAckOutcome {
    NotSent,
    Accepted,
    Rejected(ClaimRejectReason),
}

impl ClaimAckOutcome {
    /// Encode the CLAIM_ACK answering a claim that was sent. Never called
    /// for [`ClaimAckOutcome::NotSent`] -- there is nothing to acknowledge,
    /// so no CLAIM_ACK frame is sent at all (the caller checks this first).
    pub fn encode(&self) -> Vec<u8> {
        match self {
            ClaimAckOutcome::Accepted => vec![0],
            ClaimAckOutcome::Rejected(reason) => vec![1, reason.to_wire()],
            ClaimAckOutcome::NotSent => vec![],
        }
    }

    pub fn decode(bytes: &[u8]) -> Option<ClaimAckOutcome> {
        match bytes.first()? {
            0 => Some(ClaimAckOutcome::Accepted),
            1 => Some(ClaimAckOutcome::Rejected(ClaimRejectReason::from_wire(
                *bytes.get(1)?,
            )?)),
            _ => None,
        }
    }
}

/// The on-chain `bytes32` identifying a channel -- what a claim's digest is
/// actually computed over, distinct from the `String` this book (and the
/// wire) otherwise knows a channel by. Parsed once, at
/// [`ClaimBook::set_channel_domain`] time (issue #575's AC4), never
/// re-derived from the `String` on every sign/verify.
pub(crate) type OnChainChannelId = [u8; 32];

/// The EIP-712 domain a channel's claims are signed and verified under
/// (`docs/protocol/peer-semantics-pre-868.md` §3.5, ADR 0024, issue #575/#566): the
/// chain a channel is deployed on and the `TokenNetwork` contract that
/// verifies a claim's signature on redemption. Configured per channel
/// rather than assumed node-wide -- each token gets its own `TokenNetwork`
/// and therefore its own `verifyingContract` (issue #566), so there is no
/// single domain a node could default to, and deliberately not read from a
/// settlement backend (issue #575: "keeping the signing domain a
/// configured input is exactly what lets this child land ... without the
/// backend retarget").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelDomain {
    pub chain_id: u64,
    pub token_network_address: Address,
}

/// A Solana peer channel's binding (issue #732): the 32-byte channel
/// account whose raw bytes open the ed25519 balance-proof message
/// (`connector_signer::solana_balance_proof_message`), and the ed25519
/// public key whose signature this connector accepts on a claim naming it.
///
/// Deliberately **not** folded into [`ChannelDomain`]. A Solana claim's
/// signature covers a tagged 96-byte little-endian message binding the
/// settlement program id (ADR 0053) -- no `verifyingContract`, no chain id,
/// and no EIP-712 typed struct anywhere in it. There is nothing an EIP-712
/// domain and this have in common to abstract over, and merging them would
/// mean one of the two carrying fields the other's verifier silently
/// ignores. `connector_domain::client_claim::ClientClaim`
/// discriminates the same two chains the same way, and this is the peer
/// wire's counterpart to that decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SolanaChannel {
    /// The settlement program this channel lives under, raw 32 bytes.
    ///
    /// Part of every balance-proof message this channel's claims sign
    /// (ADR 0053, issue #1082), so a signature made against one deployment
    /// does not verify against another. Before #1082 nothing about the chain
    /// was signed, and the separation between clusters came from program ids
    /// happening to differ -- a deployment accident standing in for a
    /// cryptographic guarantee.
    pub program_id: [u8; 32],
    /// The raw 32 bytes the channel account's base58 id decodes to --
    /// parsed once, at [`ClaimBook::set_solana_channel`] time, exactly as
    /// [`ChannelDomain`]'s `OnChainChannelId` is.
    pub channel_account: [u8; 32],
    /// The counterparty's ed25519 public key, raw. Never a claim's own
    /// self-declared `signerPublicKey` -- see
    /// [`ClaimBook::set_verification_key`] for why the peer role reads
    /// this from its own record.
    pub counterparty_public_key: [u8; 32],
}

/// A Solana channel account or counterparty key supplied to
/// [`ClaimBook::set_solana_channel`] that is not base58 of exactly 32
/// bytes. Refused where channels are configured rather than padded,
/// truncated or hashed into shape -- the same rule [`InvalidChannelId`]
/// enforces for the EVM side, and for the same reason: a channel account
/// that is not the account is a signature check against the wrong message.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{field} {value:?} is not base58 of exactly 32 bytes")]
pub struct InvalidSolanaChannel {
    pub field: &'static str,
    pub value: String,
}

/// Decode a base58 Solana account/key into its exact 32 bytes, or refuse.
pub(crate) fn parse_base58_32(
    field: &'static str,
    value: &str,
) -> Result<[u8; 32], InvalidSolanaChannel> {
    let refuse = || InvalidSolanaChannel {
        field,
        value: value.to_string(),
    };
    let decoded = bs58::decode(value).into_vec().map_err(|_| refuse())?;
    let bytes: [u8; 32] = decoded.try_into().map_err(|_| refuse())?;
    Ok(bytes)
}

/// A peer claim's signature, discriminated by the scheme its chain
/// actually uses (issue #732). The two are different lengths over
/// different messages verified by different primitives, and the peer semantics
/// keeps them apart for the whole of their travel rather than flattening
/// both into one opaque byte string -- a 64-byte ed25519 signature stuffed
/// into a 65-byte `r ‖ s ‖ v` slot is a claim this connector could no
/// longer tell you how to check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimSignature {
    /// secp256k1 `r ‖ s ‖ v` over ADR 0024's EIP-712 `BalanceProof`
    /// digest.
    Evm(Signature),
    /// ed25519 over
    /// `connector_signer::solana_balance_proof_message`'s 96 bytes.
    Solana([u8; 64]),
}

impl ClaimSignature {
    /// The signature's own bytes, in the length its scheme defines -- 65
    /// for EVM, 64 for Solana. This is what a journal entry records and
    /// what a settlement backend is later handed; nothing pads one to the
    /// other's width.
    pub fn to_bytes(&self) -> Vec<u8> {
        match self {
            ClaimSignature::Evm(signature) => signature.to_bytes().to_vec(),
            ClaimSignature::Solana(signature) => signature.to_vec(),
        }
    }

    /// The EVM signature this carries, or `None` for a Solana one. Used by
    /// the paths that are EVM-only by construction (on-chain redemption
    /// through `connector_settlement::Claim`, whose `signature` field is
    /// `connector_signer::Signature`).
    pub fn as_evm(&self) -> Option<Signature> {
        match self {
            ClaimSignature::Evm(signature) => Some(*signature),
            ClaimSignature::Solana(_) => None,
        }
    }
}

impl From<Signature> for ClaimSignature {
    fn from(signature: Signature) -> ClaimSignature {
        ClaimSignature::Evm(signature)
    }
}

/// A channel id supplied to [`ClaimBook::set_channel_domain`] that is not
/// the on-chain `bytes32` a claim's EIP-712 digest must be computed over
/// (issue #575's AC: "an id that is not one is refused where channels are
/// configured, never hashed or truncated into one"). Accepted shapes are
/// `0x`-prefixed (or bare) 64-character hex -- `TokenNetwork.sol`'s own
/// `channelId`, and the shape `EvmSettlementBackend::open` itself returns
/// since issue #576's retarget -- and a plain decimal numeral, embedded as
/// the big-endian bytes of that same integer -- the shape this workspace's
/// own `InMemorySettlementBackend` still uses. Both are exact, lossless
/// encodings of the on-chain value the string already names -- neither
/// hashes nor truncates it; anything else is refused here rather than
/// defaulted.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error(
    "channel id {0:?} is not a 32-byte on-chain identifier (expected 0x-prefixed 64 hex characters or a decimal uint256)"
)]
pub struct InvalidChannelId(pub String);

pub(crate) fn parse_channel_id(channel_id: &str) -> Result<OnChainChannelId, InvalidChannelId> {
    let hex_digits = channel_id.strip_prefix("0x").unwrap_or(channel_id);
    if hex_digits.len() == 64 && hex_digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        let mut out = [0u8; 32];
        for (i, byte) in out.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&hex_digits[i * 2..i * 2 + 2], 16)
                .expect("already validated as hex digits");
        }
        return Ok(out);
    }
    if !channel_id.is_empty() && channel_id.bytes().all(|b| b.is_ascii_digit()) {
        if let Ok(value) = channel_id.parse::<u128>() {
            let mut out = [0u8; 32];
            out[16..].copy_from_slice(&value.to_be_bytes());
            return Ok(out);
        }
    }
    Err(InvalidChannelId(channel_id.to_string()))
}

/// Build the [`EvmBalanceProof`] a peer claim's digest is computed
/// over. `locked_amount`/`locks_root` are always zero (peer-semantics-pre-868.md
/// §3.5, ADR 0004) but still hashed -- omitting them would compute a
/// different digest than `TokenNetwork.sol`'s own typehash produces
/// (`connector_signer::claim_signature`'s own doc comment). `nonce` and
/// `cumulative_amount` are `u64` on the wire but hashed at the full
/// `uint256` word width `evm_balance_proof_digest` expects, so a claim
/// signed here recovers under exactly the same digest a verifier -- on the
/// peer semantics or on chain -- computes.
pub(crate) fn evm_proof(
    on_chain_id: OnChainChannelId,
    domain: ChannelDomain,
    nonce: u64,
    cumulative_amount: u64,
) -> EvmBalanceProof {
    EvmBalanceProof {
        channel_id: on_chain_id,
        nonce,
        transferred_amount: u128::from(cumulative_amount),
        locked_amount: 0,
        locks_root: [0u8; 32],
        chain_id: domain.chain_id,
        token_network_address: domain.token_network_address,
    }
}

/// One peer's outbound claim ledger: what this connector owes it, signed
/// here and piggybacked on the next frame out.
#[derive(Default)]
struct OutboundLedger {
    channel_id: String,
    pending: Option<WireClaim>,
    pending_since: Option<DateTime<Utc>>,
    nonce: u64,
    cumulative_amount: u64,
}

/// The part of an [`OutboundLedger`] a fulfilment advances -- and therefore
/// the whole of what a batch that could not be made durable has to put
/// back. Deliberately **not** a snapshot of the whole ledger: `pending` is
/// armed by the committer only *after* its claim's entry is durable
/// ([`GroupCommitter`]), so it is never part of an advance, and restoring a
/// snapshot of it would discard a claim that became durable in the
/// meantime.
#[derive(Clone)]
struct LedgerSequence {
    channel_id: String,
    nonce: u64,
    cumulative_amount: u64,
}

impl LedgerSequence {
    fn of(ledger: &OutboundLedger) -> LedgerSequence {
        LedgerSequence {
            channel_id: ledger.channel_id.clone(),
            nonce: ledger.nonce,
            cumulative_amount: ledger.cumulative_amount,
        }
    }
}

/// This connector's claim state across every peering relation (ADR 0004,
/// ADR 0005). Signing requires a [`Signer`]; a node with none configured
/// simply never emits a claim, matching how a node with no settlement
/// backend never gets a working channel surface (`Connector::settlement`).
///
/// Outbound state (what this connector owes a peer) is keyed by `peer_id`,
/// since that is what routing decides. Inbound state (a peer's watermark
/// on a channel) is keyed by the claim's own `channel_id` instead of by
/// peer id: there is no peer identity handshake yet. ADR 0027 makes peer
/// role a matter of authentication -- a configured credential plus a
/// `[[peer_channels]]` entry -- but that config surface is issue #677 and
/// the carriages that would present it are #676, so today's accepting side
/// does not know which configured peer reached it. A
/// claim already carries its own `channel_id`, so verification and the
/// watermark it advances need nothing else to identify which channel it is
/// -- only which address is trusted to sign for that channel, configured
/// via [`ClaimBook::set_verification_key`], and that channel's EIP-712
/// domain, configured via [`ClaimBook::set_channel_domain`].
pub struct ClaimBook {
    signer: Option<Arc<dyn Signer>>,
    /// This connector's own ed25519 identity, used to sign an outbound
    /// claim on a Solana peer channel (issue #742) -- the Solana
    /// counterpart of `signer`, and deliberately a separate field rather
    /// than a second case `signer` grows: an outbound claim is signed
    /// through exactly one of the two, decided by which map its channel is
    /// registered in, never by trying one then the other.
    solana_signer: Option<Arc<dyn Ed25519Signer>>,
    /// `peer_id` -> the channel this connector claims against when it owes
    /// that peer.
    ///
    /// Copy-on-write behind an [`ArcSwap`], like the three binding maps
    /// below: a peering established at runtime (ADR 0058) binds its channel
    /// while the process is serving, and the packet path reads these on
    /// every claim. See [`ClaimBook::rebind`] for what a write costs and
    /// why that is the right trade here.
    outbound_channels: ArcSwap<HashMap<String, String>>,
    /// `channel_id` -> its parsed on-chain `bytes32` and the EIP-712 domain
    /// its claims are signed and verified under (issue #575/#566). Shared
    /// by both directions -- outbound signing (`record_fulfillment`) and
    /// inbound verification (`accept_inbound`) build the same
    /// [`EvmBalanceProof`] shape from the same channel, differing only in
    /// which nonce/amount they carry and, for inbound, which address the
    /// recovered signer must match.
    channel_domains: ArcSwap<HashMap<String, (OnChainChannelId, ChannelDomain)>>,
    /// `channel_id` -> the EVM address whose signature this connector
    /// accepts on a claim for that channel -- recovered from the signature
    /// via `connector_signer::verify_evm_balance_proof`, never the claim's
    /// own self-declared field.
    counterparties: ArcSwap<HashMap<String, Address>>,
    /// `channel_id` -> its Solana binding (issue #732): the channel account
    /// bytes a claim's ed25519 message is built over and the counterparty
    /// key its signature must verify against. Deliberately a **second**
    /// map rather than a chain field on the first: a channel is registered
    /// on exactly one chain, and a lookup that misses here after hitting
    /// `channel_domains` (or vice versa) is the honest
    /// [`ClaimRejectReason::UnknownChannel`] a claim presenting the wrong
    /// chain's signature for a channel deserves. See
    /// [`ClaimBook::set_solana_channel`].
    solana_channels: ArcSwap<HashMap<String, SolanaChannel>>,
    /// `Arc`-wrapped, like `inbound_watermarks` and `projection`, so
    /// [`GroupCommitter`]'s thread can arm a peer's pending claim once its
    /// entry is durable -- and put this ledger back if it never is --
    /// without needing `self`.
    outbound: Arc<RwLock<HashMap<String, OutboundLedger>>>,
    /// `channel_id` -> the highest nonce/amount accepted on it so far.
    inbound_watermarks: Arc<RwLock<HashMap<String, Watermark>>>,
    /// Durable record of every claim signed and every claim accepted (ADR
    /// 0005, issue #424). Defaults to [`InMemoryJournal`] -- a node that
    /// never configures a real one keeps working exactly as it did before
    /// this issue, just without surviving a restart, matching how
    /// `settlement` degrades to `None`.
    journal: Arc<dyn Journal>,
    /// Balances, derived from `journal`'s own entries rather than stored
    /// independently (ADR 0005). Updated alongside every journal append so
    /// a live read never has to replay the journal. `Arc`-wrapped so
    /// [`GroupCommitter`]'s background thread can fold a batch's entries in
    /// -- in batch order, under one lock hold -- without needing `self`.
    projection: Arc<RwLock<Projection>>,
    /// Issue #710: batches concurrent [`ClaimBook::record_fulfillment`] and
    /// [`ClaimBook::accept_inbound`] journal appends into one
    /// [`Journal::append_batch`] write, the same group-commit mechanism
    /// issue #686 gave the client edge's `ClientClaimGate`
    /// (`connector_client_edge::claim_gate::GroupCommitter`), rollback of
    /// a batch that cannot be made durable included.
    committer: GroupCommitter,
}

impl ClaimBook {
    pub fn new(
        signer: Option<Arc<dyn Signer>>,
        outbound_channels: HashMap<String, String>,
        counterparties: HashMap<String, Address>,
    ) -> ClaimBook {
        let journal: Arc<dyn Journal> = Arc::new(InMemoryJournal::new());
        let projection = Arc::new(RwLock::new(Projection::default()));
        let outbound = Arc::new(RwLock::new(HashMap::new()));
        let inbound_watermarks = Arc::new(RwLock::new(HashMap::new()));
        let committer = GroupCommitter::spawn(CommitState {
            journal: journal.clone(),
            outbound: outbound.clone(),
            inbound_watermarks: inbound_watermarks.clone(),
            projection: projection.clone(),
        });
        ClaimBook {
            signer,
            solana_signer: None,
            outbound_channels: ArcSwap::from_pointee(outbound_channels),
            channel_domains: ArcSwap::from_pointee(HashMap::new()),
            counterparties: ArcSwap::from_pointee(counterparties),
            solana_channels: ArcSwap::from_pointee(HashMap::new()),
            outbound,
            inbound_watermarks,
            journal,
            projection,
            committer,
        }
    }

    fn outbound_mut(&self) -> std::sync::RwLockWriteGuard<'_, HashMap<String, OutboundLedger>> {
        self.outbound
            .write()
            .expect("outbound claims lock poisoned")
    }

    /// Configure this connector's own signer, used to sign every outbound
    /// claim. Takes `&mut self` -- called only while a `Connector` is still
    /// being built (`mut self` builder chain), before it is shared, exactly
    /// like `Connector::with_settlement`.
    pub fn set_signer(&mut self, signer: Arc<dyn Signer>) {
        self.signer = Some(signer);
    }

    /// This node's settlement signing key, or `None` on a node that
    /// configured none.
    ///
    /// Read by the forwarding path's client role (issue #875): the key that
    /// signs a peer claim on this book is the on-chain participant of the
    /// same channel, so the claim this node signs as an ordinary *client* of
    /// a next hop is signed by exactly the same key. Exposed rather than
    /// duplicated as a second configured signer, so the two roles can never
    /// end up signing as two different addresses on one channel.
    pub fn signer(&self) -> Option<&Arc<dyn Signer>> {
        self.signer.as_ref()
    }

    /// This connector's own ed25519 identity, or `None` when it was never
    /// given one -- the Solana counterpart of [`ClaimBook::signer`], and
    /// read for the same reason it is: the outbound CLIENT ledger
    /// (`crate::outbound_client`) signs its covering claims with the very
    /// same settlement key this book signs peer claims with, because the
    /// channel's on-chain participant is the same identity in both roles
    /// (issue #1146).
    pub fn solana_signer(&self) -> Option<&Arc<dyn Ed25519Signer>> {
        self.solana_signer.as_ref()
    }

    /// Configure this connector's own ed25519 identity, used to sign every
    /// outbound claim on a channel registered through
    /// [`ClaimBook::set_solana_channel`] (issue #742) -- the Solana
    /// counterpart of [`ClaimBook::set_signer`], and under the same
    /// builder-chain contract.
    pub fn set_solana_signer(&mut self, signer: Arc<dyn Ed25519Signer>) {
        self.solana_signer = Some(signer);
    }

    /// Configure the channel this connector claims against when it owes
    /// `peer_id`.
    pub fn set_outbound_channel(&self, peer_id: impl Into<String>, channel_id: impl Into<String>) {
        Self::rebind(&self.outbound_channels, |map| {
            map.insert(peer_id.into(), channel_id.into());
        });
    }

    /// Replace one of this book's channel-binding maps with a copy that
    /// has `change` applied.
    ///
    /// The maps are read on the packet path -- once per claim signed and
    /// once per claim verified -- and written only when an operator
    /// establishes a peering (ADR 0058) or while a `Connector` is being
    /// built. So the reads take no lock at all and a write pays for a whole
    /// clone of a map with one entry per channel this node holds, which is
    /// the same trade `Connector`'s own runtime peer/route table makes and
    /// for the same reason (ADR 0015's cold-path exception).
    ///
    /// Concurrent writers can lose each other's change, exactly as a
    /// read-modify-write without a lock always can. Every caller is behind
    /// `Connector`'s `runtime_table_lock` or is single-threaded
    /// construction, so this is a property of the type rather than a race
    /// in practice -- and it is stated here rather than assumed.
    fn rebind<K, V>(swap: &ArcSwap<HashMap<K, V>>, change: impl FnOnce(&mut HashMap<K, V>))
    where
        K: Clone + Eq + std::hash::Hash,
        V: Clone,
    {
        let mut next = (**swap.load()).clone();
        change(&mut next);
        swap.store(Arc::new(next));
    }

    /// Configure the EVM address whose signature this connector accepts on
    /// an inbound claim for `channel_id` -- the channel's counterparty,
    /// never a claim's own self-declared signer (issue #575, matching
    /// `client-edge-spec.md` §1.3 step 4's rule that a forger can declare
    /// anything). Also call [`ClaimBook::set_channel_domain`] for the same
    /// `channel_id` -- without a domain configured, a claim naming it is
    /// refused as [`ClaimRejectReason::UnknownChannel`] regardless of this.
    pub fn set_verification_key(&self, channel_id: impl Into<String>, counterparty: Address) {
        Self::rebind(&self.counterparties, |map| {
            map.insert(channel_id.into(), counterparty);
        });
    }

    /// Whether `channel_id` is one this connector recognizes -- a
    /// counterparty address has been configured for it, this connector's
    /// own record of an established payment channel absent a full
    /// peer identity handshake (ADR 0027, #676). Used by probe gating (issue
    /// #426, ADR 0011): a sender with no recognized channel gets no free
    /// traversal of this connector's network.
    pub fn has_verification_key(&self, channel_id: &str) -> bool {
        self.counterparties.load().contains_key(channel_id)
    }

    /// Whether `channel_account` is a Solana channel this connector has a
    /// counterparty key configured for (issue #732/#998) -- the Solana
    /// counterpart of [`ClaimBook::has_verification_key`], for the same
    /// reason: a channel registered here can `accept_inbound` a claim
    /// naming it.
    pub fn has_solana_channel(&self, channel_account: &str) -> bool {
        self.solana_channels.load().contains_key(channel_account)
    }

    /// The watermark this book holds for `channel_id` -- the highest nonce
    /// and cumulative amount [`ClaimBook::accept_inbound`] has accepted on
    /// it (`None` before any claim has). Rebuilt from the journal on
    /// restart like every other figure here, so it survives one.
    ///
    /// Read by [`crate::Connector::peer_channel_watermark`], which is what
    /// answers `POST /ilp/claim-state` for a channel this node holds as a
    /// **peer** channel. That endpoint used to answer every channel out of
    /// the client edge's own book, which for a peer channel is a book no
    /// claim on it ever reaches -- so a `[[pay_channels]]` payer was told
    /// nonce 0 forever and re-signed the same cumulative amount at a fresh
    /// nonce on every packet.
    #[must_use]
    pub fn inbound_watermark(&self, channel_id: &str) -> Option<Watermark> {
        self.inbound_watermarks
            .read()
            .expect("inbound watermarks lock poisoned")
            .get(channel_id)
            .copied()
    }

    /// Whether `channel_id` already has a signing domain configured (issue
    /// #780) -- lets a caller that discovers channels dynamically (a
    /// client-edge payout resolved on demand rather than declared in
    /// `[[client_channels]]`) decide whether it needs to resolve one at all,
    /// without spending a payout attempt just to find out.
    pub fn has_channel_domain(&self, channel_id: &str) -> bool {
        self.channel_domains.load().contains_key(channel_id)
    }

    /// Configure `channel_id`'s EIP-712 signing domain (issue #575/#566):
    /// the chain it is deployed on and the `TokenNetwork` contract that
    /// verifies a claim's signature on redemption. Required before this
    /// channel can sign an outbound claim or accept an inbound one -- a
    /// channel with no domain configured simply never produces or accepts
    /// a claim (AC3: "produces no claim at all rather than a claim signed
    /// under a defaulted or wrong domain"), matching how a node with no
    /// signer never emits one. `channel_id` must already be the channel's
    /// on-chain `bytes32` -- see [`InvalidChannelId`] for the accepted
    /// shapes -- and is refused here, never hashed or truncated into
    /// shape, if it is not (AC4).
    pub fn set_channel_domain(
        &self,
        channel_id: impl Into<String>,
        domain: ChannelDomain,
    ) -> Result<(), InvalidChannelId> {
        let channel_id = channel_id.into();
        let on_chain_id = parse_channel_id(&channel_id)?;
        Self::rebind(&self.channel_domains, |map| {
            map.insert(channel_id, (on_chain_id, domain));
        });
        Ok(())
    }

    /// Register `channel_account` as a Solana peer channel whose claims
    /// this connector accepts from `counterparty_public_key` (issue #732)
    /// -- the Solana counterpart to [`ClaimBook::set_channel_domain`] plus
    /// [`ClaimBook::set_verification_key`] in one call, because on Solana
    /// the two are inseparable: the account *is* the whole of the signed
    /// message's domain, and there is no separate `verifyingContract` for
    /// a second call to carry.
    ///
    /// `channel_account` is the base58 account id, and doubles as the
    /// `channel_id` a claim names and a watermark is filed under -- the
    /// same string the wire's `channelAccount` field carries. Base58 of an
    /// exact 32-byte decode has one spelling, so unlike the EVM side there
    /// is nothing to canonicalise (see
    /// `connector_domain::client_claim::canonical_channel_key`'s own
    /// reasoning), and an id that does not decode to exactly 32 bytes is
    /// refused here rather than padded into shape.
    ///
    /// A channel registered here can never be confused with one registered
    /// by `set_channel_domain`: a claim's chain is decided by which
    /// signature scheme it carries, and each scheme reads only its own map
    /// (see [`ClaimBook::accept_inbound`]). Registering the same string in
    /// both maps therefore still yields two independent channels, not one
    /// ambiguous one -- and in practice cannot happen, since a 32-byte
    /// base58 id is never `0x` + 64 hex nor a decimal numeral.
    pub fn set_solana_channel(
        &self,
        channel_account: impl Into<String>,
        counterparty_public_key: &str,
        program_id: &str,
    ) -> Result<(), InvalidSolanaChannel> {
        let channel_account = channel_account.into();
        let account_bytes = parse_base58_32("channel account", &channel_account)?;
        let counterparty_public_key = parse_base58_32("counterparty key", counterparty_public_key)?;
        let program_id = parse_base58_32("program id", program_id)?;
        Self::rebind(&self.solana_channels, |map| {
            map.insert(
                channel_account,
                SolanaChannel {
                    program_id,
                    channel_account: account_bytes,
                    counterparty_public_key,
                },
            );
        });
        Ok(())
    }

    /// Configure the durable journal claims are persisted to, replaying
    /// every entry already in it to rebuild this book's in-memory state
    /// (ADR 0005, issue #424: "rebuilt from the journal on start"). Call
    /// this *after* [`ClaimBook::set_signer`] and every
    /// [`ClaimBook::set_channel_domain`] call -- rebuild re-signs a fresh
    /// claim for any peer left with an unacknowledged one (see
    /// [`ClaimBook::rebuild_from`]'s own doc), which needs both a signer
    /// and that channel's domain already in place to do; without either,
    /// that peer's cumulative state still recovers correctly, it just
    /// cannot re-arm a claim to send until a fulfilment next changes it.
    /// Takes `&mut self` for the same reason `set_signer` does -- called
    /// only while a `Connector` is still being built.
    pub fn set_journal(&mut self, journal: Arc<dyn Journal>) -> Result<(), JournalError> {
        let entries = journal.read_all()?;
        let (outbound, inbound_watermarks, projection) = Self::rebuild_from(
            &entries,
            self.signer.as_ref(),
            &self.channel_domains.load(),
            self.solana_signer.as_ref(),
            &self.solana_channels.load(),
        );
        let projection = Arc::new(RwLock::new(projection));
        let outbound = Arc::new(RwLock::new(outbound));
        let inbound_watermarks = Arc::new(RwLock::new(inbound_watermarks));
        // A fresh committer bound to the real journal and to the state the
        // replay just rebuilt -- the one spawned in `new` was writing to
        // the default `InMemoryJournal`, and holds `Arc`s to the maps this
        // method is about to replace, so it would keep batching entries
        // into a journal nothing ever replays and arming claims on ledgers
        // nothing ever reads. Dropping the old `GroupCommitter` here drops
        // its sender, which ends that thread's loop; called only while a
        // `Connector` is still being built (this method's own doc), so no
        // commit can be in flight on it.
        self.committer = GroupCommitter::spawn(CommitState {
            journal: journal.clone(),
            outbound: outbound.clone(),
            inbound_watermarks: inbound_watermarks.clone(),
            projection: projection.clone(),
        });
        self.journal = journal;
        self.outbound = outbound;
        self.inbound_watermarks = inbound_watermarks;
        self.projection = projection;
        Ok(())
    }

    /// Fold `entries` into fresh outbound/inbound state and a
    /// [`Projection`] -- the pure replay [`ClaimBook::set_journal`] drives.
    /// A peer left with `pending` unacknowledged is *always* re-armed with
    /// a freshly signed claim of the same nonce/cumulative amount when both
    /// that chain's signer and that channel's binding are available:
    /// resending an already-acknowledged claim costs nothing (the peer's
    /// own `accept_inbound` simply rejects a nonce that does not advance
    /// its watermark), so recovery needs no separate "was this
    /// acknowledged" record -- treating every rebuilt claim as pending is
    /// always safe, matching the acceptance criteria's "no manual repair".
    /// A ledger whose channel has no binding configured on either chain is
    /// left with no pending claim, exactly as
    /// [`ClaimBook::record_fulfillment`] would have refused to sign one for
    /// it live.
    ///
    /// Which chain a ledger's `channel_id` re-signs under is decided the
    /// same way [`ClaimBook::record_fulfillment`] decides it live:
    /// `channel_domains` is tried first, then `solana_channels` -- never
    /// both, and never guessed from the journal entry itself.
    /// `JournalEntry::OutboundClaimSigned` carries no chain discriminator
    /// of its own, and does not need one: a channel is registered on
    /// exactly one chain (`ClaimBook::set_solana_channel`'s own doc), and an
    /// EVM `channel_id` (0x-hex or a decimal numeral) and a Solana one
    /// (base58 of 32 bytes) can never collide, so `channel_id` alone is
    /// already enough to tell the two apart on replay.
    fn rebuild_from(
        entries: &[JournalEntry],
        signer: Option<&Arc<dyn Signer>>,
        channel_domains: &HashMap<String, (OnChainChannelId, ChannelDomain)>,
        solana_signer: Option<&Arc<dyn Ed25519Signer>>,
        solana_channels: &HashMap<String, SolanaChannel>,
    ) -> (
        HashMap<String, OutboundLedger>,
        HashMap<String, Watermark>,
        Projection,
    ) {
        let mut outbound: HashMap<String, OutboundLedger> = HashMap::new();
        let mut inbound_watermarks: HashMap<String, Watermark> = HashMap::new();
        let mut projection = Projection::default();
        for entry in entries {
            projection.apply(entry);
            match entry {
                JournalEntry::OutboundClaimSigned {
                    peer_id,
                    channel_id,
                    nonce,
                    cumulative_amount,
                } => {
                    outbound.insert(
                        peer_id.clone(),
                        OutboundLedger {
                            channel_id: channel_id.clone(),
                            pending: None,
                            pending_since: None,
                            nonce: *nonce,
                            cumulative_amount: *cumulative_amount,
                        },
                    );
                }
                JournalEntry::InboundClaimAccepted {
                    channel_id,
                    nonce,
                    cumulative_amount,
                    ..
                } => {
                    inbound_watermarks.insert(
                        channel_id.clone(),
                        advance_watermark(*nonce, *cumulative_amount),
                    );
                }
                JournalEntry::InboundFulfillmentRecorded { .. } => {}
                // Written only to the client edge's own journal (issue
                // #977) -- see the variant's own doc. `ClaimBook` is the
                // peer semantics's book and never sees one of these in practice,
                // but the two journals share this enum, so every entry kind
                // in it must still be handled here.
                JournalEntry::InboundClaimWatermarkReset { .. } => {}
                // Written only to the client edge's own journal (issue
                // #1012) -- see the variant's own doc; same reasoning as
                // `InboundClaimWatermarkReset` above.
                JournalEntry::InboundClaimRolledBack { .. } => {}
                // Written only to the client edge's own journal (ADR 0074)
                // -- same reasoning again.
                JournalEntry::BatchChannelAdmitted { .. } => {}
            }
        }
        for ledger in outbound.values_mut() {
            if let (Some(signer), Some(&(on_chain_id, domain))) =
                (signer, channel_domains.get(&ledger.channel_id))
            {
                let proof = evm_proof(on_chain_id, domain, ledger.nonce, ledger.cumulative_amount);
                if let Ok(signature) = signer.sign(&evm_balance_proof_digest(&proof)) {
                    ledger.pending = Some(WireClaim {
                        channel_id: ledger.channel_id.clone(),
                        nonce: ledger.nonce,
                        cumulative_amount: ledger.cumulative_amount,
                        signature: ClaimSignature::Evm(signature),
                    });
                }
            } else if let (Some(solana_signer), Some(&channel)) =
                (solana_signer, solana_channels.get(&ledger.channel_id))
            {
                let message = solana_balance_proof_message(
                    &channel.program_id,
                    &channel.channel_account,
                    ledger.nonce,
                    ledger.cumulative_amount,
                );
                ledger.pending = Some(WireClaim {
                    channel_id: ledger.channel_id.clone(),
                    nonce: ledger.nonce,
                    cumulative_amount: ledger.cumulative_amount,
                    signature: ClaimSignature::Solana(solana_signer.sign(&message)),
                });
            }
        }
        (outbound, inbound_watermarks, projection)
    }

    /// The channel this connector claims against when it owes `peer_id`,
    /// if one is configured (issue #424: identifies which channel an
    /// outgoing frame to `peer_id` is claimed against, independent of
    /// whether a claim happens to be pending right now).
    pub fn outbound_channel_id(&self, peer_id: &str) -> Option<String> {
        self.outbound_channels.load().get(peer_id).cloned()
    }

    /// The latest claim this connector has ever accepted on `channel_id`
    /// (issue #425), ready to submit to a `SettlementBackend::redeem` --
    /// never a superseded one, since the projection this reads from only
    /// ever retains the highest-nonce claim (peer-semantics-pre-868.md §3.4).
    /// `None` if no claim has ever been accepted on this channel.
    pub fn latest_inbound_claim(&self, channel_id: &str) -> Option<connector_settlement::Claim> {
        let (nonce, cumulative_amount, signature) = self
            .projection
            .read()
            .expect("projection lock poisoned")
            .latest_inbound_claim(channel_id)?;
        Some(connector_settlement::Claim {
            nonce,
            cumulative_amount: cumulative_amount as u128,
            signature,
        })
    }

    /// Record that a packet forwarded to `peer_id` fulfilled, owing it
    /// `amount` more (ADR 0004 -- value moves on fulfilment). Signs a fresh
    /// claim for the new cumulative total, over that channel's own binding
    /// -- the EIP-712 domain for an EVM channel (issue #575), or the
    /// ed25519 balance-proof message for a Solana one (issue #742) -- and
    /// arms it pending. `channel_domains` is tried first and
    /// `solana_channels` second, the same order and the same "exactly one,
    /// never both" rule [`ClaimBook::rebuild_from`] and
    /// [`ClaimBook::verify_signature`] hold to. Exactly one claim is
    /// produced per call -- never batched: a second fulfilment before the
    /// first claim has gone out simply supersedes it with a fresher nonce
    /// and a higher cumulative amount (peer-semantics-pre-868.md §3.2). Does
    /// nothing -- and leaves this peer's ledger untouched -- for a peer
    /// with no configured channel, or a channel whose chain has no signer
    /// or no binding configured (AC3): every one of those is a reason a
    /// claim cannot be produced at all, not a reason to produce one under a
    /// defaulted or wrong domain.
    ///
    /// **The claim is armed only once its journal entry is durable** (ADR
    /// 0005: value is not moved until the entry is durable). The advance
    /// and the enqueue happen under the outbound write lock; the fsync
    /// happens outside it, in the committer's batch; and it is the
    /// committer -- after that batch lands -- that sets `pending`, so no
    /// concurrent [`ClaimBook::pending_claim`] can ever read a claim whose
    /// entry is still in flight and ship it to the peer. A batch that
    /// cannot be made durable puts this peer's ledger back where it was
    /// and this returns `None`: nothing was signed, as far as any caller
    /// or any restart is concerned.
    pub fn record_fulfillment(
        &self,
        peer_id: &str,
        amount: u64,
        now: DateTime<Utc>,
    ) -> Option<WireClaim> {
        let channel_id = self.outbound_channels.load().get(peer_id)?.clone();

        enum Binding<'a> {
            Evm(&'a Arc<dyn Signer>, OnChainChannelId, ChannelDomain),
            Solana(&'a Arc<dyn Ed25519Signer>, SolanaChannel),
        }
        let binding =
            if let Some(&(on_chain_id, domain)) = self.channel_domains.load().get(&channel_id) {
                let signer = self.signer.as_ref()?;
                Binding::Evm(signer, on_chain_id, domain)
            } else if let Some(&channel) = self.solana_channels.load().get(&channel_id) {
                let solana_signer = self.solana_signer.as_ref()?;
                Binding::Solana(solana_signer, channel)
            } else {
                return None;
            };

        let mut outbound = self.outbound_mut();
        let ledger = outbound
            .entry(peer_id.to_string())
            .or_insert_with(|| OutboundLedger {
                channel_id: channel_id.clone(),
                ..Default::default()
            });
        if ledger.channel_id != channel_id {
            // Config now names a different channel for this peer than the
            // one this ledger's nonce/cumulative sequence was built against
            // (a peer-channel migration, issue #832) -- a new channel starts
            // its own nonce/amount sequence by definition, so carrying the
            // old watermark across it is never correct. The signing path
            // below re-signs and re-journals from nonce 1, which is exactly
            // what a restart replaying that fresh entry through
            // `rebuild_from` will also land on, so the rebind needs no
            // journal entry of its own -- but it does get a log line, since
            // discarding a watermark is precisely the kind of
            // revenue-affecting event issue #832 found happening silently.
            tracing::warn!(
                peer_id,
                retired_channel_id = %ledger.channel_id,
                retired_nonce = ledger.nonce,
                retired_cumulative_amount = ledger.cumulative_amount,
                channel_id = %channel_id,
                "config names a new channel for this peer; rebinding the outbound ledger from nonce 1"
            );
            *ledger = OutboundLedger {
                channel_id: channel_id.clone(),
                ..Default::default()
            };
        }
        // The sequence state a failed batch has to put back: taken after
        // any rebind above, so what is restored is the ledger this claim
        // was actually built on top of.
        let previous = if ledger.nonce == 0 {
            None
        } else {
            Some(LedgerSequence::of(ledger))
        };
        ledger.cumulative_amount += amount;
        ledger.nonce += 1;
        let signature = match binding {
            Binding::Evm(signer, on_chain_id, domain) => {
                let proof = evm_proof(on_chain_id, domain, ledger.nonce, ledger.cumulative_amount);
                let signature = signer.sign(&evm_balance_proof_digest(&proof)).ok()?;
                ClaimSignature::Evm(signature)
            }
            Binding::Solana(solana_signer, channel) => {
                let message = solana_balance_proof_message(
                    &channel.program_id,
                    &channel.channel_account,
                    ledger.nonce,
                    ledger.cumulative_amount,
                );
                ClaimSignature::Solana(solana_signer.sign(&message))
            }
        };
        let claim = WireClaim {
            channel_id: ledger.channel_id.clone(),
            nonce: ledger.nonce,
            cumulative_amount: ledger.cumulative_amount,
            signature,
        };
        // Enqueue while still holding the outbound write lock, then drop it
        // before waiting for the batch's fsync (issue #710) -- the same
        // shape issue #686 gave the client edge's `ClientClaimGate::admit`.
        // Enqueueing under the lock is what keeps the committer's batch
        // order identical to the order every peer's ledger actually
        // advanced in; waiting outside it is what lets a fulfilment to one
        // peer share its fsync with a concurrent fulfilment to another,
        // instead of serializing the whole connector behind one lock for
        // the length of a disk write.
        //
        // `pending` is deliberately NOT set here. Arming it is the
        // committer's job, once the entry is durable: on `main` the append
        // ran under this same lock, so nothing could read a claim before
        // its record existed, and releasing the lock to batch the fsync
        // must not quietly give that up (ADR 0005).
        let ticket = match self.committer.enqueue(PendingCommit {
            entry: JournalEntry::OutboundClaimSigned {
                peer_id: peer_id.to_string(),
                channel_id: claim.channel_id.clone(),
                nonce: claim.nonce,
                cumulative_amount: claim.cumulative_amount,
            },
            effect: CommitEffect::OutboundClaimSigned {
                peer_id: peer_id.to_string(),
                claim: claim.clone(),
                signed_at: now,
                previous: previous.clone(),
            },
        }) {
            Ok(ticket) => ticket,
            Err(CommitterGone) => {
                // Nothing will ever fsync this entry. Undo the advance
                // while still holding the lock -- no other fulfilment has
                // seen it -- and produce no claim, exactly as a peer with
                // no signer configured produces none.
                restore_ledger(&mut outbound, peer_id, previous);
                tracing::error!(
                    peer_id,
                    "not signing a claim: the peer claim journal committer is gone, so it \
                     could not be durably recorded"
                );
                return None;
            }
        };
        drop(outbound);
        if !ticket.durable() {
            // The committer has already put this peer's ledger back (see
            // `group_commit_loop`); the claim never existed.
            return None;
        }
        Some(claim)
    }

    /// The claim owed to `peer_id`, if one is pending -- what the next
    /// frame out to that peer should carry (peer-semantics-pre-868.md §3.2).
    pub fn pending_claim(&self, peer_id: &str) -> Option<WireClaim> {
        self.outbound
            .read()
            .expect("outbound claims lock poisoned")
            .get(peer_id)
            .and_then(|ledger| ledger.pending.clone())
    }

    /// The total this connector has ever signed an outbound claim for on
    /// `peer_id` -- unlike [`ClaimBook::pending_claim`], which answers
    /// `None` once the most recent claim has been acknowledged, this never
    /// resets: [`ClaimBook::acknowledge_outbound`] only ever clears
    /// `pending`, never `cumulative_amount` (issue #700's netting --
    /// `client-edge-spec.md`'s "credited" term is what this connector has
    /// *committed* to pay, not what is still in flight, since an
    /// acknowledgement confirms delivery of a claim rather than undoing the
    /// commitment it represents). `0` for a peer this book has never signed
    /// a claim for.
    pub fn outbound_cumulative_amount(&self, peer_id: &str) -> u64 {
        self.outbound
            .read()
            .expect("outbound claims lock poisoned")
            .get(peer_id)
            .map(|ledger| ledger.cumulative_amount)
            .unwrap_or(0)
    }

    /// Record the outcome of a claim of `nonce` sent to `peer_id`. On
    /// acceptance the pending mark clears -- but only if `nonce` still
    /// names the claim actually pending: a fresher fulfilment may already
    /// have superseded it while the acknowledgement was in flight, and
    /// acknowledging the stale nonce must not clear that newer claim
    /// (peer-semantics-pre-868.md §3.2).
    pub fn acknowledge_outbound(&self, peer_id: &str, nonce: u64, outcome: ClaimAckOutcome) {
        if let ClaimAckOutcome::Rejected(reason) = outcome {
            // Same rationale as `accept_inbound`'s warn (issue #832): a peer
            // rejecting a claim this connector signed is revenue-affecting
            // and must not be silent.
            tracing::warn!(peer_id, nonce, reason = ?reason, "peer rejected outbound claim");
        }
        if outcome != ClaimAckOutcome::Accepted {
            return;
        }
        let mut outbound = self.outbound_mut();
        if let Some(ledger) = outbound.get_mut(peer_id) {
            if ledger.pending.as_ref().map(|c| c.nonce) == Some(nonce) {
                ledger.pending = None;
                ledger.pending_since = None;
            }
        }
    }

    /// Whether `claim`'s signature is genuine, for the chain the signature
    /// itself is in and against this connector's **own** record of that
    /// channel (issue #732, `client-edge-spec.md` §1.3 step 4's rule that
    /// a forger can declare anything).
    ///
    /// The claim's scheme selects which record is consulted, and each
    /// scheme reads only its own map. An ed25519 signature naming a
    /// channel registered as EVM therefore finds nothing and is
    /// [`ClaimRejectReason::UnknownChannel`], and so is the mirror case --
    /// neither is ever checked against the other chain's record, and
    /// neither can be made to pass by relabelling. Both chains verify
    /// through `connector_signer::claim_signature`, the same module the
    /// client edge's own gate uses (`ClientClaimGate`), so there is one
    /// EIP-712 digest and one ed25519 message definition in this
    /// workspace, not two per edge.
    ///
    /// ADR 0002 keeps Mina out entirely: [`ClaimSignature`] has no Mina
    /// variant, so a Mina claim is refused before it can reach here, at
    /// the carriage's own `parse`.
    ///
    /// Public because it is what decides **role** (`peer-carriage-spec.md`
    /// §1.2's P3): a peer carriage asks this before it admits a frame as a
    /// peer frame, and gets the two failures apart — `UnknownChannel` for a
    /// channel this book holds no record of, `SignatureInvalid` for one
    /// that does not recover to the configured key — because §1.6 owes an
    /// operator a different sentence for each. It accepts nothing, advances
    /// no watermark and journals nothing; [`ClaimBook::accept_inbound`] is
    /// the arm that does.
    pub fn verify_signature(&self, claim: &WireClaim) -> Result<(), ClaimRejectReason> {
        match &claim.signature {
            ClaimSignature::Evm(signature) => {
                let Some(&(on_chain_id, domain)) =
                    self.channel_domains.load().get(&claim.channel_id)
                else {
                    return Err(ClaimRejectReason::UnknownChannel);
                };
                let counterparties = self.counterparties.load();
                let Some(counterparty) = counterparties.get(&claim.channel_id) else {
                    return Err(ClaimRejectReason::UnknownChannel);
                };
                let proof = evm_proof(on_chain_id, domain, claim.nonce, claim.cumulative_amount);
                if verify_evm_balance_proof(&proof, &signature.to_bytes(), counterparty) {
                    Ok(())
                } else {
                    Err(ClaimRejectReason::SignatureInvalid)
                }
            }
            ClaimSignature::Solana(signature) => {
                let solana_channels = self.solana_channels.load();
                let Some(channel) = solana_channels.get(&claim.channel_id) else {
                    return Err(ClaimRejectReason::UnknownChannel);
                };
                if verify_solana_balance_proof(
                    &channel.program_id,
                    &channel.channel_account,
                    claim.nonce,
                    claim.cumulative_amount,
                    signature,
                    &channel.counterparty_public_key,
                ) {
                    Ok(())
                } else {
                    Err(ClaimRejectReason::SignatureInvalid)
                }
            }
        }
    }

    /// Verify and, if valid, accept an inbound `claim`, advancing the
    /// watermark on its `channel_id` (peer-semantics-pre-868.md §3.4). Independent
    /// of whatever PREPARE the claim rode in on -- a rejected claim does
    /// not reject that PREPARE, and this method never looks at one. Both
    /// an unregistered channel and one with no domain configured are
    /// [`ClaimRejectReason::UnknownChannel`] -- neither leaves anything
    /// this connector could verify a signature against.
    ///
    /// A claim this connector judged good but could not durably record is
    /// [`ClaimAckOutcome::NotSent`] -- *not acknowledged*
    /// (peer-semantics-pre-868.md §6.3) -- and its watermark advance is rolled
    /// back, so the payer's retransmission is judged fresh again. It is
    /// neither `Accepted` (there is no record to back that) nor
    /// `Rejected` (§6.1's four reasons are all verdicts on the claim
    /// itself, and this claim was fine).
    pub fn accept_inbound(&self, claim: &WireClaim) -> ClaimAckOutcome {
        let outcome = self.accept_inbound_inner(claim);
        if let ClaimAckOutcome::Rejected(reason) = outcome {
            // A claim that fails to verify is a revenue-affecting event
            // (issue #832: previously silent on every path -- no log line at
            // any level and no journal entry -- which is what let a
            // peer-channel migration silently un-pay the peering).
            tracing::warn!(
                channel_id = %claim.channel_id,
                nonce = claim.nonce,
                cumulative_amount = claim.cumulative_amount,
                reason = ?reason,
                "rejected inbound claim"
            );
        }
        outcome
    }

    fn accept_inbound_inner(&self, claim: &WireClaim) -> ClaimAckOutcome {
        match self.verify_signature(claim) {
            Ok(()) => {}
            Err(reason) => return ClaimAckOutcome::Rejected(reason),
        }

        let mut watermarks = self
            .inbound_watermarks
            .write()
            .expect("inbound watermarks lock poisoned");
        let watermark = watermarks.get(&claim.channel_id).copied();
        match validate_claim(watermark, claim.nonce, claim.cumulative_amount) {
            Ok(()) => {
                watermarks.insert(
                    claim.channel_id.clone(),
                    advance_watermark(claim.nonce, claim.cumulative_amount),
                );
                // Enqueue before dropping the watermark lock, then wait
                // outside it (issue #710, mirroring `record_fulfillment`
                // and issue #686's own `ClientClaimGate::admit`): a claim
                // accepted on one channel shares its fsync with a
                // concurrent acceptance on another instead of serializing
                // behind one lock for the length of a disk write, and this
                // channel's own entries still reach the committer in
                // exactly the order their watermark advanced in.
                let ticket = match self.committer.enqueue(PendingCommit {
                    entry: JournalEntry::InboundClaimAccepted {
                        channel_id: claim.channel_id.clone(),
                        nonce: claim.nonce,
                        cumulative_amount: claim.cumulative_amount,
                        signature: claim.signature.to_bytes(),
                    },
                    effect: CommitEffect::InboundClaimAccepted {
                        channel_id: claim.channel_id.clone(),
                        previous: watermark,
                    },
                }) {
                    Ok(ticket) => ticket,
                    Err(CommitterGone) => {
                        // Nothing will ever fsync this entry. Undo the
                        // advance while still holding the lock -- no other
                        // claim has seen it -- and leave the claim
                        // unacknowledged.
                        restore_watermark(&mut watermarks, &claim.channel_id, watermark);
                        tracing::error!(
                            channel_id = %claim.channel_id,
                            "not acknowledging a valid claim: the peer claim journal committer \
                             is gone, so its acceptance could not be durably recorded"
                        );
                        return ClaimAckOutcome::NotSent;
                    }
                };
                drop(watermarks);
                if ticket.durable() {
                    ClaimAckOutcome::Accepted
                } else {
                    // The committer has already put this channel's
                    // watermark back (see `group_commit_loop`), so the
                    // same claim retransmitted is still good.
                    // peer-semantics-pre-868.md §6.3: *not acknowledged* is the
                    // honest answer -- no ack header rides the response,
                    // the payer's claim stays pending, and it retransmits.
                    // Answering `accepted` would claim a record this node
                    // does not have; answering `rejected` would be one of
                    // four verdicts about the claim itself, none of which
                    // is true of a perfectly good claim this node merely
                    // could not write down.
                    ClaimAckOutcome::NotSent
                }
            }
            Err(ClaimError::NonceNotAdvancing { .. }) => {
                ClaimAckOutcome::Rejected(ClaimRejectReason::NonceNotAdvancing)
            }
            Err(ClaimError::AmountNotAdvancing { .. }) => {
                ClaimAckOutcome::Rejected(ClaimRejectReason::AmountNotAdvancing)
            }
            // The peer semantics never calls `validate_price` -- a route's price
            // is charged at the client edge (issue #522), not against a
            // peer's own claim -- so this arm is unreachable in practice;
            // it exists only so this match stays exhaustive as
            // `ClaimError` grows new variants.
            Err(ClaimError::Underpayment { .. }) => unreachable!(
                "accept_inbound never calls validate_price, so a peer claim cannot fail with Underpayment"
            ),
        }
    }

    /// Every peer's claim state, for the operator surface's read-only
    /// inspection interface (issue #420). `peer_id` is `None` on an inbound
    /// entry for the same reason `accept_inbound` needs none: the peer semantics
    /// has no identity handshake yet, so only the channel is known.
    pub fn views(&self) -> Vec<ClaimView> {
        let mut views: Vec<ClaimView> = self
            .outbound
            .read()
            .expect("outbound claims lock poisoned")
            .iter()
            .filter(|(_, ledger)| ledger.nonce > 0)
            .map(|(peer_id, ledger)| ClaimView {
                peer_id: Some(peer_id.clone()),
                channel_id: ledger.channel_id.clone(),
                direction: crate::operator_view::ClaimDirection::Outbound,
                nonce: ledger.nonce,
                cumulative_amount: ledger.cumulative_amount,
                pending: ledger.pending.is_some(),
                book: crate::operator_view::ClaimBookKind::Peer,
            })
            .collect();
        views.extend(
            self.inbound_watermarks
                .read()
                .expect("inbound watermarks lock poisoned")
                .iter()
                .map(|(channel_id, watermark)| ClaimView {
                    peer_id: None,
                    channel_id: channel_id.clone(),
                    direction: crate::operator_view::ClaimDirection::Inbound,
                    nonce: watermark.nonce,
                    cumulative_amount: watermark.cumulative_amount,
                    pending: false,
                    book: crate::operator_view::ClaimBookKind::Peer,
                }),
        );
        views
    }
}

/// The most entries one journal batch carries -- a bound on the buffer a
/// commit builds, not a tuning knob, mirroring
/// `connector_client_edge::claim_gate`'s own `GROUP_COMMIT_MAX_BATCH`: the
/// committer drains only what is already queued, so a batch is naturally
/// sized by how many entries arrived during the previous batch's fsync.
const GROUP_COMMIT_MAX_BATCH: usize = 4096;

/// What the state this connector reads live still owes a queued
/// [`JournalEntry`] once its batch resolves: the advance to *complete* if
/// the batch is durable, and the advance to *undo* if it is not. One
/// variant per journal entry [`ClaimBook`] writes.
enum CommitEffect {
    /// `record_fulfillment` advanced `peer_id`'s ledger to `claim`'s
    /// nonce/cumulative amount. On success `claim` is armed as that peer's
    /// pending claim -- the first moment anything may transmit it (ADR
    /// 0005). On failure the ledger goes back to `previous`, so the next
    /// fulfilment re-signs this same nonce rather than skipping it.
    OutboundClaimSigned {
        peer_id: String,
        claim: WireClaim,
        signed_at: DateTime<Utc>,
        previous: Option<LedgerSequence>,
    },
    /// `accept_inbound` advanced `channel_id`'s watermark; on failure it
    /// goes back to `previous`, so the peer's retransmission of the very
    /// same claim is judged fresh again rather than bouncing off its own
    /// unrecorded ghost.
    InboundClaimAccepted {
        channel_id: String,
        previous: Option<Watermark>,
    },
}

/// One entry queued for the committer: what to write, and what that write
/// landing (or not) does to the live state it was decided against.
struct PendingCommit {
    entry: JournalEntry,
    effect: CommitEffect,
}

/// The committer thread has exited, so nothing will ever journal this
/// entry. Only possible after that thread panicked -- its loop runs until
/// the book (the sender) is dropped.
struct CommitterGone;

/// A queued entry's pending durability: resolves once the batch carrying
/// it has been fsync'd, or reports that it could not be. Its caller has
/// already released the lock its advance was decided under, so a failure
/// here has been rolled back by the committer rather than by the caller.
struct DurabilityTicket {
    durable: mpsc::Receiver<bool>,
}

impl DurabilityTicket {
    /// Block until this entry's batch -- and every other entry sharing it
    /// -- has been written, and answer whether it is durable.
    /// `record_fulfillment`/`accept_inbound`'s entire durability contract:
    /// synchronous, and it always returns. A sender dropped without an
    /// answer is a committer that died mid-batch, which is not durable
    /// either.
    fn durable(self) -> bool {
        matches!(self.durable.recv(), Ok(true))
    }
}

/// Everything the committer thread touches: the journal it writes and the
/// three pieces of live state a batch completes or undoes. Grouped so
/// [`GroupCommitter::spawn`] takes one argument rather than four, and so
/// [`ClaimBook::set_journal`] cannot rebind one of them and forget another.
struct CommitState {
    journal: Arc<dyn Journal>,
    outbound: Arc<RwLock<HashMap<String, OutboundLedger>>>,
    inbound_watermarks: Arc<RwLock<HashMap<String, Watermark>>>,
    projection: Arc<RwLock<Projection>>,
}

/// Issue #710's group commit for [`ClaimBook`]'s peer claim journal: a
/// dedicated thread that drains every [`PendingCommit`] queued since the
/// last batch and writes them as one [`Journal::append_batch`] -- one
/// write, one fsync -- instead of the one-fsync-per-entry `Journal::append`
/// calls `record_fulfillment`/`accept_inbound` each made directly before
/// this issue. The mechanism is issue #686's, adopted rather than
/// reinvented (see `connector_client_edge::claim_gate::GroupCommitter`):
/// concurrent forwards queue behind one another only for the microseconds
/// it takes to enqueue, not for a whole fsync each.
///
/// A dedicated OS thread rather than a task because
/// [`Journal::append_batch`] blocks on disk I/O and this loop exists to do
/// nothing else; it exits when the book is dropped (the sender goes away)
/// and takes nothing with it.
///
/// **Nothing is published before its entry is durable, and a batch that
/// cannot be made durable is rolled back.** Those are the two halves of
/// ADR 0005 that holding the append under the caller's write lock used to
/// give for free, and moving the fsync out from under that lock has to buy
/// back explicitly:
///
/// * *Publish after.* A signed outbound claim is a bearer instrument --
///   `Connector::forward` reads `pending_claim` and ships it -- so
///   `record_fulfillment` advances the ledger and enqueues under the lock
///   but does not arm `pending`; this thread arms it, after the batch
///   lands. Without that, a forward on another thread could transmit a
///   nonce whose journal entry never made it to disk, and a restart would
///   replay a lower nonce that the peer already holds and will reject as
///   non-advancing -- the value unrecoverable, which is precisely what
///   ADR 0005 exists to prevent.
/// * *Roll back.* A failed batch leaves ledger nonces and inbound
///   watermarks promising a durable record that does not exist. So this
///   thread retakes the write locks those advances were decided under,
///   drains whatever else was queued against the now-unrecorded state
///   (it could only have landed in this batch or a later one, and there is
///   no later one until this loop comes back around), restores every
///   touched peer and channel to its state before the *earliest* failed
///   entry, and only then releases the waiters -- who answer `None` /
///   *not acknowledged*. The projection is likewise folded only on
///   success: under ADR 0005 it is derived from the journal, so an entry
///   with no journal line behind it has no business in it.
struct GroupCommitter {
    sender: mpsc::Sender<(PendingCommit, mpsc::Sender<bool>)>,
}

impl GroupCommitter {
    fn spawn(state: CommitState) -> GroupCommitter {
        let (sender, receiver) = mpsc::channel();
        thread::Builder::new()
            .name("peer-claim-journal-commit".to_string())
            .spawn(move || group_commit_loop(receiver, state))
            .expect("spawning the peer claim journal committer thread");
        GroupCommitter { sender }
    }

    /// Queue `pending` for the next batch -- microseconds, no I/O. Callers
    /// hold the write lock their advance was decided under while calling
    /// this; that is the ordering guarantee, not an accident, and it is
    /// what keeps the committer's batch order identical to the order those
    /// advances happened in.
    fn enqueue(&self, pending: PendingCommit) -> Result<DurabilityTicket, CommitterGone> {
        let (done_tx, done_rx) = mpsc::channel();
        self.sender
            .send((pending, done_tx))
            .map_err(|_| CommitterGone)?;
        Ok(DurabilityTicket { durable: done_rx })
    }
}

type QueuedCommit = (PendingCommit, mpsc::Sender<bool>);

fn group_commit_loop(receiver: mpsc::Receiver<QueuedCommit>, state: CommitState) {
    while let Ok(first) = receiver.recv() {
        let mut batch = vec![first];
        while batch.len() < GROUP_COMMIT_MAX_BATCH {
            match receiver.try_recv() {
                Ok(queued) => batch.push(queued),
                Err(_) => break,
            }
        }
        // Split rather than clone: the entries go to the journal, the
        // effects and waiters stay here for whatever the write says. Both
        // halves keep batch order, which is enqueue order, which is the
        // order the advances they describe actually happened in.
        let (entries, mut resolved): (Vec<JournalEntry>, Vec<(CommitEffect, mpsc::Sender<bool>)>) =
            batch
                .into_iter()
                .map(|(pending, done)| (pending.entry, (pending.effect, done)))
                .unzip();
        match state.journal.append_batch(&entries) {
            Ok(()) => {
                {
                    // Applied in batch order, so the projection never sees
                    // a later entry before an earlier one for the same
                    // channel, regardless of which caller's thread wakes
                    // first.
                    let mut projection =
                        state.projection.write().expect("projection lock poisoned");
                    for entry in &entries {
                        projection.apply(entry);
                    }
                }
                arm_pending_claims(&state.outbound, &resolved);
                for (_, done) in resolved {
                    // A receiver gone before its batch lands means the
                    // caller stopped waiting for some other reason -- the
                    // entry is durable regardless, so there is nothing to
                    // do about it.
                    let _ = done.send(true);
                }
            }
            Err(err) => {
                tracing::error!(
                    %err,
                    entries = entries.len(),
                    "failed to durably append a batch of peer claim journal entries; rolling \
                     back every advance they recorded"
                );
                roll_back(&state, &receiver, &mut resolved);
                for (_, done) in resolved {
                    let _ = done.send(false);
                }
            }
        }
    }
}

/// Arm every outbound claim in a batch that has just been made durable --
/// the moment a signed claim becomes visible to `pending_claim`, and
/// therefore transmittable. In batch order, so when one peer's ledger
/// advanced twice in a batch the fresher claim is the one left pending
/// (peer-semantics-pre-868.md §3.2's supersession), and under one lock hold.
fn arm_pending_claims(
    outbound: &RwLock<HashMap<String, OutboundLedger>>,
    resolved: &[(CommitEffect, mpsc::Sender<bool>)],
) {
    if !resolved
        .iter()
        .any(|(effect, _)| matches!(effect, CommitEffect::OutboundClaimSigned { .. }))
    {
        return;
    }
    let mut ledgers = outbound.write().expect("outbound claims lock poisoned");
    for (effect, _) in resolved {
        if let CommitEffect::OutboundClaimSigned {
            peer_id,
            claim,
            signed_at,
            ..
        } = effect
        {
            if let Some(ledger) = ledgers.get_mut(peer_id) {
                ledger.pending = Some(claim.clone());
                ledger.pending_since = Some(*signed_at);
            }
        }
    }
}

/// Undo every advance a failed batch recorded, plus every advance queued
/// behind it -- see [`GroupCommitter`]'s doc for why both. `resolved` is
/// extended with whatever is drained, so its waiters are refused too.
fn roll_back(
    state: &CommitState,
    receiver: &mpsc::Receiver<QueuedCommit>,
    resolved: &mut Vec<(CommitEffect, mpsc::Sender<bool>)>,
) {
    // Both locks for the whole unwind, taken in this order everywhere they
    // are taken together (only here -- `record_fulfillment` and
    // `accept_inbound` each take exactly one), so nothing can be decided
    // against state that is about to be rolled back.
    let mut ledgers = state
        .outbound
        .write()
        .expect("outbound claims lock poisoned");
    let mut watermarks = state
        .inbound_watermarks
        .write()
        .expect("inbound watermarks lock poisoned");
    while let Ok((pending, done)) = receiver.try_recv() {
        resolved.push((pending.effect, done));
    }
    // First failed effect per peer/channel wins: effects are in advance
    // order, so its `previous` is the last state with a durable record
    // behind it. Two sets rather than one -- a peer id and a channel id
    // are different namespaces and may collide as strings.
    let mut restored_peers: HashSet<&str> = HashSet::new();
    let mut restored_channels: HashSet<&str> = HashSet::new();
    for (effect, _) in resolved.iter() {
        match effect {
            CommitEffect::OutboundClaimSigned {
                peer_id, previous, ..
            } => {
                if restored_peers.insert(peer_id.as_str()) {
                    restore_ledger(&mut ledgers, peer_id, previous.clone());
                }
            }
            CommitEffect::InboundClaimAccepted {
                channel_id,
                previous,
            } => {
                if restored_channels.insert(channel_id.as_str()) {
                    restore_watermark(&mut watermarks, channel_id, *previous);
                }
            }
        }
    }
}

/// Put `peer_id`'s ledger sequence back to `previous` -- the inverse of one
/// fulfilment's advance. `pending` and `pending_since` are deliberately
/// left alone: they are armed only after a batch is durable, so whatever
/// they hold is a claim with a journal line behind it that this unwind has
/// no business discarding. `None` is a ledger that had never advanced, so
/// the entry goes with it.
fn restore_ledger(
    ledgers: &mut HashMap<String, OutboundLedger>,
    peer_id: &str,
    previous: Option<LedgerSequence>,
) {
    match previous {
        Some(sequence) => {
            if let Some(ledger) = ledgers.get_mut(peer_id) {
                ledger.channel_id = sequence.channel_id;
                ledger.nonce = sequence.nonce;
                ledger.cumulative_amount = sequence.cumulative_amount;
            }
        }
        None => {
            ledgers.remove(peer_id);
        }
    }
}

/// Put `channel_id` back to `previous` -- the inverse of one watermark
/// advance, the same unwind
/// `connector_client_edge::claim_gate::restore_watermark` performs for the
/// client edge's gate.
fn restore_watermark(
    watermarks: &mut HashMap<String, Watermark>,
    channel_id: &str,
    previous: Option<Watermark>,
) {
    match previous {
        Some(watermark) => {
            watermarks.insert(channel_id.to_string(), watermark);
        }
        None => {
            watermarks.remove(channel_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use connector_signer::{derive_evm_address, LocalSigner};

    fn now() -> DateTime<Utc> {
        "2030-01-01T00:00:00Z".parse().unwrap()
    }

    /// A fixed EIP-712 domain every test channel shares -- Base Sepolia's
    /// chain id and an arbitrary `TokenNetwork` address; nothing in this
    /// module's tests depends on their real-world provenance, only that
    /// signing and verifying a claim use the same domain unless a test
    /// deliberately varies it.
    fn test_domain() -> ChannelDomain {
        ChannelDomain {
            chain_id: 84_532,
            token_network_address: [0x1E; 20],
        }
    }

    /// A valid on-chain `bytes32` channel id for tests -- `0x` followed by
    /// `n` left-padded to 64 hex characters (issue #575's AC4: a peer-role
    /// claim's channel id must already be a real bytes32, never an
    /// arbitrary label like the old `"channel-a"` placeholders this module
    /// used before this issue).
    fn channel_id(n: u8) -> String {
        format!("0x{n:064x}")
    }

    /// Sign a claim for `channel`/`nonce`/`amount` under [`test_domain`],
    /// exactly as [`ClaimBook::record_fulfillment`] would.
    fn sign_claim(signer: &LocalSigner, channel: &str, nonce: u64, amount: u64) -> WireClaim {
        let on_chain_id = parse_channel_id(channel).expect("test channel id is valid");
        let proof = evm_proof(on_chain_id, test_domain(), nonce, amount);
        WireClaim {
            channel_id: channel.to_string(),
            nonce,
            cumulative_amount: amount,
            signature: ClaimSignature::Evm(
                signer
                    .sign(&evm_balance_proof_digest(&proof))
                    .expect("sign"),
            ),
        }
    }

    /// A book that can both sign outbound claims to `peer_id` on
    /// `channel`, and verify inbound claims on `channel` against
    /// `counterparty` -- with `channel`'s domain already registered as
    /// [`test_domain`].
    fn book_with_peer(peer_id: &str, channel: &str, counterparty: Address) -> ClaimBook {
        let signer = Arc::new(LocalSigner::generate("claim-key"));
        let mut outbound_channels = HashMap::new();
        outbound_channels.insert(peer_id.to_string(), channel.to_string());
        let mut counterparties = HashMap::new();
        counterparties.insert(channel.to_string(), counterparty);
        let book = ClaimBook::new(Some(signer), outbound_channels, counterparties);
        book.set_channel_domain(channel, test_domain())
            .expect("test channel id is valid");
        book
    }

    #[test]
    fn a_wire_claim_round_trips_through_encode_and_decode() {
        let claim = WireClaim {
            channel_id: channel_id(1),
            nonce: 7,
            cumulative_amount: 900,
            signature: ClaimSignature::Evm(Signature {
                r: [1u8; 32],
                s: [2u8; 32],
                recovery_id: 1,
            }),
        };
        let mut bytes = claim.encode();
        bytes.extend_from_slice(b"trailing");

        let (decoded, consumed) = WireClaim::decode(&bytes).unwrap();
        assert_eq!(decoded, claim);
        assert_eq!(&bytes[consumed..], b"trailing");
    }

    #[test]
    fn a_claim_ack_round_trips_through_encode_and_decode() {
        for outcome in [
            ClaimAckOutcome::Accepted,
            ClaimAckOutcome::Rejected(ClaimRejectReason::SignatureInvalid),
            ClaimAckOutcome::Rejected(ClaimRejectReason::NonceNotAdvancing),
            ClaimAckOutcome::Rejected(ClaimRejectReason::AmountNotAdvancing),
            ClaimAckOutcome::Rejected(ClaimRejectReason::UnknownChannel),
        ] {
            let bytes = outcome.encode();
            assert_eq!(ClaimAckOutcome::decode(&bytes), Some(outcome));
        }
    }

    mod channel_id_parsing {
        use super::*;

        #[test]
        fn a_0x_prefixed_64_hex_char_id_parses_exactly() {
            let mut expected = [0u8; 32];
            expected[31] = 0xab;
            assert_eq!(parse_channel_id(&format!("0x{:064x}", 0xab)), Ok(expected));
        }

        #[test]
        fn a_bare_64_hex_char_id_parses_the_same_as_0x_prefixed() {
            assert_eq!(
                parse_channel_id(&"ab".repeat(32)),
                parse_channel_id(&format!("0x{}", "ab".repeat(32)))
            );
        }

        #[test]
        fn a_decimal_numeral_embeds_as_big_endian_bytes_of_that_integer() {
            let mut expected = [0u8; 32];
            expected[31] = 42;
            assert_eq!(parse_channel_id("42"), Ok(expected));
            assert_eq!(parse_channel_id("0"), Ok([0u8; 32]));
        }

        #[test]
        fn an_arbitrary_label_is_refused_rather_than_hashed_or_truncated() {
            assert_eq!(
                parse_channel_id("channel-a"),
                Err(InvalidChannelId("channel-a".to_string()))
            );
            assert_eq!(parse_channel_id(""), Err(InvalidChannelId(String::new())));
            // One hex character short of 32 bytes -- not silently padded.
            assert_eq!(
                parse_channel_id(&"a".repeat(63)),
                Err(InvalidChannelId("a".repeat(63)))
            );
        }

        #[test]
        fn set_channel_domain_refuses_an_invalid_channel_id_and_registers_nothing() {
            let book = ClaimBook::new(None, HashMap::new(), HashMap::new());

            let result = book.set_channel_domain("channel-a", test_domain());

            assert_eq!(result, Err(InvalidChannelId("channel-a".to_string())));
        }
    }

    #[test]
    fn no_claim_is_recorded_without_a_signer() {
        let mut outbound_channels = HashMap::new();
        outbound_channels.insert("peer-b".to_string(), channel_id(1));
        let book = ClaimBook::new(None, outbound_channels, HashMap::new());
        book.set_channel_domain(channel_id(1), test_domain())
            .unwrap();

        assert!(book.record_fulfillment("peer-b", 100, now()).is_none());
    }

    #[test]
    fn no_claim_is_recorded_for_an_unregistered_peer() {
        let book = ClaimBook::new(
            Some(Arc::new(LocalSigner::generate("k"))),
            HashMap::new(),
            HashMap::new(),
        );

        assert!(book.record_fulfillment("peer-b", 100, now()).is_none());
    }

    #[test]
    fn no_claim_is_recorded_for_a_channel_with_no_domain_configured() {
        let signer = Arc::new(LocalSigner::generate("k"));
        let mut outbound_channels = HashMap::new();
        outbound_channels.insert("peer-b".to_string(), channel_id(1));
        // Deliberately never calling `set_channel_domain`.
        let book = ClaimBook::new(Some(signer), outbound_channels, HashMap::new());

        assert!(book.record_fulfillment("peer-b", 100, now()).is_none());
        assert_eq!(book.pending_claim("peer-b"), None);
    }

    #[test]
    fn recording_a_fulfillment_arms_exactly_one_pending_claim_with_nonce_one() {
        let key = derive_evm_address(&LocalSigner::generate("k").public_key().unwrap());
        let book = book_with_peer("peer-b", &channel_id(1), key);

        let claim = book.record_fulfillment("peer-b", 100, now()).unwrap();

        assert_eq!(claim.nonce, 1);
        assert_eq!(claim.cumulative_amount, 100);
        assert_eq!(book.pending_claim("peer-b"), Some(claim));
    }

    #[test]
    fn a_second_fulfillment_before_the_first_drains_supersedes_it_rather_than_batching() {
        let key = derive_evm_address(&LocalSigner::generate("k").public_key().unwrap());
        let book = book_with_peer("peer-b", &channel_id(1), key);

        book.record_fulfillment("peer-b", 100, now()).unwrap();
        let second = book.record_fulfillment("peer-b", 50, now()).unwrap();

        // Exactly one pending claim, holding the latest cumulative state --
        // not two, and not the first one.
        assert_eq!(second.nonce, 2);
        assert_eq!(second.cumulative_amount, 150);
        assert_eq!(book.pending_claim("peer-b"), Some(second));
    }

    #[test]
    fn a_channel_change_rebinds_the_ledger_instead_of_carrying_the_old_watermark() {
        let key = derive_evm_address(&LocalSigner::generate("k").public_key().unwrap());
        let book = book_with_peer("peer-b", &channel_id(1), key);
        book.record_fulfillment("peer-b", 100, now()).unwrap();
        book.record_fulfillment("peer-b", 50, now()).unwrap();
        assert_eq!(book.outbound_cumulative_amount("peer-b"), 150);

        // Config now names a different channel for the same peer -- a
        // peer-channel migration (issue #832). Reach in and repoint
        // `outbound_channels` the way `Connector` reconfiguring
        // `[[peer_channels]]` and restarting would.
        book.set_channel_domain(channel_id(2), test_domain())
            .unwrap();
        book.set_outbound_channel("peer-b", channel_id(2));

        let claim = book.record_fulfillment("peer-b", 10, now()).unwrap();

        // A fresh nonce/amount sequence on the new channel, not nonce 3 /
        // cumulative 160 carried over from the old one.
        assert_eq!(claim.channel_id, channel_id(2));
        assert_eq!(claim.nonce, 1);
        assert_eq!(claim.cumulative_amount, 10);
        assert_eq!(book.outbound_cumulative_amount("peer-b"), 10);
    }

    #[test]
    fn acknowledging_the_pending_nonce_clears_it() {
        let key = derive_evm_address(&LocalSigner::generate("k").public_key().unwrap());
        let book = book_with_peer("peer-b", &channel_id(1), key);
        let claim = book.record_fulfillment("peer-b", 100, now()).unwrap();

        book.acknowledge_outbound("peer-b", claim.nonce, ClaimAckOutcome::Accepted);

        assert_eq!(book.pending_claim("peer-b"), None);
    }

    #[test]
    fn acknowledging_a_stale_nonce_does_not_clear_a_fresher_pending_claim() {
        let key = derive_evm_address(&LocalSigner::generate("k").public_key().unwrap());
        let book = book_with_peer("peer-b", &channel_id(1), key);
        let first = book.record_fulfillment("peer-b", 100, now()).unwrap();
        let second = book.record_fulfillment("peer-b", 50, now()).unwrap();

        book.acknowledge_outbound("peer-b", first.nonce, ClaimAckOutcome::Accepted);

        assert_eq!(book.pending_claim("peer-b"), Some(second));
    }

    #[test]
    fn a_rejected_ack_leaves_the_claim_pending() {
        let key = derive_evm_address(&LocalSigner::generate("k").public_key().unwrap());
        let book = book_with_peer("peer-b", &channel_id(1), key);
        let claim = book.record_fulfillment("peer-b", 100, now()).unwrap();

        book.acknowledge_outbound(
            "peer-b",
            claim.nonce,
            ClaimAckOutcome::Rejected(ClaimRejectReason::SignatureInvalid),
        );

        assert_eq!(book.pending_claim("peer-b"), Some(claim));
    }

    #[test]
    fn outbound_cumulative_amount_is_zero_for_a_peer_never_signed_for() {
        let book = ClaimBook::new(None, HashMap::new(), HashMap::new());

        assert_eq!(book.outbound_cumulative_amount("peer-b"), 0);
    }

    #[test]
    fn outbound_cumulative_amount_tracks_the_running_total_across_fulfillments() {
        let key = derive_evm_address(&LocalSigner::generate("k").public_key().unwrap());
        let book = book_with_peer("peer-b", &channel_id(1), key);

        book.record_fulfillment("peer-b", 100, now()).unwrap();
        book.record_fulfillment("peer-b", 50, now()).unwrap();

        assert_eq!(book.outbound_cumulative_amount("peer-b"), 150);
    }

    #[test]
    fn outbound_cumulative_amount_survives_acknowledgement() {
        let key = derive_evm_address(&LocalSigner::generate("k").public_key().unwrap());
        let book = book_with_peer("peer-b", &channel_id(1), key);
        let claim = book.record_fulfillment("peer-b", 100, now()).unwrap();

        book.acknowledge_outbound("peer-b", claim.nonce, ClaimAckOutcome::Accepted);

        // Acknowledgement clears `pending`, not the running total this
        // connector committed to -- the whole point of issue #700's
        // "credited" being distinct from "pending".
        assert_eq!(book.pending_claim("peer-b"), None);
        assert_eq!(book.outbound_cumulative_amount("peer-b"), 100);
    }

    #[test]
    fn a_genuinely_signed_claim_from_the_registered_counterparty_is_accepted() {
        let peer_signer = LocalSigner::generate("peer-key");
        let key = derive_evm_address(&peer_signer.public_key().unwrap());
        let book = book_with_peer("peer-b", &channel_id(1), key);
        let claim = sign_claim(&peer_signer, &channel_id(1), 1, 100);

        let outcome = book.accept_inbound(&claim);

        assert_eq!(outcome, ClaimAckOutcome::Accepted);
    }

    #[test]
    fn a_claim_signed_by_the_wrong_key_is_rejected() {
        let key = derive_evm_address(&LocalSigner::generate("peer-key").public_key().unwrap());
        let book = book_with_peer("peer-b", &channel_id(1), key);
        let impostor = LocalSigner::generate("impostor-key");
        let claim = sign_claim(&impostor, &channel_id(1), 1, 100);

        let outcome = book.accept_inbound(&claim);

        assert_eq!(
            outcome,
            ClaimAckOutcome::Rejected(ClaimRejectReason::SignatureInvalid)
        );
    }

    #[test]
    fn a_claim_signed_under_a_different_chain_id_is_rejected() {
        let peer_signer = LocalSigner::generate("peer-key");
        let key = derive_evm_address(&peer_signer.public_key().unwrap());
        let book = book_with_peer("peer-b", &channel_id(1), key);
        // Signed under a genuine digest, but for a different chain id than
        // the channel is registered against -- must not recover to the
        // same signature the registered domain would accept.
        let on_chain_id = parse_channel_id(&channel_id(1)).unwrap();
        let wrong_domain = ChannelDomain {
            chain_id: test_domain().chain_id + 1,
            ..test_domain()
        };
        let proof = evm_proof(on_chain_id, wrong_domain, 1, 100);
        let claim = WireClaim {
            channel_id: channel_id(1),
            nonce: 1,
            cumulative_amount: 100,
            signature: ClaimSignature::Evm(
                peer_signer.sign(&evm_balance_proof_digest(&proof)).unwrap(),
            ),
        };

        let outcome = book.accept_inbound(&claim);

        assert_eq!(
            outcome,
            ClaimAckOutcome::Rejected(ClaimRejectReason::SignatureInvalid)
        );
    }

    #[test]
    fn a_claim_from_an_unregistered_channel_is_rejected_as_unknown_channel() {
        let signer = LocalSigner::generate("k");
        let claim = sign_claim(&signer, &channel_id(1), 1, 100);
        let book = ClaimBook::new(Some(Arc::new(signer)), HashMap::new(), HashMap::new());

        let outcome = book.accept_inbound(&claim);

        assert_eq!(
            outcome,
            ClaimAckOutcome::Rejected(ClaimRejectReason::UnknownChannel)
        );
    }

    #[test]
    fn a_claim_on_a_channel_with_a_counterparty_but_no_domain_is_rejected_as_unknown_channel() {
        let signer = LocalSigner::generate("k");
        let counterparty = derive_evm_address(&signer.public_key().unwrap());
        let claim = sign_claim(&signer, &channel_id(1), 1, 100);
        let mut counterparties = HashMap::new();
        counterparties.insert(channel_id(1), counterparty);
        // Deliberately never calling `set_channel_domain`.
        let book = ClaimBook::new(Some(Arc::new(signer)), HashMap::new(), counterparties);

        let outcome = book.accept_inbound(&claim);

        assert_eq!(
            outcome,
            ClaimAckOutcome::Rejected(ClaimRejectReason::UnknownChannel)
        );
    }

    #[test]
    fn a_second_claim_that_does_not_advance_the_nonce_is_rejected_and_the_watermark_holds() {
        let peer_signer = LocalSigner::generate("peer-key");
        let key = derive_evm_address(&peer_signer.public_key().unwrap());
        let book = book_with_peer("peer-b", &channel_id(1), key);
        let sign =
            |nonce: u64, amount: u64| sign_claim(&peer_signer, &channel_id(1), nonce, amount);

        assert_eq!(
            book.accept_inbound(&sign(5, 500)),
            ClaimAckOutcome::Accepted
        );
        let replay = book.accept_inbound(&sign(5, 999));

        assert_eq!(
            replay,
            ClaimAckOutcome::Rejected(ClaimRejectReason::NonceNotAdvancing)
        );
        // A rejected claim never moves the watermark: the next genuinely
        // advancing claim is still judged against nonce 5 / amount 500.
        assert_eq!(
            book.accept_inbound(&sign(6, 500)),
            ClaimAckOutcome::Accepted
        );
    }

    /// Issue #575's AC5: a claim this connector signs recovers, through
    /// `connector_signer::verify_evm_balance_proof`, to this connector's
    /// own address -- and does not recover under a different domain.
    #[test]
    fn an_outbound_claim_recovers_to_the_signers_own_address_and_not_under_a_different_domain() {
        let signer = LocalSigner::generate("claim-key");
        let own_address = derive_evm_address(&signer.public_key().unwrap());
        let mut outbound_channels = HashMap::new();
        outbound_channels.insert("peer-b".to_string(), channel_id(1));
        let book = ClaimBook::new(Some(Arc::new(signer)), outbound_channels, HashMap::new());
        book.set_channel_domain(channel_id(1), test_domain())
            .unwrap();

        let claim = book.record_fulfillment("peer-b", 100, now()).unwrap();
        let on_chain_id = parse_channel_id(&channel_id(1)).unwrap();
        let proof = evm_proof(
            on_chain_id,
            test_domain(),
            claim.nonce,
            claim.cumulative_amount,
        );

        assert!(verify_evm_balance_proof(
            &proof,
            &claim.signature.to_bytes(),
            &own_address
        ));

        let wrong_domain = ChannelDomain {
            token_network_address: [0xAA; 20],
            ..test_domain()
        };
        let proof_under_wrong_domain = evm_proof(
            on_chain_id,
            wrong_domain,
            claim.nonce,
            claim.cumulative_amount,
        );
        assert!(!verify_evm_balance_proof(
            &proof_under_wrong_domain,
            &claim.signature.to_bytes(),
            &own_address
        ));
    }

    mod redemption {
        use super::*;

        #[test]
        fn no_claim_is_redeemable_before_one_is_accepted() {
            let book = ClaimBook::new(None, HashMap::new(), HashMap::new());
            assert_eq!(book.latest_inbound_claim(&channel_id(1)), None);
        }

        #[test]
        fn the_latest_accepted_claim_is_redeemable_and_carries_its_signature() {
            let peer_signer = LocalSigner::generate("peer-key");
            let key = derive_evm_address(&peer_signer.public_key().unwrap());
            let book = book_with_peer("peer-b", &channel_id(1), key);
            let first = sign_claim(&peer_signer, &channel_id(1), 1, 100);
            assert_eq!(book.accept_inbound(&first), ClaimAckOutcome::Accepted);
            let second = sign_claim(&peer_signer, &channel_id(1), 2, 150);
            assert_eq!(book.accept_inbound(&second), ClaimAckOutcome::Accepted);

            // Only the higher-nonce claim is redeemable -- the superseded
            // first claim is never returned (peer-semantics-pre-868.md §3.4: claims
            // supersede rather than accumulate).
            let redeemable = book.latest_inbound_claim(&channel_id(1)).unwrap();
            assert_eq!(redeemable.nonce, 2);
            assert_eq!(redeemable.cumulative_amount, 150);
            assert_eq!(redeemable.signature, second.signature.to_bytes().to_vec());
        }

        /// Issue #573's own regression: `connector_settlement::Claim` must
        /// carry the nonce its signature covers, or nothing it produces is
        /// redeemable on any real chain -- a chain-side check this test
        /// cannot exercise directly, so it pins the one thing that would
        /// silently regress that guarantee: `latest_inbound_claim` reporting
        /// the accepted claim's own nonce, not a default or dropped one.
        #[test]
        fn the_redeemable_claims_nonce_is_the_one_the_peer_actually_signed() {
            let peer_signer = LocalSigner::generate("peer-key");
            let key = derive_evm_address(&peer_signer.public_key().unwrap());
            let book = book_with_peer("peer-b", &channel_id(1), key);
            let claim = sign_claim(&peer_signer, &channel_id(1), 7, 300);
            assert_eq!(book.accept_inbound(&claim), ClaimAckOutcome::Accepted);

            let redeemable = book.latest_inbound_claim(&channel_id(1)).unwrap();
            assert_eq!(redeemable.nonce, 7);
        }
    }

    #[test]
    fn outbound_channel_id_reports_the_configured_channel_for_a_peer() {
        let book = ClaimBook::new(None, HashMap::new(), HashMap::new());
        book.set_outbound_channel("peer-b", channel_id(1));

        assert_eq!(book.outbound_channel_id("peer-b"), Some(channel_id(1)));
        assert_eq!(book.outbound_channel_id("peer-nowhere"), None);
    }

    mod journal_recovery {
        use super::*;
        use crate::journal::{FileJournal, InMemoryJournal};

        #[test]
        fn a_freshly_configured_journal_has_nothing_to_replay() {
            let mut book = ClaimBook::new(None, HashMap::new(), HashMap::new());
            book.set_journal(Arc::new(InMemoryJournal::new())).unwrap();

            assert_eq!(book.latest_inbound_claim(&channel_id(1)), None);
        }

        /// The acceptance criteria's own scenario: a node killed mid-traffic
        /// recovers its money state by replay, with no manual repair. This
        /// rebuilds a *fresh* `ClaimBook` from the same durable journal a
        /// prior instance wrote to, standing in for a restart, and asserts
        /// both sides of its money state -- what it owes downstream and what
        /// a channel has claimed -- come back exactly as they were.
        #[test]
        fn a_node_restarted_against_the_same_journal_recovers_its_money_state() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("journal.log");
            let signer = Arc::new(LocalSigner::generate("claim-key"));
            let peer_key = LocalSigner::generate("peer-key");
            let out_channel = channel_id(1);
            let in_channel = channel_id(2);

            {
                let mut book = ClaimBook::new(
                    Some(signer.clone() as Arc<dyn Signer>),
                    HashMap::new(),
                    HashMap::new(),
                );
                book.set_outbound_channel("peer-b", out_channel.clone());
                book.set_verification_key(
                    in_channel.clone(),
                    derive_evm_address(&peer_key.public_key().unwrap()),
                );
                book.set_channel_domain(out_channel.clone(), test_domain())
                    .unwrap();
                book.set_channel_domain(in_channel.clone(), test_domain())
                    .unwrap();
                book.set_journal(Arc::new(FileJournal::open(&path).unwrap()))
                    .unwrap();

                // What we owe peer-b: two fulfilments, superseding into one
                // pending claim.
                book.record_fulfillment("peer-b", 100, now());
                book.record_fulfillment("peer-b", 50, now());

                // A claim channel-in sent us.
                let claim = sign_claim(&peer_key, &in_channel, 1, 40);
                assert_eq!(book.accept_inbound(&claim), ClaimAckOutcome::Accepted);
            }

            // A fresh book, backed by the same journal file, standing in for
            // a restarted process -- nothing here was told about the prior
            // instance's in-memory state directly. Channel domains are
            // reconfigured before the journal, exactly like a real restart
            // reloading its static config before replaying its journal.
            let mut restarted = ClaimBook::new(
                Some(signer as Arc<dyn Signer>),
                HashMap::new(),
                HashMap::new(),
            );
            restarted.set_outbound_channel("peer-b", out_channel.clone());
            restarted
                .set_channel_domain(out_channel.clone(), test_domain())
                .unwrap();
            restarted
                .set_channel_domain(in_channel.clone(), test_domain())
                .unwrap();
            restarted
                .set_journal(Arc::new(FileJournal::open(&path).unwrap()))
                .unwrap();

            // The outbound debt to peer-b survived, re-armed with a fresh
            // signature over the same nonce/cumulative amount -- resendable
            // with no manual repair.
            let pending = restarted.pending_claim("peer-b").expect("still pending");
            assert_eq!(pending.nonce, 2);
            assert_eq!(pending.cumulative_amount, 150);
            // The inbound claim's watermark survived too.
            let redeemable = restarted.latest_inbound_claim(&in_channel).unwrap();
            assert_eq!(redeemable.nonce, 1);
            assert_eq!(redeemable.cumulative_amount, 40);
        }

        /// Issue #832's own regression scenario: a journal that names
        /// channel A for `peer_id`, replayed against config that now names
        /// channel B for the same peer (a peer-channel migration's config
        /// edit, applied exactly as the runbook prescribes). The next claim
        /// signed must bind to B at a fresh nonce/amount, not silently carry
        /// A's watermark forward into a claim signed under B's domain --
        /// asserted against the journal itself, since that is what a
        /// restart actually replays, not just the returned `WireClaim`.
        #[test]
        fn a_peer_channel_migration_rebinds_from_config_not_the_replayed_journal() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("journal.log");
            let signer = Arc::new(LocalSigner::generate("claim-key"));
            let channel_a = channel_id(1);
            let channel_b = channel_id(2);

            {
                let mut book = ClaimBook::new(
                    Some(signer.clone() as Arc<dyn Signer>),
                    HashMap::new(),
                    HashMap::new(),
                );
                book.set_outbound_channel("peer-b", channel_a.clone());
                book.set_channel_domain(channel_a.clone(), test_domain())
                    .unwrap();
                book.set_journal(Arc::new(FileJournal::open(&path).unwrap()))
                    .unwrap();

                book.record_fulfillment("peer-b", 100, now()).unwrap();
                book.record_fulfillment("peer-b", 50, now()).unwrap();
            }

            // The premise the migration is applied on top of, asserted
            // rather than assumed: the journal on disk ends on channel A.
            assert_eq!(
                FileJournal::open(&path).unwrap().read_all().unwrap().last(),
                Some(&JournalEntry::OutboundClaimSigned {
                    peer_id: "peer-b".to_string(),
                    channel_id: channel_a,
                    nonce: 2,
                    cumulative_amount: 150,
                })
            );

            // A fresh process, standing in for the restart the migration
            // runbook's step 6 performs: config now names channel B for
            // peer-b, the journal on disk still ends on channel A.
            let mut migrated = ClaimBook::new(
                Some(signer as Arc<dyn Signer>),
                HashMap::new(),
                HashMap::new(),
            );
            migrated.set_outbound_channel("peer-b", channel_b.clone());
            migrated
                .set_channel_domain(channel_b.clone(), test_domain())
                .unwrap();
            migrated
                .set_journal(Arc::new(FileJournal::open(&path).unwrap()))
                .unwrap();

            let claim = migrated.record_fulfillment("peer-b", 10, now()).unwrap();

            assert_eq!(claim.channel_id, channel_b);
            assert_eq!(claim.nonce, 1);
            assert_eq!(claim.cumulative_amount, 10);

            let entries = migrated.journal.read_all().unwrap();
            assert_eq!(
                entries.last(),
                Some(&JournalEntry::OutboundClaimSigned {
                    peer_id: "peer-b".to_string(),
                    channel_id: channel_b,
                    nonce: 1,
                    cumulative_amount: 10,
                })
            );
        }
    }

    /// Issue #710: `ClaimBook`'s peer claim journal group-commits the way
    /// issue #686 already had the client edge do it.
    mod group_commit {
        use super::*;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::{Barrier, Mutex};
        use std::time::Duration;

        /// A [`Journal`] whose first `append_batch` stalls long enough
        /// that every concurrently-enqueuing caller lands in the channel
        /// before it returns -- the deterministic way to prove batching
        /// happens at all, rather than hoping a race resolves the same way
        /// twice on a loaded CI box. Records every batch's size (and every
        /// single-entry `append`, which group commit should never call)
        /// so a test can assert on the shape of what was actually written.
        struct StallingJournal {
            inner: InMemoryJournal,
            stalled_once: AtomicBool,
            batch_sizes: Mutex<Vec<usize>>,
        }

        impl StallingJournal {
            fn new() -> StallingJournal {
                StallingJournal {
                    inner: InMemoryJournal::new(),
                    stalled_once: AtomicBool::new(false),
                    batch_sizes: Mutex::new(Vec::new()),
                }
            }
        }

        impl Journal for StallingJournal {
            fn append(&self, entry: &JournalEntry) -> Result<(), JournalError> {
                self.batch_sizes.lock().expect("lock poisoned").push(1);
                self.inner.append(entry)
            }

            fn append_batch(&self, entries: &[JournalEntry]) -> Result<(), JournalError> {
                if !self.stalled_once.swap(true, Ordering::SeqCst) {
                    // Give every other concurrently-enqueuing caller time
                    // to land in the committer's channel before this (the
                    // first) batch's write returns and the committer loops
                    // back for more -- everything queued while this sleeps
                    // is guaranteed to drain into its successor batch in
                    // one shot.
                    thread::sleep(Duration::from_millis(200));
                }
                self.batch_sizes
                    .lock()
                    .expect("lock poisoned")
                    .push(entries.len());
                self.inner.append_batch(entries)
            }

            fn read_all(&self) -> Result<Vec<JournalEntry>, JournalError> {
                self.inner.read_all()
            }
        }

        /// Issue #710's own claim: concurrent forwards no longer pay one
        /// fsync each under `ClaimBook`'s one journal-file lock. Eight
        /// threads each accept a claim on their own channel at once --
        /// before this issue's fix, `ClaimBook::accept_inbound` called
        /// `Journal::append` directly and every one of those eight would
        /// be its own `append`/fsync; with the fix, the ones that arrive
        /// while the first (stalled) write is in flight share its
        /// successor `append_batch` call.
        #[test]
        fn concurrent_inbound_claims_on_distinct_channels_share_a_batch() {
            const CHANNELS: u8 = 8;
            let peer_key = LocalSigner::generate("peer-key");
            let counterparty = derive_evm_address(&peer_key.public_key().unwrap());

            let mut book = ClaimBook::new(None, HashMap::new(), HashMap::new());
            for n in 1..=CHANNELS {
                book.set_verification_key(channel_id(n), counterparty);
                book.set_channel_domain(channel_id(n), test_domain())
                    .unwrap();
            }
            let journal = Arc::new(StallingJournal::new());
            book.set_journal(journal.clone()).unwrap();
            let book = Arc::new(book);

            let barrier = Arc::new(Barrier::new(CHANNELS as usize));
            let handles: Vec<_> = (1..=CHANNELS)
                .map(|n| {
                    let book = book.clone();
                    let barrier = barrier.clone();
                    let claim = sign_claim(&peer_key, &channel_id(n), 1, 100);
                    thread::spawn(move || {
                        barrier.wait();
                        assert_eq!(book.accept_inbound(&claim), ClaimAckOutcome::Accepted);
                    })
                })
                .collect();
            for handle in handles {
                handle.join().expect("accepting thread panicked");
            }

            let batch_sizes = journal.batch_sizes.lock().expect("lock poisoned");
            assert_eq!(
                batch_sizes.iter().sum::<usize>(),
                CHANNELS as usize,
                "every accepted claim must land in exactly one batch: {batch_sizes:?}"
            );
            assert!(
                batch_sizes.iter().any(|&size| size > 1),
                "expected at least one batch to carry more than one entry \
                 (group commit not amortizing concurrent appends), got {batch_sizes:?}"
            );
            for n in 1..=CHANNELS {
                assert_eq!(
                    book.latest_inbound_claim(&channel_id(n)),
                    Some(connector_settlement::Claim {
                        nonce: 1,
                        cumulative_amount: 100,
                        signature: sign_claim(&peer_key, &channel_id(n), 1, 100)
                            .signature
                            .to_bytes(),
                    })
                );
            }
        }

        /// The send-side mirror of the receive-side test above:
        /// `record_fulfillment` holds a single global outbound-ledger lock
        /// across every peer (issue #710's own "under one lock"), so it is
        /// this path -- not `accept_inbound`'s -- where a fsync held under
        /// the lock would have serialized every forward in the connector,
        /// not just forwards on the same peer.
        #[test]
        fn concurrent_outbound_fulfillments_across_peers_share_a_batch() {
            const PEERS: u8 = 8;
            let signer = Arc::new(LocalSigner::generate("claim-key"));
            let mut outbound_channels = HashMap::new();
            for n in 1..=PEERS {
                outbound_channels.insert(format!("peer-{n}"), channel_id(n));
            }
            let mut book = ClaimBook::new(Some(signer), outbound_channels, HashMap::new());
            for n in 1..=PEERS {
                book.set_channel_domain(channel_id(n), test_domain())
                    .unwrap();
            }
            let journal = Arc::new(StallingJournal::new());
            book.set_journal(journal.clone()).unwrap();
            let book = Arc::new(book);

            let barrier = Arc::new(Barrier::new(PEERS as usize));
            let handles: Vec<_> = (1..=PEERS)
                .map(|n| {
                    let book = book.clone();
                    let barrier = barrier.clone();
                    thread::spawn(move || {
                        barrier.wait();
                        book.record_fulfillment(&format!("peer-{n}"), 100, now())
                            .expect("channel is bound and signed")
                    })
                })
                .collect();
            for handle in handles {
                handle.join().expect("fulfilling thread panicked");
            }

            let batch_sizes = journal.batch_sizes.lock().expect("lock poisoned");
            assert_eq!(
                batch_sizes.iter().sum::<usize>(),
                PEERS as usize,
                "every signed claim must land in exactly one batch: {batch_sizes:?}"
            );
            assert!(
                batch_sizes.iter().any(|&size| size > 1),
                "expected at least one batch to carry more than one entry \
                 (group commit not amortizing concurrent appends), got {batch_sizes:?}"
            );
            for n in 1..=PEERS {
                assert_eq!(book.pending_claim(&format!("peer-{n}")).unwrap().nonce, 1);
            }
        }

        /// A [`Journal`] that parks inside `append_batch` until a test
        /// releases it, so the test can observe the book at exactly the
        /// moment an entry has been enqueued but is not yet durable -- the
        /// window `record_fulfillment` opened when it stopped holding the
        /// outbound lock across the fsync.
        struct GatedJournal {
            inner: InMemoryJournal,
            entered: mpsc::Sender<()>,
            release: Mutex<mpsc::Receiver<()>>,
        }

        impl Journal for GatedJournal {
            fn append(&self, entry: &JournalEntry) -> Result<(), JournalError> {
                self.inner.append(entry)
            }

            fn append_batch(&self, entries: &[JournalEntry]) -> Result<(), JournalError> {
                self.entered.send(()).expect("the test is still watching");
                self.release
                    .lock()
                    .expect("lock poisoned")
                    .recv()
                    .expect("the test releases every batch it gates");
                self.inner.append_batch(entries)
            }

            fn read_all(&self) -> Result<Vec<JournalEntry>, JournalError> {
                self.inner.read_all()
            }
        }

        /// ADR 0005 at the boundary this issue moved: a signed claim is a
        /// bearer instrument -- `Connector::forward` reads `pending_claim`
        /// and ships it -- so it must not be visible until its journal
        /// entry is durable. Before the fix that shape was inverted:
        /// `record_fulfillment` armed `pending` under the lock and *then*
        /// waited for the batch, leaving a window in which a concurrent
        /// forward could transmit a nonce whose entry never reached disk.
        #[test]
        fn a_signed_claim_is_not_visible_until_its_batch_is_durable() {
            let (entered_tx, entered_rx) = mpsc::channel();
            let (release_tx, release_rx) = mpsc::channel();
            let journal = Arc::new(GatedJournal {
                inner: InMemoryJournal::new(),
                entered: entered_tx,
                release: Mutex::new(release_rx),
            });

            let signer = Arc::new(LocalSigner::generate("claim-key"));
            let mut outbound_channels = HashMap::new();
            outbound_channels.insert("peer-a".to_string(), channel_id(1));
            let mut book = ClaimBook::new(Some(signer), outbound_channels, HashMap::new());
            book.set_channel_domain(channel_id(1), test_domain())
                .unwrap();
            book.set_journal(journal.clone()).unwrap();
            let book = Arc::new(book);

            let fulfilling = {
                let book = book.clone();
                thread::spawn(move || book.record_fulfillment("peer-a", 100, now()))
            };

            // The committer is now inside `append_batch` with the entry
            // queued and the outbound lock long since released -- exactly
            // the moment a concurrent `Connector::forward` would read the
            // ledger.
            entered_rx
                .recv_timeout(Duration::from_secs(5))
                .expect("the committer reaches the journal");
            assert_eq!(
                book.pending_claim("peer-a"),
                None,
                "a claim whose journal entry is still in flight must not be transmittable"
            );

            release_tx.send(()).expect("the committer is waiting");
            let claim = fulfilling
                .join()
                .expect("fulfilling thread panicked")
                .expect("channel is bound and signed");
            assert_eq!(claim.nonce, 1);
            assert_eq!(
                book.pending_claim("peer-a").map(|claim| claim.nonce),
                Some(1),
                "the claim is armed once -- and only once -- its entry is durable"
            );
        }

        /// A [`Journal`] whose writes can be made to fail and work again
        /// in place, for the rollback the issue requires ("preserve
        /// rollback on a batch that cannot be made durable"). In place
        /// matters: `ClaimBook::set_journal` rebuilds the whole book from
        /// the journal it is handed, so swapping in a broken one would
        /// reset exactly the state a rollback test is trying to observe.
        struct BreakableJournal {
            inner: InMemoryJournal,
            broken: AtomicBool,
        }

        impl BreakableJournal {
            fn new(broken: bool) -> BreakableJournal {
                BreakableJournal {
                    inner: InMemoryJournal::new(),
                    broken: AtomicBool::new(broken),
                }
            }

            fn set_broken(&self, broken: bool) {
                self.broken.store(broken, Ordering::SeqCst);
            }

            fn error() -> JournalError {
                JournalError::Corrupt("this journal cannot write".to_string())
            }
        }

        impl Journal for BreakableJournal {
            fn append(&self, entry: &JournalEntry) -> Result<(), JournalError> {
                if self.broken.load(Ordering::SeqCst) {
                    return Err(BreakableJournal::error());
                }
                self.inner.append(entry)
            }

            fn append_batch(&self, entries: &[JournalEntry]) -> Result<(), JournalError> {
                if self.broken.load(Ordering::SeqCst) {
                    return Err(BreakableJournal::error());
                }
                self.inner.append_batch(entries)
            }

            fn read_all(&self) -> Result<Vec<JournalEntry>, JournalError> {
                self.inner.read_all()
            }
        }

        fn signing_book(peer_id: &str, journal: Arc<BreakableJournal>) -> ClaimBook {
            let signer = Arc::new(LocalSigner::generate("claim-key"));
            let mut outbound_channels = HashMap::new();
            outbound_channels.insert(peer_id.to_string(), channel_id(1));
            let mut book = ClaimBook::new(Some(signer), outbound_channels, HashMap::new());
            book.set_channel_domain(channel_id(1), test_domain())
                .unwrap();
            book.set_journal(journal).unwrap();
            book
        }

        /// A fulfilment whose batch cannot be made durable leaves the peer
        /// exactly as it found it: no claim returned, no claim armed, and
        /// -- the part that matters after a restart -- the ledger's nonce
        /// and cumulative amount back where they were, so the next
        /// fulfilment re-signs this nonce instead of skipping past it into
        /// a sequence the journal has no record of.
        #[test]
        fn a_batch_that_cannot_be_made_durable_rolls_the_outbound_ledger_back() {
            let journal = Arc::new(BreakableJournal::new(false));
            let book = signing_book("peer-a", journal.clone());

            let first = book
                .record_fulfillment("peer-a", 100, now())
                .expect("a working journal signs a claim");
            assert_eq!((first.nonce, first.cumulative_amount), (1, 100));

            journal.set_broken(true);
            assert_eq!(
                book.record_fulfillment("peer-a", 100, now()),
                None,
                "a claim that could not be journaled was never signed"
            );
            assert_eq!(
                book.pending_claim("peer-a").map(|claim| claim.nonce),
                Some(1),
                "the rolled-back fulfilment must not disturb the claim already armed"
            );
            assert_eq!(
                book.outbound_cumulative_amount("peer-a"),
                100,
                "the ledger is back at the last durably journaled advance"
            );

            // And the sequence resumes at the nonce the failure rolled
            // back to, not one past it.
            journal.set_broken(false);
            let resumed = book
                .record_fulfillment("peer-a", 100, now())
                .expect("a working journal signs a claim");
            assert_eq!((resumed.nonce, resumed.cumulative_amount), (2, 200));
            assert_eq!(
                journal.read_all().unwrap().len(),
                2,
                "only the two durable advances are on record"
            );
        }

        /// The very first fulfilment on a peer has no earlier state to go
        /// back to, so rolling it back means removing the ledger outright
        /// -- and the peer must look untouched to `views`, not like a peer
        /// carrying an advance nothing recorded.
        #[test]
        fn a_first_fulfilment_that_cannot_be_journaled_leaves_no_ledger_behind() {
            let journal = Arc::new(BreakableJournal::new(true));
            let book = signing_book("peer-a", journal);

            assert_eq!(book.record_fulfillment("peer-a", 100, now()), None);
            assert_eq!(book.pending_claim("peer-a"), None);
            assert_eq!(book.outbound_cumulative_amount("peer-a"), 0);
            assert!(
                book.views().is_empty(),
                "a rolled-back first fulfilment leaves nothing for the operator surface to see"
            );
        }

        /// The inbound half of the same rule: an acceptance that cannot be
        /// journaled is not acknowledged (peer-semantics-pre-868.md §6.3) and its
        /// watermark is restored, so the payer's retransmission of the
        /// very same claim is judged fresh rather than bouncing off its
        /// own unrecorded ghost.
        #[test]
        fn a_batch_that_cannot_be_made_durable_rolls_the_inbound_watermark_back() {
            let peer_key = LocalSigner::generate("peer-key");
            let counterparty = derive_evm_address(&peer_key.public_key().unwrap());
            let journal = Arc::new(BreakableJournal::new(true));
            let mut book = ClaimBook::new(None, HashMap::new(), HashMap::new());
            book.set_verification_key(channel_id(1), counterparty);
            book.set_channel_domain(channel_id(1), test_domain())
                .unwrap();
            book.set_journal(journal.clone()).unwrap();

            let claim = sign_claim(&peer_key, &channel_id(1), 1, 100);
            assert_eq!(
                book.accept_inbound(&claim),
                ClaimAckOutcome::NotSent,
                "a claim this node could not record is not acknowledged, neither accepted \
                 nor rejected"
            );
            assert_eq!(
                book.latest_inbound_claim(&channel_id(1)),
                None,
                "an acceptance with no journal line behind it is not in the projection either"
            );

            // The retransmission -- byte-identical, as §6.3 expects -- is
            // accepted once the journal works again, which it could not be
            // if the failed acceptance had left its watermark standing.
            journal.set_broken(false);
            assert_eq!(book.accept_inbound(&claim), ClaimAckOutcome::Accepted);
        }
    }

    /// Issue #732: the peer semantics's Solana half, both directions -- inbound
    /// verification (#732/#738) and outbound signing (#742, added
    /// alongside the `outbound` submodule below).
    mod solana {
        use super::*;
        use connector_signer::{solana_balance_proof_message, LocalEd25519Signer};
        use ed25519_dalek::{Keypair, PublicKey, SecretKey, Signer as DalekSigner};

        /// A deterministic ed25519 keypair -- no RNG, so a failure here
        /// reproduces exactly.
        fn keypair(seed: u8) -> Keypair {
            let secret = SecretKey::from_bytes(&[seed; 32]).expect("32 bytes is a valid seed");
            let public = PublicKey::from(&secret);
            Keypair { secret, public }
        }

        fn base58(bytes: &[u8; 32]) -> String {
            bs58::encode(bytes).into_string()
        }

        /// A Solana channel account id, distinct per `n`.
        fn account(n: u8) -> [u8; 32] {
            let mut bytes = [0xA0; 32];
            bytes[31] = n;
            bytes
        }

        /// A book that accepts claims on `account(n)` signed by `signer`.
        fn book_with_solana_channel(n: u8, signer: &Keypair) -> ClaimBook {
            let book = ClaimBook::new(None, HashMap::new(), HashMap::new());
            book.set_solana_channel(
                base58(&account(n)),
                &base58(&signer.public.to_bytes()),
                "US517G5965aydkZ46HS38QLi7UQiSojurfbQfKCELFx",
            )
            .expect("a 32-byte base58 account and key");
            book
        }

        /// A claim on `account(n)`, genuinely signed by `signer` over the
        /// 96-byte balance-proof message -- exactly what a peer's own
        /// Solana signing path produces.
        fn sign_solana(signer: &Keypair, n: u8, nonce: u64, amount: u64) -> WireClaim {
            let message = solana_balance_proof_message(&[7u8; 32], &account(n), nonce, amount);
            WireClaim {
                channel_id: base58(&account(n)),
                nonce,
                cumulative_amount: amount,
                signature: ClaimSignature::Solana(signer.sign(&message).to_bytes()),
            }
        }

        #[test]
        fn a_genuine_solana_claim_from_the_registered_counterparty_is_accepted() {
            let peer = keypair(1);
            let book = book_with_solana_channel(1, &peer);

            assert_eq!(
                book.accept_inbound(&sign_solana(&peer, 1, 1, 100)),
                ClaimAckOutcome::Accepted
            );
        }

        #[test]
        fn a_solana_claim_signed_by_the_wrong_key_is_rejected() {
            let peer = keypair(1);
            let impostor = keypair(2);
            let book = book_with_solana_channel(1, &peer);

            assert_eq!(
                book.accept_inbound(&sign_solana(&impostor, 1, 1, 100)),
                ClaimAckOutcome::Rejected(ClaimRejectReason::SignatureInvalid)
            );
        }

        /// The claim's own `signerPublicKey` is dropped at the carriage
        /// and this book consults only its own record, so re-registering
        /// the channel to a different key invalidates every claim the old
        /// key ever signed -- the property that makes the self-declared
        /// field worthless to a forger.
        #[test]
        fn re_registering_the_counterparty_invalidates_the_old_keys_claims() {
            let peer = keypair(1);
            let claim = sign_solana(&peer, 1, 1, 100);
            let book = book_with_solana_channel(1, &peer);
            book.set_solana_channel(
                base58(&account(1)),
                &base58(&keypair(2).public.to_bytes()),
                "US517G5965aydkZ46HS38QLi7UQiSojurfbQfKCELFx",
            )
            .unwrap();

            assert_eq!(
                book.accept_inbound(&claim),
                ClaimAckOutcome::Rejected(ClaimRejectReason::SignatureInvalid)
            );
        }

        /// A genuine signature over *another* account's message is not a
        /// claim on this one: the account bytes open the signed message,
        /// and they come from this book's record of the channel the claim
        /// names, never from the claim.
        #[test]
        fn a_signature_over_a_different_channel_account_does_not_verify() {
            let peer = keypair(1);
            let book = book_with_solana_channel(1, &peer);
            let elsewhere = sign_solana(&peer, 2, 1, 100);
            let relabelled = WireClaim {
                channel_id: base58(&account(1)),
                ..elsewhere
            };

            assert_eq!(
                book.accept_inbound(&relabelled),
                ClaimAckOutcome::Rejected(ClaimRejectReason::SignatureInvalid)
            );
        }

        #[test]
        fn a_solana_claim_on_an_unregistered_account_is_rejected_as_unknown_channel() {
            let peer = keypair(1);
            let book = ClaimBook::new(None, HashMap::new(), HashMap::new());

            assert_eq!(
                book.accept_inbound(&sign_solana(&peer, 1, 1, 100)),
                ClaimAckOutcome::Rejected(ClaimRejectReason::UnknownChannel)
            );
        }

        /// **Chain confusion, both directions.** Each scheme reads only
        /// its own map, so neither a Solana signature on an EVM-registered
        /// channel nor an EVM signature on a Solana-registered one is ever
        /// checked against the other chain's record. Both are
        /// `unknown_channel`, and neither can be made to pass by
        /// relabelling.
        #[test]
        fn a_claim_carrying_the_other_chains_signature_scheme_is_unknown_channel() {
            let peer = keypair(1);
            let evm_signer = LocalSigner::generate("peer-key");
            let evm_key = derive_evm_address(&evm_signer.public_key().unwrap());

            // An EVM-registered channel, reached by a Solana signature.
            let evm_book = book_with_peer("peer-b", &channel_id(1), evm_key);
            let solana_on_evm_channel = WireClaim {
                channel_id: channel_id(1),
                ..sign_solana(&peer, 1, 1, 100)
            };
            assert_eq!(
                evm_book.accept_inbound(&solana_on_evm_channel),
                ClaimAckOutcome::Rejected(ClaimRejectReason::UnknownChannel)
            );

            // A Solana-registered channel, reached by an EVM signature.
            let solana_book = book_with_solana_channel(1, &peer);
            let evm_on_solana_channel = WireClaim {
                channel_id: base58(&account(1)),
                ..sign_claim(&evm_signer, &channel_id(1), 1, 100)
            };
            assert_eq!(
                solana_book.accept_inbound(&evm_on_solana_channel),
                ClaimAckOutcome::Rejected(ClaimRejectReason::UnknownChannel)
            );
        }

        /// Verify, advance, acknowledge -- the three things #732's
        /// definition of done asks for, in one pass.
        #[test]
        fn an_accepted_solana_claim_advances_the_ledger_and_a_replay_is_refused() {
            let peer = keypair(1);
            let book = book_with_solana_channel(1, &peer);
            let first = sign_solana(&peer, 1, 1, 100);
            let second = sign_solana(&peer, 1, 2, 250);

            assert_eq!(book.accept_inbound(&first), ClaimAckOutcome::Accepted);
            assert_eq!(book.accept_inbound(&second), ClaimAckOutcome::Accepted);

            // The watermark moved, and the ledger reports the *latest*
            // claim -- with its 64 ed25519 bytes intact, not padded to
            // EVM's 65.
            let view = book
                .views()
                .into_iter()
                .find(|view| view.channel_id == base58(&account(1)))
                .expect("the channel is known");
            assert_eq!((view.nonce, view.cumulative_amount), (2, 250));
            let latest = book
                .latest_inbound_claim(&base58(&account(1)))
                .expect("a claim was accepted");
            assert_eq!((latest.nonce, latest.cumulative_amount), (2, 250));
            assert_eq!(latest.signature.len(), 64);

            // Replaying either is refused rather than re-accepted.
            assert_eq!(
                book.accept_inbound(&first),
                ClaimAckOutcome::Rejected(ClaimRejectReason::NonceNotAdvancing)
            );
            assert_eq!(
                book.accept_inbound(&second),
                ClaimAckOutcome::Rejected(ClaimRejectReason::NonceNotAdvancing)
            );
        }

        /// A fresher nonce that *lowers* the cumulative amount is refused
        /// -- the same rule the EVM side is held to, since it is
        /// `connector_domain::validate_claim`'s rule and not a per-chain
        /// one. (A nonce that advances while the amount merely holds
        /// steady is legal there and stays legal here: it moves no value,
        /// so it takes none back either.)
        #[test]
        fn a_solana_claim_lowering_the_cumulative_amount_is_refused() {
            let peer = keypair(1);
            let book = book_with_solana_channel(1, &peer);
            book.accept_inbound(&sign_solana(&peer, 1, 1, 100));

            assert_eq!(
                book.accept_inbound(&sign_solana(&peer, 1, 2, 99)),
                ClaimAckOutcome::Rejected(ClaimRejectReason::AmountNotAdvancing)
            );
            assert_eq!(
                book.accept_inbound(&sign_solana(&peer, 1, 2, 100)),
                ClaimAckOutcome::Accepted
            );
        }

        /// An account or key that is not base58 of exactly 32 bytes is
        /// refused where channels are configured -- never padded,
        /// truncated or hashed into shape, the same rule
        /// `set_channel_domain` holds an EVM id to.
        #[test]
        fn set_solana_channel_refuses_anything_that_is_not_a_32_byte_account() {
            let book = ClaimBook::new(None, HashMap::new(), HashMap::new());
            let good = base58(&account(1));

            assert!(book
                .set_solana_channel(
                    &good,
                    "not base58 0OIl",
                    "US517G5965aydkZ46HS38QLi7UQiSojurfbQfKCELFx"
                )
                .is_err());
            assert!(book
                .set_solana_channel(
                    bs58::encode([0u8; 31]).into_string(),
                    &good,
                    "US517G5965aydkZ46HS38QLi7UQiSojurfbQfKCELFx"
                )
                .is_err());
            assert!(book
                .set_solana_channel(
                    &good,
                    &bs58::encode([0u8; 33]).into_string(),
                    "US517G5965aydkZ46HS38QLi7UQiSojurfbQfKCELFx"
                )
                .is_err());
            assert!(book
                .set_solana_channel("", &good, "US517G5965aydkZ46HS38QLi7UQiSojurfbQfKCELFx")
                .is_err());

            // Nothing was registered by any of those, so a genuine claim
            // still finds no channel.
            assert_eq!(
                book.accept_inbound(&sign_solana(&keypair(1), 1, 1, 100)),
                ClaimAckOutcome::Rejected(ClaimRejectReason::UnknownChannel)
            );
        }

        /// A Solana channel's accepted claims survive a restart through
        /// the same ADR 0005 journal an EVM channel's do, with the 64-byte
        /// signature recovered intact.
        #[test]
        fn a_solana_watermark_rebuilds_from_the_journal() {
            let peer = keypair(1);
            let journal = Arc::new(InMemoryJournal::new());
            let mut book = book_with_solana_channel(1, &peer);
            book.set_journal(journal.clone()).unwrap();
            book.accept_inbound(&sign_solana(&peer, 1, 4, 400));

            let mut rebuilt = book_with_solana_channel(1, &peer);
            rebuilt.set_journal(journal).unwrap();

            // The replay is refused against the rebuilt watermark, which
            // is the only thing that makes a restart safe.
            assert_eq!(
                rebuilt.accept_inbound(&sign_solana(&peer, 1, 4, 400)),
                ClaimAckOutcome::Rejected(ClaimRejectReason::NonceNotAdvancing)
            );
            assert_eq!(
                rebuilt.accept_inbound(&sign_solana(&peer, 1, 5, 500)),
                ClaimAckOutcome::Accepted
            );
        }

        /// **The bidirectional ledger** (#262's riskiest surface, and the
        /// reason a second chain doubles it): one `ClaimBook` holds live
        /// outbound state *and* live inbound state at the same time, and
        /// after #732 the two can be on different chains. Interleaving an
        /// EVM outbound ledger with a Solana inbound watermark must leave
        /// each exactly where it would have been alone -- the failure mode
        /// is lost money, not a wrong number.
        #[test]
        fn an_evm_outbound_ledger_and_a_solana_inbound_watermark_do_not_disturb_each_other() {
            let peer = keypair(1);
            let evm_key = derive_evm_address(&LocalSigner::generate("k").public_key().unwrap());
            let book = book_with_peer("peer-b", &channel_id(1), evm_key);
            book.set_solana_channel(
                base58(&account(1)),
                &base58(&peer.public.to_bytes()),
                "US517G5965aydkZ46HS38QLi7UQiSojurfbQfKCELFx",
            )
            .unwrap();

            book.record_fulfillment("peer-b", 100, now()).unwrap();
            assert_eq!(
                book.accept_inbound(&sign_solana(&peer, 1, 1, 70)),
                ClaimAckOutcome::Accepted
            );
            let outbound = book.record_fulfillment("peer-b", 50, now()).unwrap();
            assert_eq!(
                book.accept_inbound(&sign_solana(&peer, 1, 2, 90)),
                ClaimAckOutcome::Accepted
            );

            // Outbound: still EVM-signed, still the running total, and
            // untouched by anything that arrived on the Solana channel.
            assert_eq!(book.outbound_cumulative_amount("peer-b"), 150);
            assert!(matches!(outbound.signature, ClaimSignature::Evm(_)));
            assert_eq!(book.pending_claim("peer-b"), Some(outbound));
            assert_eq!(book.outbound_channel_id("peer-b"), Some(channel_id(1)));

            // Inbound: the Solana watermark is the Solana claims' own, and
            // the EVM channel has no watermark at all -- no claim arrived
            // on it.
            assert_eq!(
                book.accept_inbound(&sign_solana(&peer, 1, 2, 90)),
                ClaimAckOutcome::Rejected(ClaimRejectReason::NonceNotAdvancing)
            );
            assert!(book
                .views()
                .iter()
                .any(|view| view.channel_id == base58(&account(1))
                    && view.direction == crate::operator_view::ClaimDirection::Inbound
                    && view.nonce == 2));
        }

        proptest::proptest! {
            /// The watermark rule is the same rule on both chains
            /// (`connector_domain::validate_claim`, not a per-chain
            /// copy): an arbitrary sequence of genuinely signed Solana
            /// claims is accepted exactly when the nonce strictly
            /// advances and the cumulative amount does not go backwards,
            /// and the high-water mark tracks what was *accepted* --
            /// never a value only a rejected claim carried.
            #[test]
            fn only_strictly_advancing_solana_claims_are_ever_accepted(
                steps in proptest::collection::vec((1u64..8, 0u64..400), 1..24)
            ) {
                let peer = keypair(1);
                let book = book_with_solana_channel(1, &peer);
                let mut accepted: Option<(u64, u64)> = None;

                for (nonce, amount) in steps {
                    let outcome = book.accept_inbound(&sign_solana(&peer, 1, nonce, amount));
                    let advances = match accepted {
                        None => true,
                        Some((high_nonce, high_amount)) => {
                            nonce > high_nonce && amount >= high_amount
                        }
                    };
                    proptest::prop_assert_eq!(
                        outcome == ClaimAckOutcome::Accepted,
                        advances,
                        "nonce {} amount {} against watermark {:?}",
                        nonce,
                        amount,
                        accepted
                    );
                    if advances {
                        accepted = Some((nonce, amount));
                    }
                }

                match accepted {
                    None => proptest::prop_assert!(
                        book.latest_inbound_claim(&base58(&account(1))).is_none()
                    ),
                    Some((nonce, amount)) => {
                        let latest = book
                            .latest_inbound_claim(&base58(&account(1)))
                            .expect("a claim was accepted");
                        proptest::prop_assert_eq!(latest.nonce, nonce);
                        proptest::prop_assert_eq!(latest.cumulative_amount, u128::from(amount));
                    }
                }
            }

            /// A signature is only ever accepted for the exact
            /// `(account, nonce, amount)` triple it covers: perturbing any
            /// one of the three after signing is a forgery, whatever the
            /// watermark would otherwise have said.
            #[test]
            fn a_solana_signature_never_covers_a_field_it_did_not_sign(
                nonce in 1u64..1000,
                amount in 1u64..1_000_000,
                nonce_delta in 1u64..50,
                amount_delta in 1u64..50,
            ) {
                let peer = keypair(1);
                let book = book_with_solana_channel(1, &peer);
                let genuine = sign_solana(&peer, 1, nonce, amount);

                let tampered_nonce = WireClaim { nonce: nonce + nonce_delta, ..genuine.clone() };
                let tampered_amount = WireClaim {
                    cumulative_amount: amount + amount_delta,
                    ..genuine.clone()
                };

                proptest::prop_assert_eq!(
                    book.accept_inbound(&tampered_nonce),
                    ClaimAckOutcome::Rejected(ClaimRejectReason::SignatureInvalid)
                );
                proptest::prop_assert_eq!(
                    book.accept_inbound(&tampered_amount),
                    ClaimAckOutcome::Rejected(ClaimRejectReason::SignatureInvalid)
                );
                // ...and the genuine one still lands, so no rejection
                // above moved the watermark.
                proptest::prop_assert_eq!(
                    book.accept_inbound(&genuine),
                    ClaimAckOutcome::Accepted
                );
            }
        }

        /// Issue #742: the other direction. `ClaimBook` could verify a
        /// Solana peer claim since #732/#738 but never sign one --
        /// `record_fulfillment` fell straight through to `evm_proof` and
        /// refused (via `channel_domains.get(&channel_id)?`) any channel
        /// that was only ever registered as Solana. These tests mirror the
        /// outer `tests` module's EVM outbound fixture set (`sign_claim`,
        /// `book_with_peer`, `no_claim_is_recorded_*`,
        /// `an_outbound_claim_recovers_to_the_signers_own_address_*`, the
        /// journal-replay regression) one for one.
        mod outbound {
            use super::*;

            /// A book that signs outbound claims to `peer_id` on Solana
            /// channel `n` with the identity derived from `seed` -- the
            /// Solana counterpart of the outer module's `book_with_peer`.
            /// Re-derives the signer from `seed` a second time to read back
            /// its own public key, since [`LocalEd25519Signer`] holds its
            /// key pair privately rather than exposing it for cloning.
            fn book_with_solana_peer(peer_id: &str, n: u8, seed: [u8; 32]) -> ClaimBook {
                let mut outbound_channels = HashMap::new();
                outbound_channels.insert(peer_id.to_string(), base58(&account(n)));
                let mut book = ClaimBook::new(None, outbound_channels, HashMap::new());
                book.set_solana_signer(Arc::new(
                    LocalEd25519Signer::from_secret_bytes(seed).expect("32 bytes is a valid seed"),
                ));
                let public_key = LocalEd25519Signer::from_secret_bytes(seed)
                    .expect("32 bytes is a valid seed")
                    .public_key();
                book.set_solana_channel(
                    base58(&account(n)),
                    &base58(&public_key),
                    "US517G5965aydkZ46HS38QLi7UQiSojurfbQfKCELFx",
                )
                .expect("a 32-byte base58 account and key");
                book
            }

            #[test]
            fn no_outbound_solana_claim_is_recorded_without_a_solana_signer() {
                let mut outbound_channels = HashMap::new();
                outbound_channels.insert("peer-b".to_string(), base58(&account(1)));
                let book = ClaimBook::new(None, outbound_channels, HashMap::new());
                book.set_solana_channel(
                    base58(&account(1)),
                    &base58(&keypair(1).public.to_bytes()),
                    "US517G5965aydkZ46HS38QLi7UQiSojurfbQfKCELFx",
                )
                .unwrap();

                assert!(book.record_fulfillment("peer-b", 100, now()).is_none());
            }

            #[test]
            fn no_outbound_solana_claim_is_recorded_for_a_channel_with_no_binding_configured() {
                let mut outbound_channels = HashMap::new();
                outbound_channels.insert("peer-b".to_string(), base58(&account(1)));
                let mut book = ClaimBook::new(None, outbound_channels, HashMap::new());
                book.set_solana_signer(Arc::new(LocalEd25519Signer::generate()));

                assert!(book.record_fulfillment("peer-b", 100, now()).is_none());
            }

            #[test]
            fn recording_a_fulfillment_arms_a_pending_solana_claim_with_nonce_one() {
                let book = book_with_solana_peer("peer-b", 1, [5u8; 32]);

                let claim = book.record_fulfillment("peer-b", 100, now()).unwrap();

                assert_eq!(claim.channel_id, base58(&account(1)));
                assert_eq!(claim.nonce, 1);
                assert_eq!(claim.cumulative_amount, 100);
                assert!(matches!(claim.signature, ClaimSignature::Solana(_)));
                assert_eq!(book.pending_claim("peer-b"), Some(claim));
            }

            /// A second fulfilment before the first claim drains supersedes
            /// it with a fresher nonce and a higher cumulative amount --
            /// the same rule the EVM ledger is held to, since it is
            /// `OutboundLedger`'s rule and not a per-chain one.
            #[test]
            fn a_second_fulfillment_supersedes_the_first_pending_solana_claim() {
                let book = book_with_solana_peer("peer-b", 1, [5u8; 32]);

                book.record_fulfillment("peer-b", 100, now()).unwrap();
                let second = book.record_fulfillment("peer-b", 50, now()).unwrap();

                assert_eq!((second.nonce, second.cumulative_amount), (2, 150));
                assert_eq!(book.pending_claim("peer-b"), Some(second));
            }

            /// Issue #742's own acceptance criteria, mirroring #575's AC5
            /// for EVM: a Solana claim this connector signs recovers,
            /// through `connector_signer::verify_solana_balance_proof`, to
            /// this connector's own ed25519 identity -- and not to a
            /// different key, including the counterparty's own (a claim
            /// this connector signs is never checked against the *peer's*
            /// key, the same asymmetry `set_solana_channel`'s doc draws
            /// between "who signs" and "who is accepted from").
            #[test]
            fn an_outbound_solana_claim_recovers_to_the_signers_own_public_key_and_not_a_different_one(
            ) {
                let seed = [9u8; 32];
                let book = book_with_solana_peer("peer-b", 1, seed);
                let own_public_key = LocalEd25519Signer::from_secret_bytes(seed)
                    .unwrap()
                    .public_key();
                let counterparty = keypair(1);

                let claim = book.record_fulfillment("peer-b", 100, now()).unwrap();

                assert!(verify_solana_balance_proof(
                    &[7u8; 32],
                    &account(1),
                    claim.nonce,
                    claim.cumulative_amount,
                    &claim.signature.to_bytes(),
                    &own_public_key,
                ));
                assert!(!verify_solana_balance_proof(
                    &[7u8; 32],
                    &account(1),
                    claim.nonce,
                    claim.cumulative_amount,
                    &claim.signature.to_bytes(),
                    &counterparty.public.to_bytes(),
                ));
            }

            /// The acceptance criteria's own restart scenario, ported from
            /// the outer module's `a_node_restarted_against_the_same_journal_recovers_its_money_state`:
            /// a pending outbound Solana claim survives a restart, re-armed
            /// with a fresh signature over the same nonce/cumulative amount
            /// from the same journal entry -- no chain discriminator was
            /// added to `JournalEntry::OutboundClaimSigned` for this,
            /// because the channel id alone (base58 of 32 bytes, disjoint
            /// from every EVM shape `parse_channel_id` accepts) already
            /// tells `rebuild_from` which map, and therefore which chain,
            /// governs replay.
            #[test]
            fn an_outbound_solana_claim_survives_a_restart_and_is_resigned_from_the_journal() {
                let seed = [3u8; 32];
                let peer_id = "peer-b";
                let journal = Arc::new(InMemoryJournal::new());

                let mut book = book_with_solana_peer(peer_id, 1, seed);
                book.set_journal(journal.clone()).unwrap();
                book.record_fulfillment(peer_id, 100, now());
                book.record_fulfillment(peer_id, 50, now());

                let mut restarted = book_with_solana_peer(peer_id, 1, seed);
                restarted.set_journal(journal).unwrap();

                let pending = restarted.pending_claim(peer_id).expect("still pending");
                assert_eq!((pending.nonce, pending.cumulative_amount), (2, 150));
                assert!(matches!(pending.signature, ClaimSignature::Solana(_)));
                let own_public_key = LocalEd25519Signer::from_secret_bytes(seed)
                    .unwrap()
                    .public_key();
                assert!(verify_solana_balance_proof(
                    &[7u8; 32],
                    &account(1),
                    pending.nonce,
                    pending.cumulative_amount,
                    &pending.signature.to_bytes(),
                    &own_public_key,
                ));
            }

            /// The mirror image of the outer module's
            /// `an_evm_outbound_ledger_and_a_solana_inbound_watermark_do_not_disturb_each_other`:
            /// a Solana outbound ledger and an EVM inbound watermark, on
            /// the same book, must leave each other exactly where they
            /// would have been alone -- #262's riskiest surface, now
            /// exercised in both directions.
            #[test]
            fn a_solana_outbound_ledger_and_an_evm_inbound_watermark_do_not_disturb_each_other() {
                let evm_peer_signer = LocalSigner::generate("peer-key");
                let evm_key = derive_evm_address(&evm_peer_signer.public_key().unwrap());

                let book = book_with_solana_peer("peer-b", 1, [6u8; 32]);
                book.set_verification_key(channel_id(2), evm_key);
                book.set_channel_domain(channel_id(2), test_domain())
                    .unwrap();

                let outbound = book.record_fulfillment("peer-b", 100, now()).unwrap();
                assert_eq!(
                    book.accept_inbound(&sign_claim(&evm_peer_signer, &channel_id(2), 1, 70)),
                    ClaimAckOutcome::Accepted
                );
                let outbound2 = book.record_fulfillment("peer-b", 50, now()).unwrap();
                assert_eq!(
                    book.accept_inbound(&sign_claim(&evm_peer_signer, &channel_id(2), 2, 90)),
                    ClaimAckOutcome::Accepted
                );

                // Outbound: still Solana-signed, still the running total.
                assert_eq!(book.outbound_cumulative_amount("peer-b"), 150);
                assert!(matches!(outbound2.signature, ClaimSignature::Solana(_)));
                assert_eq!(book.pending_claim("peer-b"), Some(outbound2));
                assert_eq!(outbound.channel_id, base58(&account(1)));

                // Inbound: the EVM watermark is the EVM claims' own, and
                // the Solana channel has no watermark -- no claim arrived
                // on it.
                assert_eq!(
                    book.accept_inbound(&sign_claim(&evm_peer_signer, &channel_id(2), 2, 90)),
                    ClaimAckOutcome::Rejected(ClaimRejectReason::NonceNotAdvancing)
                );
                assert!(book
                    .views()
                    .iter()
                    .any(|view| view.channel_id == channel_id(2)
                        && view.direction == crate::operator_view::ClaimDirection::Inbound
                        && view.nonce == 2));
            }
        }
    }
}
