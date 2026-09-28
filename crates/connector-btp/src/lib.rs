//! The BTP frame codec and session framing (RFC-0023), transport-neutral and
//! role-neutral (issue #713, ADR 0027).
//!
//! This crate is *only* the wire: how a `type`/`requestId`/body frame is
//! encoded and decoded, the `protocolData` list every frame type shares, the
//! session-scoped outbound `requestId` allocator and the demux table that
//! correlates the RESPONSE/ERROR answering a request this connector
//! originated. It knows nothing about claims, gates, routes, prices, refusal
//! taxonomies or terms — those are the *policy* of whichever pipeline mounts
//! a carriage on top of it (`connector-client-edge` today; the peer carriage
//! of ADR 0027 / issue #676 next), and this crate deliberately cannot see
//! them: it depends on no other connector crate.
//!
//! **One grammar, not two.** ADR 0027 says "one codec, two roles", and this
//! crate is what makes that structural rather than aspirational. There is
//! exactly one implementation of every frame type here. The "deployed
//! `@toon-protocol/client` dialect" and "RFC-0023's full symmetric grammar"
//! are not two grammars needing two codecs: since issue #702 the dialect *is*
//! RFC-23's grammar, and what distinguishes the deployed client is only which
//! subset of frames it happens to send — MESSAGE, answered by RESPONSE or
//! ERROR, never a TRANSFER and never a server-originated request. That subset
//! is a property of the *caller*, expressed by which functions a carriage
//! calls: the client edge answers and never originates, while a peer carriage
//! will originate through [`BtpSessionHandle`]. Nothing about the bytes
//! differs, so nothing about the bytes is parameterised — adding a second
//! implementation of one frame type, keyed on a role, is precisely how the
//! dialect drifted from its spec the first time.
//!
//! **The vectors are the contract** (ADR 0021): [`frame`]'s tests pin the
//! exact bytes `@toon-protocol/client`'s `serializeBtpMessage` /
//! `parseBtpMessage` produce and accept, against a live deployed client. They
//! moved here verbatim with the code they pin. Prose is not the thing to
//! conform to; those bytes are.
//!
//! What is deliberately *not* here: the session read loop. Ordering — that
//! claims on one session are judged strictly sequentially in arrival order,
//! with only the post-admission tail overlapping — is a property of the
//! pipeline that reads frames and decides what is order-sensitive, not of the
//! codec, and it lives with that pipeline (see
//! `connector-client-edge`'s `btp` module).

mod frame;
mod session;

pub use frame::{
    decode_frame, encode_error, encode_message, encode_response, encode_transfer, BtpDecodeError,
    BtpFrame, CarriageNames, ProtocolData, ACCUMULATED_COST_HEADER, ACCUMULATED_COST_PROTOCOL,
    AUTH_PROTOCOL, BTP_ERROR, BTP_MESSAGE, BTP_RESPONSE, BTP_TRANSFER, CARRIAGE_NAMES,
    CLAIM_ACK_HEADER, CLAIM_ACK_PROTOCOL, CLAIM_HEADER, CLAIM_PROTOCOL, CONTENT_TYPE_TEXT,
    FLUSH_REQUESTED_HEADER, PAYMENT_REQUIRED_HEADER, PAYMENT_REQUIRED_PROTOCOL,
    PAYOUT_CLAIM_PROTOCOL, PEER_CHALLENGE_HEADER, PEER_CHALLENGE_PROTOCOL,
};
pub use session::{
    reply, BtpSessionHandle, OriginateError, OutboundRequests, SessionGone, OUTBOUND_ANSWER_TIMEOUT,
};
