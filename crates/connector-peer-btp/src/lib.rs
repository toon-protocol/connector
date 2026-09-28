//! The **BTP peer carriage** (`docs/protocol/peer-carriage-spec.md`,
//! ADR 0027, issue #727): peering with another connector over RFC-0023
//! frames on a `wss://` websocket, in both directions -- [`dial`] a peer's
//! endpoint, [`accept`] a session a peer dialed into us.
//!
//! # What this crate is, and what it deliberately is not
//!
//! It is the **carriage**: where the bytes ride. It is not the semantics.
//! Voucher exchange, fees and the refusal taxonomy are `peer-semantics-pre-868.md` §3--§6's and live above the
//! [`connector_runtime::PeerTransport`] port, unchanged by which wire
//! carried them. This crate maps §3's table onto frames and back, and
//! nothing else:
//!
//! | Concept | BTP |
//! | ------- | --- |
//! | PREPARE | MESSAGE (type 6), OER PREPARE in `ilpPacket` |
//! | FULFILL / REJECT | RESPONSE (type 1) under that `requestId` |
//! | piggybacked voucher | `payment-channel-claim` entry, raw UTF-8 JSON ([`claim_json`]) |
//! | **FLUSH** | **TRANSFER (type 7)**: `amount` = the claim's new cumulative, claim in `payment-channel-claim`, **no `ilpPacket`**. No longer sent (ADR 0075, #1380); an arriving one gets an empty RESPONSE |
//! | CLAIM_ACK | `claim-ack` entry on the RESPONSE that already answers the claim-bearing frame ([`ack`]) |
//! | `accumulatedCost` | `toon-accumulated-cost` entry on a REJECT ([`fields`]) |
//! | x402 greeting | `payment-required` entry on the `F06` REJECT an uncovered PREPARE gets ([`price_gate`], [`fields`]) |
//!
//! There is no row for a peer credential, and there was one: the `auth`
//! entry used to carry a `{peerId, secret}` shared secret. ADR 0060 deleted
//! it — the voucher already in the table above (or, on a packet that moves
//! no value, the peer-role challenge, ADR 0075 decision 5) is what proves
//! the peering, so
//! the `auth` entry is the client edge's again and this carriage neither
//! sends one nor reads one.
//!
//! # Three things this crate must not do, and how it cannot
//!
//! 1. **Fork the frame grammar.** Every frame is encoded and decoded by
//!    [`connector_btp`], the codec extracted in issue #713 precisely so the
//!    peer carriage and the client edge cannot drift. There is no encoder
//!    here. A peer uses RFC-23's *full* grammar (it originates, and it
//!    sends TRANSFER) while the deployed client sends a narrower subset --
//!    that difference is caller-side, expressed by which functions each
//!    carriage calls, never by a flag on the codec.
//! 2. **Re-decide role.** [`connector_peer_auth::decide_voucher_role`] owns
//!    §1.2's P2/P3 rule, and [`role_gate::decide_frame`] is the one place in this
//!    workspace that joins it to the receiving half's verdict on a voucher
//!    or a peer-role challenge (ADR 0075 decision 5, #1380) --
//!    called by this carriage, by the HTTP one, and by the client edge's
//!    shared front door, so one rule runs on every arrival. What this crate
//!    owns is what a *frame* adds and that crate cannot see: which bytes on
//!    the wire are the voucher.
//! 3. **Fork the claim.** §4's claim JSON *is* the client edge's claim
//!    JSON, parsed by the client edge's own structural validator (spec I4).
//!    See [`claim_json`].
//!
//! # Ordering (§7.1)
//!
//! Identical to the client edge's, and reusing its mechanism rather than a
//! peer-specific one: **vouchers on one session are judged strictly
//! sequentially in arrival order**, inline on the session task, so vouchers
//! sent in order on one socket cannot race each other into
//! `amount_not_advancing`. Only the post-admission tail -- routing, the
//! downstream round trip, writing the RESPONSE -- overlaps, bounded by the
//! same `btp_session_window` (#688) whose absence was the measured
//! ~125--150 events/s admission wall.
//!
//! # What is not here
//!
//! The ILP-over-HTTP carriage (issue #728) and the paired
//! `peer_carriage` vectors (issue #729). Where this crate names a §3 field
//! it uses the constant [`connector_btp`] declares for it, so the HTTP
//! carriage's header twin is added beside that one declaration (spec I2)
//! rather than beside a second copy here.

pub mod accept;
pub mod ack;
pub mod challenge_json;
pub mod claim_json;
pub mod dial;
pub mod fields;
pub mod price_gate;
pub mod role_gate;
pub mod ws;

pub use accept::{PeerAcceptPolicy, PeerCarriageState, PeerSession};
pub use challenge_json::{ChallengeDecodeError, PeerRoleChallenge};
pub use claim_json::{ClaimDecodeError, PeerClaimDomain, PresentedPeerClaim};
pub use dial::{decode_answer, BtpPeerTransport, DialError, PeerAnswer, PeerDialer, PeerRelation};
pub use price_gate::{ClaimEnforcementPolicy, PaymentRequired};
pub use role_gate::{FrameEvidence, VoucherCheck, VoucherEvidence};
pub use ws::TungsteniteDialer;
