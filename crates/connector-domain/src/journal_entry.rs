//! The alphabet of a durable journal (ADR 0005, issue #424). Pure, no I/O
//! -- the journal itself (a file, in `connector-runtime`) is an
//! infrastructure concern.
//!
//! A balances projection folded from these entries lived here too, until
//! its last reader went with the retired payout ledger (ADR 0075 decision 7,
//! issue #1381): every book now replays its own entries into its own state.
//! `InboundFulfillmentRecorded`, which backed the exposure/ceiling
//! accounting ADR 0033 (issue #882) retired, and `OutboundClaimSigned`,
//! which only the retired payout ledger wrote, stay in the alphabet only so
//! an older journal still decodes.

/// One durably-recorded fact about money state (ADR 0005).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JournalEntry {
    /// Historical entry kind, no longer produced (ADR 0075 decision 7, issue
    /// #1381): this connector signed an outbound `toon-channel` claim owed
    /// to `peer_id` on `channel_id` (the retired
    /// `ClaimBook::record_fulfillment`). Kept so a journal an older build
    /// wrote still decodes; nothing replays it into any state.
    OutboundClaimSigned {
        peer_id: String,
        channel_id: String,
        nonce: u64,
        cumulative_amount: u64,
    },
    /// A signed claim on `channel_id` was verified and accepted, advancing
    /// that channel's watermark to `nonce`/`cumulative_amount`
    /// (peer-semantics-pre-868.md §3.4; replayed by `ClaimBook` and written
    /// by the client edge's claim gate). `signature` is
    /// carried through opaque (chain- and scheme-specific verification
    /// already happened before this entry is ever appended) and durably
    /// retained rather than discarded once accepted: on-chain redemption
    /// (issue #425) needs the actual claim, not just its watermark, and per
    /// ADR 0005 what is signed is exactly what this journal exists to keep.
    /// `cumulative_amount` is `u128` (ADR 0074 decision 3, amended
    /// 2026-09-30 #1429), matching an EVM voucher's own width; a pre-widening
    /// journal's `u64`-range decimal text still decodes unchanged.
    InboundClaimAccepted {
        channel_id: String,
        nonce: u64,
        cumulative_amount: u128,
        signature: Vec<u8>,
    },
    /// Historical entry kind, no longer produced (ADR 0031, ADR 0033, issue
    /// #882): a packet arriving on `channel_id` fulfilled for `amount`,
    /// extending this connector's exposure to that channel's counterparty
    /// until a covering claim was accepted -- the credit-window accounting
    /// this connector kept before every peer PREPARE carried its own
    /// covering claim. Kept in the alphabet, not removed, so a journal a
    /// pre-#882 build already wrote still decodes; replay folds it into
    /// nothing.
    InboundFulfillmentRecorded { channel_id: String, amount: u64 },
    /// `channel_id`'s watermark was durably reset because this connector
    /// discovered the chain no longer vouches for it -- settled,
    /// deallocated, or otherwise gone (issue #977). Written only by
    /// `connector_client_edge::ClientClaimGate::reset_watermark`, into the
    /// client edge's own journal -- a channel's deterministic on-chain
    /// address means a reopened channel reuses the exact key its settled
    /// predecessor's watermark was filed under, and without this entry a
    /// reopened channel would inherit that predecessor's watermark forever,
    /// charging its payer again for units already settled on chain (or, at
    /// the limit, refusing every claim it could ever present). Folds into
    /// nothing in the peer semantics's own book, which this entry kind is
    /// never written to (see the client edge's own
    /// journal file, kept separate from the peer semantics's for exactly this
    /// reason) -- it is in this shared alphabet only so both journals'
    /// entries decode through one enum, matching every other entry kind
    /// here.
    InboundClaimWatermarkReset { channel_id: String },
    /// `channel_id`'s watermark was durably rolled back to `nonce`/
    /// `cumulative_amount` because the PREPARE the claim that reached that
    /// watermark covered is now known never to have been carried: a
    /// client-priced forwarded route (ADR 0028) admits the client's claim
    /// before learning whether the next hop will fulfil it, and the next
    /// hop's own terminal reject (F06 after a covered retry, T01
    /// unreachable) is discoverable only after admission (issue #1012).
    /// Written only by `connector_client_edge::ClientClaimGate::roll_back`,
    /// into the client edge's own journal, immediately after the
    /// `InboundClaimAccepted` entry it undoes -- never speculatively, and
    /// never for a claim a later admission has already superseded.
    ///
    /// Unlike `InboundClaimAccepted`, whose replay folds by componentwise
    /// max (this module's own doc), a replay of this entry SETS the
    /// watermark directly, matching `InboundClaimWatermarkReset`: it exists
    /// specifically to move a watermark down, which a legitimately
    /// advancing claim never does, so folding it by max would silently
    /// undo the very thing it records. Folds into nothing here: like
    /// `InboundClaimWatermarkReset`, this entry kind is written only to the
    /// client edge's own journal, never the peer wire's.
    InboundClaimRolledBack {
        channel_id: String,
        nonce: u64,
        cumulative_amount: u128,
    },
    /// The client edge accepted its first voucher on the x402
    /// `batch-settlement` channel `channel_id` (ADR 0074): the canonical
    /// `evm:0x…` or `solana:…` key its watermark is filed under.
    /// `presentation` is what restores the channel to its settlement
    /// backend after a restart, carried opaque as `InboundClaimAccepted`'s
    /// signature is: on EVM the channel's `ChannelConfig`, which the
    /// contract stores only as a hash and a voucher signs only as that
    /// hash, so without it a voucher already accepted could never be
    /// `claim`ed; on Solana nothing, since the channel account holds every
    /// field.
    ///
    /// Written only by `connector_client_edge::ClientClaimGate`, into the
    /// client edge's own journal, in the same batch as -- and immediately
    /// before -- the `InboundClaimAccepted` of the channel's first accepted
    /// voucher. It also marks the key as a batch-settlement channel's, which
    /// the gate's `TokenNetwork`-shaped sweep must never judge. Folds into
    /// nothing here, like the other client-edge-only kinds.
    BatchChannelAdmitted {
        channel_id: String,
        presentation: Vec<u8>,
    },
    /// This node built an outbound x402 `batch-settlement` channel toward a
    /// counterparty, and is about to send its opening transaction (ADR 0075
    /// decision 8). `channel_id` is the canonical `evm:0x…` or `solana:…`
    /// key; `record` is the settlement port's `OutboundChannelRecord`, as
    /// its own bytes: on EVM the whole `ChannelConfig`, `salt` included, and
    /// on Solana the payer-signed `open`. Nothing else can rebuild either.
    ///
    /// Written **before** the opening transaction is sent, so a crash
    /// between the two leaves a record a retried open resumes rather than a
    /// channel with this node's deposit in it that it no longer knows it
    /// holds. Neither signed by a counterparty nor irreversible, and
    /// journaled anyway, for the reason `BatchChannelAdmitted` is.
    ///
    /// Written only by `connector_runtime::OutboundChannels`, into its own
    /// journal file. Folds into nothing here, like the client-edge kinds.
    OutboundChannelOpening { channel_id: String, record: Vec<u8> },
    /// The outbound channel `channel_id` is on chain: its opening
    /// transaction landed, or a retried open found that it had.
    OutboundChannelOpened { channel_id: String },
    /// The outbound channel `channel_id` was never opened and never will
    /// be: on Solana, its `open`'s blockhash expired with nothing on chain.
    /// A later open toward the same counterparty builds a fresh channel.
    OutboundChannelAbandoned { channel_id: String },
    /// This node signed a voucher for `cumulative_amount` on its outbound
    /// channel `channel_id`, and journaled it before handing it to anyone
    /// (ADR 0005: what is signed is what the journal keeps). Superseding,
    /// like the voucher: the highest is this node's watermark on the
    /// channel, which a restart must never let go backwards.
    OutboundVoucherSigned {
        channel_id: String,
        cumulative_amount: u128,
    },
    /// This node's signed watermark on its outbound channel `channel_id` is
    /// now `cumulative_amount`, which may be LOWER than a voucher it signed
    /// before: the receiver reported, after a forward ended in a reject,
    /// that it holds no voucher that high, so the packet the voucher paid
    /// for was never carried and is not owed (ADR 0075, issue #1446).
    /// Written only by `connector_runtime::OutboundChannels::set_watermark`,
    /// into its own journal file, and only after a later voucher has been
    /// found not to have been signed since.
    ///
    /// Unlike `OutboundVoucherSigned`, whose replay folds by max, a replay
    /// of this entry SETS the watermark, as `InboundClaimRolledBack` does
    /// for the inbound side: it exists to move a watermark down, which a
    /// max-fold would silently undo.
    OutboundWatermarkSet {
        channel_id: String,
        cumulative_amount: u128,
    },
    /// A zk edge granted an allowance (ADR 0080 decisions 3 and 11): the
    /// note state `nullifier` is spent, the request proof that spent it is
    /// the evidence-file record whose SHA-256 is `evidence_hash`, and these
    /// are the allowance's terms. Written, and fsynced, after the evidence
    /// record and before the grant is answered, so no nullifier is ever
    /// spent twice across a crash.
    ///
    /// `allowance_id` is the opaque id the zk edge minted (32 lowercase
    /// hex); field elements are 32 bytes little-endian, the evidence file's
    /// encoding. The allowance's watermark is carried by the
    /// `InboundClaim*` kinds under the key `zk-note:<allowance_id>`, so an
    /// allowance claim rolls back exactly as a voucher does; this entry is
    /// what marks that key as an allowance's rather than a retired
    /// `toon-channel`'s. `commitment` is the proof's re-randomised balance
    /// commitment, which the successor is charged from.
    ///
    /// `subsidy` is set on a subsidised allowance (ADR 0080 decision 14,
    /// #1581): the subsidy period it was granted in and its per-period
    /// nullifier. The per-period nullifier is journaled with the grant, so
    /// it is durable before the grant is answered and a restart still
    /// refuses it; the period is the budget the tier is reserved against.
    ///
    /// Written only by `connector_client_edge::ClientClaimGate`, into the
    /// client edge's own journal.
    AllowanceGranted {
        allowance_id: String,
        nullifier: [u8; 32],
        evidence_hash: [u8; 32],
        tier: u64,
        expires_at_ms: u64,
        session_key_x: [u8; 32],
        session_key_is_odd: bool,
        commitment_x: [u8; 32],
        commitment_y: [u8; 32],
        subsidy: Option<AllowanceSubsidy>,
    },
    /// A zk edge stopped an allowance taking claims (ADR 0080 decision 4):
    /// on its session key's signed close, on a claim reaching its budget, or
    /// at its expiry. Written, and fsynced, when the close starts -- before
    /// a signed close is answered -- so a restart brings the allowance back
    /// closing, never open, and the boot sweep finishes it. Written at most
    /// once per allowance, before its [`JournalEntry::AllowanceClosed`].
    AllowanceClosing { allowance_id: String },
    /// A zk edge closed an allowance and fixed its charge Δ (ADR 0080
    /// decisions 4 and 11, #1523): the allowance's final watermark, written
    /// once every forward in flight on it had resolved, and never
    /// rewritten. Written, and fsynced, before the evidence file's close
    /// record and before the successor is signed, so a restart can finish
    /// a close but never charge it differently.
    ///
    /// For a subsidised allowance Δ is the subsidy budget's charge, not the
    /// note's: the note is charged nothing.
    AllowanceClosed { allowance_id: String, charge: u64 },
    /// The successor state a zk edge signed for a closed allowance (ADR
    /// 0080 decision 11, #1523): zk_toon's state message over the charged
    /// commitment and the next anchor, and the state key's signature over
    /// it. Field elements and the signature's two halves are 32 bytes
    /// little-endian. Written once, after [`JournalEntry::AllowanceClosed`];
    /// a successor fetch only ever reads it back.
    AllowanceSuccessorSigned {
        allowance_id: String,
        commitment_x: [u8; 32],
        commitment_y: [u8; 32],
        anchor: [u8; 32],
        signature_rx: [u8; 32],
        signature_s: [u8; 32],
    },
}

/// What a subsidised allowance adds to its [`JournalEntry::AllowanceGranted`]
/// (ADR 0080 decision 14).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllowanceSubsidy {
    /// The subsidy period the grant reserved its tier in.
    pub period: u64,
    /// The proof's per-period nullifier, 32 bytes little-endian.
    pub period_nullifier: [u8; 32],
}
