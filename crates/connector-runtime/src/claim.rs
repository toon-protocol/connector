//! Per-peering-relation claim exchange (ADR 0004, ADR 0005, ADR 0024,
//! `docs/protocol/peer-semantics-pre-868.md` §3, issue #423): verifying and
//! watermarking a `toon-channel` claim a peer sends, and the wire shape a
//! claim travels in. The nonce/watermark rule itself lives in
//! `connector_domain::validate_claim`; this module is the in-memory
//! bookkeeping around it, plus the chain-specific digest a claim's
//! signature covers (issue #575: `connector_signer::evm_balance_proof_digest`,
//! the same EIP-712 `BalanceProof` digest `TokenNetwork.sol` verifies).
//!
//! Signing a claim this connector owes lived here too, as
//! `ClaimBook::record_fulfillment`, until its last caller -- the client
//! payout ledger -- moved to vouchers on an outbound x402 channel (ADR 0075
//! decision 7, issue #1381). Every outbound claim is now a voucher, signed
//! through `crate::OutboundChannels`.

use std::collections::{HashMap, HashSet};
use std::sync::{mpsc, Arc, RwLock};
use std::thread;

use arc_swap::ArcSwap;

use connector_domain::{advance_watermark, validate_claim, ClaimError, JournalEntry, Watermark};
use connector_signer::{
    verify_evm_balance_proof, verify_solana_balance_proof, Address, Ed25519Signer, EvmBalanceProof,
    Signature, Signer,
};
use thiserror::Error;

use crate::journal::{InMemoryJournal, Journal, JournalError};
use crate::operator_view::ClaimView;

/// A claim as it travels the wire (peer-semantics-pre-868.md §3.5): a channel
/// identifier, a nonce, a cumulative amount, and a signature. `channel_id`
/// is expected to already name the channel's on-chain `bytes32` (see
/// [`ClaimBook::set_channel_domain`]) -- this type itself carries it as an
/// opaque `String` (as it always has) so the wire encoding below is
/// unchanged; it is [`ClaimBook`] that refuses to accept a claim
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

/// What covers a PREPARE this node sends a peer (ADR 0042: a packet
/// carries its claim), as the [`crate::PeerTransport`] port hands it to a
/// carriage.
///
/// Two schemes ride the peer wire until #1380 retires the first: a
/// `toon-channel` [`WireClaim`], which the carriage renders itself, and an
/// x402 **voucher** (ADR 0075 decision 6), which arrives rendered -- the
/// client edge's own voucher JSON (`client-edge-spec.md` §1.3), exactly the
/// bytes the receiving half parses. A packet that moves no value on an x402
/// peering carries no voucher (ADR 0075 decision 5) and, where it needs the
/// peer role, the voucher claim-state **challenge** instead: a separate
/// slot on both carriages (`peer-carriage-spec.md` §1.4), because a
/// challenge is not a claim and moves nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Covering {
    /// A `toon-channel` claim: the claim slot, rendered by the carriage.
    Claim(WireClaim),
    /// An x402 `batch-settlement` voucher's JSON: the claim slot, verbatim.
    Voucher(String),
    /// A voucher claim-state challenge's JSON: the peer-role challenge
    /// slot, verbatim.
    Challenge(String),
}

impl Covering {
    /// The `toon-channel` claim this covering is, if it is one.
    #[must_use]
    pub fn claim(&self) -> Option<&WireClaim> {
        match self {
            Covering::Claim(claim) => Some(claim),
            Covering::Voucher(_) | Covering::Challenge(_) => None,
        }
    }

    /// The `toon-channel` claim this covering is, by value.
    #[must_use]
    pub fn into_claim(self) -> Option<WireClaim> {
        match self {
            Covering::Claim(claim) => Some(claim),
            Covering::Voucher(_) | Covering::Challenge(_) => None,
        }
    }
}

impl From<WireClaim> for Covering {
    fn from(claim: WireClaim) -> Covering {
        Covering::Claim(claim)
    }
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
    /// its claims are verified under (issue #575/#566): `accept_inbound`
    /// builds the [`EvmBalanceProof`] a claim's signature must recover
    /// over from it.
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
    /// `channel_id` -> the highest nonce/amount accepted on it so far.
    inbound_watermarks: Arc<RwLock<HashMap<String, Watermark>>>,
    /// Durable record of every claim accepted (ADR 0005, issue #424).
    /// Defaults to [`InMemoryJournal`] -- a node that never configures a
    /// real one keeps working, just without surviving a restart.
    journal: Arc<dyn Journal>,
    /// Issue #710: batches concurrent [`ClaimBook::accept_inbound`] journal
    /// appends into one
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
        let inbound_watermarks = Arc::new(RwLock::new(HashMap::new()));
        let committer = GroupCommitter::spawn(CommitState {
            journal: journal.clone(),
            inbound_watermarks: inbound_watermarks.clone(),
        });
        ClaimBook {
            signer,
            solana_signer: None,
            outbound_channels: ArcSwap::from_pointee(outbound_channels),
            channel_domains: ArcSwap::from_pointee(HashMap::new()),
            counterparties: ArcSwap::from_pointee(counterparties),
            solana_channels: ArcSwap::from_pointee(HashMap::new()),
            inbound_watermarks,
            journal,
            committer,
        }
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
    /// every entry already in it to rebuild this book's inbound watermarks
    /// (ADR 0005, issue #424: "rebuilt from the journal on start"). Takes
    /// `&mut self` for the same reason `set_signer` does -- called only while
    /// a `Connector` is still being built.
    ///
    /// A journal written before ADR 0075 (issue #1381) may still carry
    /// `OutboundClaimSigned` entries from the retired payout ledger; nothing
    /// signs one any more, and they replay as nothing.
    pub fn set_journal(&mut self, journal: Arc<dyn Journal>) -> Result<(), JournalError> {
        let entries = journal.read_all()?;
        let inbound_watermarks = Arc::new(RwLock::new(Self::rebuild_from(&entries)));
        // A fresh committer bound to the real journal and to the state the
        // replay just rebuilt -- the one spawned in `new` was writing to
        // the default `InMemoryJournal`, and holds an `Arc` to the map this
        // method is about to replace. Dropping the old `GroupCommitter` here
        // drops its sender, which ends that thread's loop; called only while
        // a `Connector` is still being built, so no commit can be in flight
        // on it.
        self.committer = GroupCommitter::spawn(CommitState {
            journal: journal.clone(),
            inbound_watermarks: inbound_watermarks.clone(),
        });
        self.journal = journal;
        self.inbound_watermarks = inbound_watermarks;
        Ok(())
    }

    /// Fold `entries` into fresh inbound watermarks -- the pure replay
    /// [`ClaimBook::set_journal`] drives.
    fn rebuild_from(entries: &[JournalEntry]) -> HashMap<String, Watermark> {
        let mut inbound_watermarks: HashMap<String, Watermark> = HashMap::new();
        for entry in entries {
            if let JournalEntry::InboundClaimAccepted {
                channel_id,
                nonce,
                cumulative_amount,
                ..
            } = entry
            {
                inbound_watermarks.insert(
                    channel_id.clone(),
                    advance_watermark(*nonce, *cumulative_amount),
                );
            }
        }
        inbound_watermarks
    }

    /// The channel this connector claims against when it owes `peer_id`,
    /// if one is configured (issue #424: identifies which channel an
    /// outgoing frame to `peer_id` is claimed against, independent of
    /// whether a claim happens to be pending right now).
    pub fn outbound_channel_id(&self, peer_id: &str) -> Option<String> {
        self.outbound_channels.load().get(peer_id).cloned()
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
                // outside it (issue #710, mirroring issue #686's own
                // `ClientClaimGate::admit`): a claim
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
                scheme: crate::operator_view::ClaimScheme::ToonChannel,
            })
            .collect()
    }
}

/// The most entries one journal batch carries -- a bound on the buffer a
/// commit builds, not a tuning knob, mirroring
/// `connector_client_edge::claim_gate`'s own `GROUP_COMMIT_MAX_BATCH`: the
/// committer drains only what is already queued, so a batch is naturally
/// sized by how many entries arrived during the previous batch's fsync.
const GROUP_COMMIT_MAX_BATCH: usize = 4096;

/// What the state this connector reads live still owes a queued
/// [`JournalEntry`] once its batch resolves: the advance to *undo* if it is
/// not durable. One variant per journal entry [`ClaimBook`] writes.
enum CommitEffect {
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
    /// `accept_inbound`'s entire durability contract:
    /// synchronous, and it always returns. A sender dropped without an
    /// answer is a committer that died mid-batch, which is not durable
    /// either.
    fn durable(self) -> bool {
        matches!(self.durable.recv(), Ok(true))
    }
}

/// Everything the committer thread touches: the journal it writes and the
/// live state a failed batch undoes. Grouped so [`ClaimBook::set_journal`]
/// cannot rebind one and forget the other.
struct CommitState {
    journal: Arc<dyn Journal>,
    inbound_watermarks: Arc<RwLock<HashMap<String, Watermark>>>,
}

/// Issue #710's group commit for [`ClaimBook`]'s peer claim journal: a
/// dedicated thread that drains every [`PendingCommit`] queued since the
/// last batch and writes them as one [`Journal::append_batch`] -- one
/// write, one fsync -- instead of the one-fsync-per-entry `Journal::append`
/// calls `accept_inbound` made directly before this issue. The mechanism is issue #686's, adopted rather than
/// reinvented (see `connector_client_edge::claim_gate::GroupCommitter`):
/// concurrent forwards queue behind one another only for the microseconds
/// it takes to enqueue, not for a whole fsync each.
///
/// A dedicated OS thread rather than a task because
/// [`Journal::append_batch`] blocks on disk I/O and this loop exists to do
/// nothing else; it exits when the book is dropped (the sender goes away)
/// and takes nothing with it.
///
/// **A batch that cannot be made durable is rolled back** -- the half of
/// ADR 0005 that holding the append under the caller's write lock used to
/// give for free, and moving the fsync out from under that lock has to buy
/// back explicitly. A failed batch leaves inbound watermarks promising a
/// durable record that does not exist. So this thread retakes the write
/// lock those advances were decided under, drains whatever else was queued
/// against the now-unrecorded state (it could only have landed in this
/// batch or a later one, and there is no later one until this loop comes
/// back around), restores every touched channel to its state before the
/// *earliest* failed entry, and only then releases the waiters -- who
/// answer *not acknowledged*.
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

/// Undo every advance a failed batch recorded, plus every advance queued
/// behind it -- see [`GroupCommitter`]'s doc for why both. `resolved` is
/// extended with whatever is drained, so its waiters are refused too.
fn roll_back(
    state: &CommitState,
    receiver: &mpsc::Receiver<QueuedCommit>,
    resolved: &mut Vec<(CommitEffect, mpsc::Sender<bool>)>,
) {
    // The lock for the whole unwind, so nothing can be decided against
    // state that is about to be rolled back.
    let mut watermarks = state
        .inbound_watermarks
        .write()
        .expect("inbound watermarks lock poisoned");
    while let Ok((pending, done)) = receiver.try_recv() {
        resolved.push((pending.effect, done));
    }
    // First failed effect per channel wins: effects are in advance order,
    // so its `previous` is the last state with a durable record behind it.
    let mut restored_channels: HashSet<&str> = HashSet::new();
    for (effect, _) in resolved.iter() {
        let CommitEffect::InboundClaimAccepted {
            channel_id,
            previous,
        } = effect;
        if restored_channels.insert(channel_id.as_str()) {
            restore_watermark(&mut watermarks, channel_id, *previous);
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
    use connector_signer::{derive_evm_address, evm_balance_proof_digest, LocalSigner};

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
    /// exactly as a peer signs one.
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

            assert_eq!(book.inbound_watermark(&channel_id(1)), None);
        }

        /// The acceptance criteria's own scenario: a node killed mid-traffic
        /// recovers its money state by replay, with no manual repair. This
        /// rebuilds a *fresh* `ClaimBook` from the same durable journal a
        /// prior instance wrote to, standing in for a restart, and asserts a
        /// channel's watermark comes back exactly as it was. The journal also
        /// carries an `OutboundClaimSigned` line from the retired payout
        /// ledger (ADR 0075 decision 7), which replays as nothing.
        #[test]
        fn a_node_restarted_against_the_same_journal_recovers_its_money_state() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("journal.log");
            let peer_key = LocalSigner::generate("peer-key");
            let in_channel = channel_id(2);
            let book_over = |path: &std::path::Path| {
                let mut book = ClaimBook::new(None, HashMap::new(), HashMap::new());
                book.set_verification_key(
                    in_channel.clone(),
                    derive_evm_address(&peer_key.public_key().unwrap()),
                );
                book.set_channel_domain(in_channel.clone(), test_domain())
                    .unwrap();
                book.set_journal(Arc::new(FileJournal::open(path).unwrap()))
                    .unwrap();
                book
            };

            FileJournal::open(&path)
                .unwrap()
                .append(&JournalEntry::OutboundClaimSigned {
                    peer_id: "peer-b".to_string(),
                    channel_id: channel_id(1),
                    nonce: 2,
                    cumulative_amount: 150,
                })
                .unwrap();
            {
                let book = book_over(&path);
                let claim = sign_claim(&peer_key, &in_channel, 1, 40);
                assert_eq!(book.accept_inbound(&claim), ClaimAckOutcome::Accepted);
            }

            let restarted = book_over(&path);
            assert_eq!(
                restarted.inbound_watermark(&in_channel),
                Some(advance_watermark(1, 40))
            );
            assert_eq!(
                restarted.accept_inbound(&sign_claim(&peer_key, &in_channel, 1, 40)),
                ClaimAckOutcome::Rejected(ClaimRejectReason::NonceNotAdvancing),
                "the replayed watermark refuses the claim it already holds"
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
                    book.inbound_watermark(&channel_id(n)),
                    Some(advance_watermark(1, 100))
                );
            }
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
                book.inbound_watermark(&channel_id(1)),
                None,
                "an acceptance with no journal line behind it leaves no watermark"
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
        use connector_signer::solana_balance_proof_message;
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

            // The watermark moved to the *latest* claim.
            let view = book
                .views()
                .into_iter()
                .find(|view| view.channel_id == base58(&account(1)))
                .expect("the channel is known");
            assert_eq!((view.nonce, view.cumulative_amount), (2, 250));

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

                proptest::prop_assert_eq!(
                    book.inbound_watermark(&base58(&account(1))),
                    accepted.map(|(nonce, amount)| advance_watermark(nonce, amount))
                );
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
    }
}
