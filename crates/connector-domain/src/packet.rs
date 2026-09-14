//! ILPv4 packet types and their wire encoding: **RFC-0027's semantics in
//! TOON's own encoding, which is not byte-compatible with it** (ADR 0063).
//!
//! A packet these types produce will not decode in a conforming ILPv4
//! implementation, and one from a conforming implementation will not decode
//! here. Four things differ from RFC-0027 §Packet Format, and nothing else
//! does:
//!
//! | RFC-0027                                                | This codec                                     |
//! | ------------------------------------------------------- | ---------------------------------------------- |
//! | Outer type-length wrapper: `type` then a VarOctetString | Type byte, then fields inline -- no wrapper    |
//! | `amount` is a fixed `UInt64` (8 bytes)                  | `encode_var_uint` -- a VarUInt                 |
//! | `expiresAt` is a 17-byte Interledger Timestamp          | 19-byte GeneralizedTime, `YYYYMMDDHHMMSS.fffZ` |
//! | `executionCondition`, a 32-byte `UInt256`               | Gone (issue #1269); a one-byte `greeting` flag |
//!
//! The fourth is not an encoding quirk like the first three -- it removes a
//! field RFC-0027 requires and states a decision that field never bought
//! anything a forwarding hop was paid to check (issue #1269 / ADR 0069).
//! Everything else here is RFC-0027's: the three type bytes (12, 13, 14), the
//! remaining field order and meanings, `Fulfill.fulfillment` and its relation
//! to a request's shared secret (ADR 0019), and the `F`/`T`/`R` error
//! taxonomy. The OER primitives the fields are built from are RFC-0030's,
//! tightened by ADR 0023 -- see `oer.rs`.
//!
//! **The bytes are pinned, not merely described.**
//! `vectors/wire-vectors.json`'s `peer_carriage.prepare.http_body_hex` is a
//! complete PREPARE in this dialect and `peer_carriage.fulfill_ack_accepted` /
//! `reject_with_cost` carry the other two; `vectors/README.md` walks the
//! PREPARE byte by byte. ADR 0021 makes those the cross-repo contract
//! `toon-client`, `rig` and `swap` are held to, so a change to this file that
//! does not regenerate them fails `cargo test --workspace`.
//!
//! ## Why the dialect exists, and why it stays
//!
//! These types were ported byte-for-byte from the TypeScript prototype's
//! `packages/shared/src/types/ilp.ts` and `packages/shared/src/encoding/oer.ts`
//! -- a **historical** citation: ADR 0017 retired that connector and the path
//! no longer exists in this repository. That encoder had already diverged from
//! RFC-0027, so porting it faithfully reproduced the divergence.
//!
//! The comment that stood here until ADR 0063 drew a false conclusion from that
//! true premise: it said the port was so "a real ILPv4-over-HTTP client
//! (RFC-0035) can address this connector's client edge". It does not follow
//! from porting an encoder that was never conformant, nobody checked it, and
//! three independent readings of this codebase took it at face value. Byte
//! compatibility would not buy that property in any case -- ADR 0018 makes
//! `data` a gift wrap sealed to the terminating connector's identity key and
//! ADR 0019 derives the fulfilment from the secret inside it, so a conforming
//! sender with perfect bytes still cannot build a packet this connector will
//! pay out on. ADR 0063 has the other three reasons and the cost of changing
//! it.

use chrono::{DateTime, Utc};

use crate::address::is_valid_ilp_address;
use crate::error::PacketError;
use crate::oer::{
    decode_fixed_octet_string, decode_generalized_time, decode_var_octet_string, decode_var_uint,
    encode_generalized_time, encode_var_octet_string, encode_var_uint,
};

const TYPE_PREPARE: u8 = 12;
const TYPE_FULFILL: u8 = 13;
const TYPE_REJECT: u8 = 14;

/// Check the packet type byte against `expected`, returning the number of
/// bytes consumed (always 1) so callers can fold it into their `offset`.
fn decode_type_byte(buf: &[u8], expected: u8) -> Result<usize, PacketError> {
    let type_byte = *buf.first().ok_or(PacketError::BufferUnderflow)?;
    if type_byte != expected {
        return Err(PacketError::InvalidType);
    }
    Ok(1)
}

/// An ILP PREPARE packet (RFC-0027 Section 3.1): a conditional payment
/// addressed to `destination`, carrying an opaque application payload.
///
/// Carries no execution condition (issue #1269 / ADR 0069): the condition was
/// `derive_condition(derive_fulfillment(shared_secret))`, invariant across
/// every hop and distinctive per packet -- a perfect join key for any two
/// hops on a path -- while buying no hop anything it does not already have.
/// A hop is paid on arrival (ADR 0042), a mismatch still charges
/// (`f99_application_error`'s old mismatch branch), and a termination's own
/// check was a tautology: it derives the fulfilment from the same secret the
/// sender minted the condition from. The sender already verifies end to end
/// (`connector send` against its own `derive_fulfillment`), which is the only
/// check that ever protected anything. `greeting` replaces the all-zero
/// condition as the bootstrap-probe discriminator (issue #807's fix,
/// restated as a stated shape rather than an inferred one).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prepare {
    pub amount: u64,
    pub expires_at: DateTime<Utc>,
    /// Whether this PREPARE declares itself a bootstrap/greeting probe
    /// rather than a real payment attempt. Consulted only by
    /// `connector-client-edge`'s `handle_ilp`/`handle_frame`, and only when
    /// no claim header is attached: an unclaimed request with `greeting`
    /// set is never routed, priced or fulfilled, always answered with the
    /// x402 `payment-required` terms instead. A claim header suppresses
    /// this unconditionally regardless of `greeting`'s value, so a
    /// claim-bearing PREPARE is routed normally whether or not it declares
    /// itself a greeting -- the flag can broaden when the unclaimed case is
    /// greeted, never narrow when the claimed case is charged. Replaces the
    /// old all-zero-condition discriminator: a shape the protocol states,
    /// not one inferred from the absence of a field that no longer exists.
    pub greeting: bool,
    pub destination: String,
    pub data: Vec<u8>,
}

impl Prepare {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.push(TYPE_PREPARE);
        out.extend(encode_var_uint(self.amount));
        out.extend(encode_generalized_time(self.expires_at));
        out.push(u8::from(self.greeting));
        out.extend(encode_var_octet_string(self.destination.as_bytes()));
        out.extend(encode_var_octet_string(&self.data));
        out
    }

    pub fn decode(buf: &[u8]) -> Result<Prepare, PacketError> {
        let mut offset = decode_type_byte(buf, TYPE_PREPARE)?;

        let (amount, n) = decode_var_uint(buf, offset)?;
        offset += n;

        let (expires_at, n) = decode_generalized_time(buf, offset)?;
        offset += n;

        let greeting_byte = *buf.get(offset).ok_or(PacketError::BufferUnderflow)?;
        let greeting = match greeting_byte {
            0 => false,
            1 => true,
            _ => return Err(PacketError::InvalidType),
        };
        offset += 1;

        let (destination_bytes, n) = decode_var_octet_string(buf, offset)?;
        offset += n;
        let destination = String::from_utf8(destination_bytes)
            .map_err(|_| PacketError::InvalidAddress(String::new()))?;
        if !is_valid_ilp_address(&destination) {
            return Err(PacketError::InvalidAddress(destination));
        }

        let (data, n) = decode_var_octet_string(buf, offset)?;
        offset += n;

        if offset != buf.len() {
            return Err(PacketError::TrailingBytes);
        }

        Ok(Prepare {
            amount,
            expires_at,
            greeting,
            destination,
            data,
        })
    }
}

/// An ILP FULFILL packet (RFC-0027 Section 3.2): proof that a PREPARE was
/// honored, carrying an optional return payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fulfill {
    pub fulfillment: [u8; 32],
    pub data: Vec<u8>,
}

impl Fulfill {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.push(TYPE_FULFILL);
        out.extend_from_slice(&self.fulfillment);
        out.extend(encode_var_octet_string(&self.data));
        out
    }

    pub fn decode(buf: &[u8]) -> Result<Fulfill, PacketError> {
        let mut offset = decode_type_byte(buf, TYPE_FULFILL)?;

        let (fulfillment, n) = decode_fixed_octet_string(buf, offset, 32)?;
        offset += n;

        let (data, n) = decode_var_octet_string(buf, offset)?;
        offset += n;

        if offset != buf.len() {
            return Err(PacketError::TrailingBytes);
        }

        Ok(Fulfill { fulfillment, data })
    }
}

/// A three-character ILP error code (RFC-0027 Section 3.3): `F`-prefixed
/// (final), `T`-prefixed (temporary) or `R`-prefixed (relative).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RejectCode(String);

impl RejectCode {
    /// F00: Bad Request -- generic final error. Used (issue #596) when a
    /// terminated envelope's `target` attempts to escape the route's
    /// configured handler path -- refused before the app is ever called,
    /// distinct from F01 (the envelope itself did not decode) and from an
    /// app's own answer, including a 404, which never produces a reject at
    /// all.
    pub fn f00_bad_request() -> RejectCode {
        RejectCode("F00".to_string())
    }

    /// F01: Invalid Packet -- malformed.
    pub fn f01_invalid_packet() -> RejectCode {
        RejectCode("F01".to_string())
    }

    /// F02: Unreachable -- no route to the destination.
    pub fn f02_unreachable() -> RejectCode {
        RejectCode("F02".to_string())
    }

    /// F03: Invalid Amount -- a claim's value does not cover what it is
    /// paying for: a locally-terminated route's configured price (issue
    /// #522, `client-edge-spec.md` §1.3 step 3) or, later, a
    /// request-request-bound route's price (§1.5). Distinct from F01: the
    /// claim is structurally and cryptographically fine, it is simply not
    /// enough value.
    pub fn f03_invalid_amount() -> RejectCode {
        RejectCode("F03".to_string())
    }

    /// F06: Unexpected Payment -- a payment arrived without the claim that
    /// pays for it. The client edge's BTP carriage answers a claimless
    /// PREPARE to a priced route with this code plus the route's x402 terms
    /// as `payment-required` protocolData (client-edge-spec.md §1.9), since
    /// BTP cannot answer HTTP 402; the HTTP carriage answers 402 itself and
    /// never raises this code.
    pub fn f06_unexpected_payment() -> RejectCode {
        RejectCode("F06".to_string())
    }

    /// F99: Application Error -- the terminating app declined the delivery.
    /// Until issue #1269, also raised when a candidate fulfilment failed to
    /// verify against the packet's execution condition; that check (and the
    /// condition itself) is retired, so this constructor has no caller in
    /// this workspace today. Kept rather than deleted: it names one of
    /// RFC-0027's own error codes, not a mechanism this connector invented,
    /// and this type otherwise enumerates that vocabulary exhaustively
    /// whether or not every code currently has a producer.
    pub fn f99_application_error() -> RejectCode {
        RejectCode("F99".to_string())
    }

    /// R00: Transfer Timed Out -- the packet has run out of time here.
    ///
    /// Two cases, one fact. Its expiry has already passed on arrival
    /// (`packet-flow-spec.md` PF-02), or it arrived alive but with no more
    /// time left than the message window a hop must keep back to forward at
    /// all (PF-19, [`crate::forwarded_expiry`]) -- the second is the first
    /// one hop later, and neither is about this hop's fee, route or
    /// configuration. Class-only under ADR 0051: the sender's move is a
    /// fresh packet with more budget either way, so there is nothing for
    /// the code to bind beyond its class.
    pub fn r00_transfer_timed_out() -> RejectCode {
        RejectCode("R00".to_string())
    }

    /// R01: Insufficient Source Amount -- RFC 0027's own definition, "the
    /// amount received by a connector in the path was too little to forward
    /// (zero or less)": this hop's flat fee alone exceeds what arrived, so
    /// `amount_after_fee` yields nothing to pass on.
    ///
    /// This is the *standard* ILPv4 meaning and the only one. The
    /// minimum-delivery meaning this code also carried -- "the amount minus
    /// this hop's fee falls below the floor the sender declared" -- is
    /// retired with the field itself (ADR 0057, issue #1143); the case
    /// above is not, and is RFC 0027's `R01` rather than ADR 0051's `F03`,
    /// whose row is about an amount wrong for a *price* the sender can pay.
    /// Relative, not final, because the sender's move is to send more.
    pub fn r01_insufficient_source_amount() -> RejectCode {
        RejectCode("R01".to_string())
    }

    /// T00: Internal Error -- this connector could not do its own part of
    /// the work, through no fault of the packet. Retryable, and
    /// deliberately temporary rather than final (issue #605): a claim this
    /// connector could not durably record is refused under this code, so a
    /// sender learns to retry rather than that its perfectly good claim was
    /// invalid.
    pub fn t00_internal_error() -> RejectCode {
        RejectCode("T00".to_string())
    }

    /// T01: Peer Unreachable -- the app could not be reached over HTTP.
    /// Also used (issue #698) when a client-edge destination has no live
    /// BTP session bound to it, or the session that would have carried the
    /// delivery ended or was superseded mid-flight: in every case the
    /// packet itself is fine and the only fact worth reporting is "there is
    /// currently no way to reach this peer," so the sender should retry
    /// rather than conclude anything about the packet.
    pub fn t01_peer_unreachable() -> RejectCode {
        RejectCode("T01".to_string())
    }

    /// T04: Insufficient Liquidity (RFC-0027). Used until issue #424
    /// (peer-semantics-pre-868.md §5.1/§5.3) for this connector's own exposure
    /// ceiling; that machinery is retired (ADR 0031, ADR 0033, issue #882).
    ///
    /// It is emitted again, for a different thing: ADR 0049's **cap**, the
    /// largest amount this connector will forward to a given peer. A packet
    /// over that cap is refused `T04` by `Connector::forward_to_peer`, and
    /// the refusal's message carries the cap -- discovery is by this refusal
    /// and nothing else, because clause 4 decided against publishing caps
    /// (they would disclose who this node peers with and how far it trusts
    /// each). Between #424 and the cap landing nothing emitted `T04`, and
    /// this comment went on saying so; issue #1079 corrected it.
    pub fn t04_insufficient_liquidity() -> RejectCode {
        RejectCode("T04".to_string())
    }

    /// T05: Rate Limited -- this connector is deliberately withholding free
    /// work from the sender rather than failing at it (issue #613,
    /// RFC-0027's own gloss: "the connector is rate limiting the sender").
    /// Retryable, and the distinction from `T00` is the whole point: `T00`
    /// says this connector tried and could not, `T05` says it declined to
    /// try, and a sender told the first would retry immediately while a
    /// sender told the second should wait out a window.
    pub fn t05_rate_limited() -> RejectCode {
        RejectCode("T05".to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn parse(code: &[u8]) -> Result<RejectCode, PacketError> {
        let text = std::str::from_utf8(code).map_err(|_| {
            PacketError::InvalidErrorCode(String::from_utf8_lossy(code).into_owned())
        })?;
        if text.len() != 3 || !text.is_ascii() {
            return Err(PacketError::InvalidErrorCode(text.to_string()));
        }
        Ok(RejectCode(text.to_string()))
    }
}

/// An ILP REJECT packet (RFC-0027 Section 3.3): a PREPARE was not honored.
///
/// `accumulated_cost` is the running total of what this packet's path has
/// charged so far: the fees of every hop it actually passed through, plus
/// the price of the route that terminates it, if it has reached one (ADR
/// 0011, issue #523, `peer-semantics-pre-868.md` §5.2) -- `0` when this connector
/// originated the reject itself before forwarding or terminating the
/// packet, since neither a fee nor a price applies to a hop the packet
/// never used. It is a single sum -- never a per-hop breakdown, and never
/// split between fees and price, since either would leak topology or
/// pricing a probe has no need to know.
/// Deliberately **not** part of this struct's OER wire encoding below:
/// RFC-0027 has no such field, and this codec's **field set** is faithfully
/// the RFC's even though its byte layout is not (see this module's own doc and
/// ADR 0063). Adding a field would be a departure of a larger and different
/// kind from the three encoding ones -- every reader of the reject bytes,
/// including ones this project does not write, would have to learn it, whereas
/// the encoding divergences leave the field set intelligible.
/// `accumulated_cost` instead rides beside the packet -- at the peer
/// wire's frame level, or the client edge's `TOON-Accumulated-Cost` response
/// header (`docs/protocol/client-edge-spec.md` §1.6) -- never inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reject {
    pub code: RejectCode,
    pub triggered_by: String,
    pub message: String,
    pub data: Vec<u8>,
    pub accumulated_cost: u64,
}

impl Reject {
    /// Encodes exactly RFC-0027's REJECT fields -- `accumulated_cost` is
    /// deliberately absent, see the struct's own doc.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.push(TYPE_REJECT);
        out.extend_from_slice(self.code.as_str().as_bytes());
        out.extend(encode_var_octet_string(self.triggered_by.as_bytes()));
        out.extend(encode_var_octet_string(self.message.as_bytes()));
        out.extend(encode_var_octet_string(&self.data));
        out
    }

    pub fn decode(buf: &[u8]) -> Result<Reject, PacketError> {
        let mut offset = decode_type_byte(buf, TYPE_REJECT)?;

        let code_end = offset + 3;
        if code_end > buf.len() {
            return Err(PacketError::BufferUnderflow);
        }
        let code = RejectCode::parse(&buf[offset..code_end])?;
        offset = code_end;

        let (triggered_by_bytes, n) = decode_var_octet_string(buf, offset)?;
        offset += n;
        let triggered_by = String::from_utf8(triggered_by_bytes)
            .map_err(|_| PacketError::InvalidAddress(String::new()))?;
        if !triggered_by.is_empty() && !is_valid_ilp_address(&triggered_by) {
            return Err(PacketError::InvalidAddress(triggered_by));
        }

        let (message_bytes, n) = decode_var_octet_string(buf, offset)?;
        offset += n;
        let message = String::from_utf8(message_bytes).map_err(|_| PacketError::InvalidType)?;

        let (data, n) = decode_var_octet_string(buf, offset)?;
        offset += n;

        if offset != buf.len() {
            return Err(PacketError::TrailingBytes);
        }

        Ok(Reject {
            code,
            triggered_by,
            message,
            data,
            accumulated_cost: 0,
        })
    }
}

/// The outcome of routing a [`Prepare`]: exactly one of the two ILP-level
/// results a PREPARE can produce.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PacketResponse {
    Fulfill(Fulfill),
    Reject(Reject),
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn sample_prepare() -> Prepare {
        Prepare {
            amount: 100,
            expires_at: Utc.with_ymd_and_hms(2030, 6, 15, 12, 0, 0).unwrap(),
            greeting: false,
            destination: "g.example.app".to_string(),
            data: b"hello app".to_vec(),
        }
    }

    #[test]
    fn prepare_round_trips() {
        let prepare = sample_prepare();
        let encoded = prepare.encode();
        assert_eq!(encoded[0], TYPE_PREPARE);
        let decoded = Prepare::decode(&encoded).expect("decode");
        assert_eq!(decoded, prepare);
    }

    /// The `greeting` flag is the one field this codec added over RFC-0027's
    /// own set (issue #1269) -- it must survive a round trip exactly like
    /// every other field, in both states.
    #[test]
    fn greeting_flag_round_trips_in_both_states() {
        let greeting_probe = Prepare {
            greeting: true,
            ..sample_prepare()
        };
        let decoded = Prepare::decode(&greeting_probe.encode()).expect("decode");
        assert_eq!(decoded, greeting_probe);
        assert!(decoded.greeting);

        let ordinary = Prepare {
            greeting: false,
            ..sample_prepare()
        };
        let decoded = Prepare::decode(&ordinary.encode()).expect("decode");
        assert_eq!(decoded, ordinary);
        assert!(!decoded.greeting);
    }

    /// The `greeting` byte is a single octet immediately after `expiresAt`
    /// and before `destination` -- any value other than 0 or 1 is a
    /// malformed packet, not a truthy/falsy coercion, so a decoder never
    /// silently accepts a byte a well-formed encoder would never produce.
    #[test]
    fn a_greeting_byte_other_than_zero_or_one_is_rejected() {
        let mut encoded = sample_prepare().encode();
        // Type byte (1) + `amount: 100`'s single-byte VarUInt (1) +
        // GeneralizedTime (19) = offset 21 -- the same layout
        // `prepare_decode_rejects_a_non_canonical_amount_determinant` above
        // pins for `encoded[1]`.
        let greeting_offset = 21;
        // Sanity: the byte we are about to corrupt really is the greeting
        // flag `sample_prepare()` encoded as `false` (0x00).
        assert_eq!(encoded[greeting_offset], 0x00);
        encoded[greeting_offset] = 0x02;
        assert!(matches!(
            Prepare::decode(&encoded),
            Err(PacketError::InvalidType)
        ));
    }

    /// Issue #546 tightens canonicality in the shared `oer.rs` primitives
    /// rather than only in the envelope's own decode path (see that issue's
    /// resolution note and `oer.rs::decode_var_uint`'s doc comment for why),
    /// so a PREPARE's `amount` -- a VarUInt, like everything #546 describes --
    /// is covered by the same fix without any change to this file.
    #[test]
    fn prepare_decode_rejects_a_non_canonical_amount_determinant() {
        let mut encoded = sample_prepare().encode();
        // `amount: 100` canonically encodes as the single byte 0x64
        // (100 <= 127). Splice in the long-form alias 0x81 0x64 instead.
        assert_eq!(encoded[1], 0x64);
        encoded.splice(1..2, [0x81, 0x64]);
        assert!(matches!(
            Prepare::decode(&encoded),
            Err(PacketError::NonCanonicalLength)
        ));
    }

    #[test]
    fn prepare_decode_rejects_wrong_type_byte() {
        let mut encoded = sample_prepare().encode();
        encoded[0] = TYPE_FULFILL;
        assert!(matches!(
            Prepare::decode(&encoded),
            Err(PacketError::InvalidType)
        ));
    }

    #[test]
    fn prepare_decode_rejects_an_invalid_destination() {
        let mut prepare = sample_prepare();
        prepare.destination = "g..app".to_string();
        let encoded = prepare.encode();
        assert!(matches!(
            Prepare::decode(&encoded),
            Err(PacketError::InvalidAddress(_))
        ));
    }

    #[test]
    fn prepare_decode_rejects_truncated_input() {
        let encoded = sample_prepare().encode();
        assert!(matches!(
            Prepare::decode(&encoded[..encoded.len() - 1]),
            Err(PacketError::TrailingBytes) | Err(PacketError::BufferUnderflow)
        ));
    }

    #[test]
    fn prepare_decode_rejects_trailing_bytes() {
        let mut encoded = sample_prepare().encode();
        encoded.push(0xff);
        assert!(matches!(
            Prepare::decode(&encoded),
            Err(PacketError::TrailingBytes)
        ));
    }

    #[test]
    fn fulfill_round_trips() {
        let fulfill = Fulfill {
            fulfillment: [7u8; 32],
            data: b"ok".to_vec(),
        };
        let encoded = fulfill.encode();
        assert_eq!(encoded[0], TYPE_FULFILL);
        assert_eq!(Fulfill::decode(&encoded).expect("decode"), fulfill);
    }

    #[test]
    fn reject_round_trips() {
        let reject = Reject {
            code: RejectCode::f02_unreachable(),
            triggered_by: "g.connector".to_string(),
            message: "no route".to_string(),
            data: vec![],
            accumulated_cost: 0,
        };
        let encoded = reject.encode();
        assert_eq!(encoded[0], TYPE_REJECT);
        assert_eq!(Reject::decode(&encoded).expect("decode"), reject);
    }

    #[test]
    fn reject_allows_an_empty_triggered_by() {
        let reject = Reject {
            code: RejectCode::f99_application_error(),
            triggered_by: String::new(),
            message: "declined".to_string(),
            data: vec![],
            accumulated_cost: 0,
        };
        let encoded = reject.encode();
        assert_eq!(Reject::decode(&encoded).expect("decode"), reject);
    }

    /// ADR 0011 / peer-semantics-pre-868.md §5.2: `accumulated_cost` rides beside the
    /// packet (frame level / response header), never inside RFC-0027's own
    /// REJECT encoding -- so a nonzero value never survives an encode/decode
    /// round trip through this struct's wire format alone.
    #[test]
    fn accumulated_cost_does_not_ride_the_oer_wire_encoding() {
        let reject = Reject {
            code: RejectCode::f02_unreachable(),
            triggered_by: String::new(),
            message: "no route".to_string(),
            data: vec![],
            accumulated_cost: 42,
        };
        let encoded = reject.encode();
        let decoded = Reject::decode(&encoded).expect("decode");
        assert_eq!(decoded.accumulated_cost, 0);
        assert_eq!(decoded.code, reject.code);
        assert_eq!(decoded.message, reject.message);
    }

    #[test]
    fn reject_code_as_str_matches_the_constant() {
        assert_eq!(RejectCode::f02_unreachable().as_str(), "F02");
        assert_eq!(RejectCode::f99_application_error().as_str(), "F99");
        assert_eq!(RejectCode::t01_peer_unreachable().as_str(), "T01");
        assert_eq!(RejectCode::t04_insufficient_liquidity().as_str(), "T04");
        assert_eq!(RejectCode::t05_rate_limited().as_str(), "T05");
        assert_eq!(RejectCode::f00_bad_request().as_str(), "F00");
        assert_eq!(RejectCode::f01_invalid_packet().as_str(), "F01");
        assert_eq!(RejectCode::r00_transfer_timed_out().as_str(), "R00");
        assert_eq!(RejectCode::r01_insufficient_source_amount().as_str(), "R01");
        assert_eq!(RejectCode::f03_invalid_amount().as_str(), "F03");
    }
}
