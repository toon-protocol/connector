//! Carriage-layer fields (`peer-carriage-spec.md` §5): the values
//! RFC-0027 has no field for, riding as protocolData entries.
//!
//! §8.3's layering invariant is why they ride here at all: **carriage-layer
//! fields are never sealed, and sealed payloads are never carriage-layer
//! fields**, precisely so a hop can read and judge them without opening a
//! payload it holds no key for.

use connector_btp::{
    ProtocolData, ACCUMULATED_COST_PROTOCOL, CONTENT_TYPE_TEXT, PAYMENT_REQUIRED_PROTOCOL,
};
use connector_domain::x402::{parse_greeting, GreetingError, X402PaymentRequired};

/// The `toon-accumulated-cost` entry on a REJECT's RESPONSE (§5.2).
///
/// Already implemented on the client edge and **reused verbatim** -- same
/// entry name, same decimal uint64 text. Two carriage-level requirements
/// live here: the field rides **only** a REJECT (never beside a FULFILL),
/// and a hop **always** emits it on a REJECT it sends even when the value
/// is `0`, so that "absent" never has to carry meaning in the direction
/// that matters.
pub fn accumulated_cost_protocol_data(accumulated_cost: u64) -> ProtocolData {
    ProtocolData {
        name: ACCUMULATED_COST_PROTOCOL.to_string(),
        content_type: CONTENT_TYPE_TEXT,
        data: accumulated_cost.to_string().into_bytes(),
    }
}

/// The `payment-required` entry a REJECT carries when a peer PREPARE is
/// refused for want of a covering claim (issue #880): `terms` is
/// [`connector_domain::x402::terms_body`]'s own bytes, raw UTF-8 JSON --
/// the BTP twin of the HTTP peer carriage's base64 header, content
/// identical, only the carriage differs.
pub fn payment_required_protocol_data(terms: Vec<u8>) -> ProtocolData {
    ProtocolData {
        name: PAYMENT_REQUIRED_PROTOCOL.to_string(),
        content_type: CONTENT_TYPE_TEXT,
        data: terms,
    }
}

/// Read a REJECT's accumulated cost back. **Absent means zero on receipt**
/// (§5.2), and a relaying hop still adds its own fee to that zero before
/// passing the REJECT upstream.
pub fn accumulated_cost(protocol_data: &[ProtocolData]) -> u64 {
    protocol_data
        .iter()
        .find(|pd| pd.name == ACCUMULATED_COST_PROTOCOL)
        .and_then(|pd| std::str::from_utf8(&pd.data).ok())
        .and_then(|text| text.parse().ok())
        .unwrap_or(0)
}

/// The x402 terms a REJECT's `payment-required` entry carries (issue #874),
/// the BTP twin of the HTTP 402's `Payment-Required` header -- and by
/// construction the identical bytes, since the client edge serves both from
/// one `x402_terms_body`.
///
/// Three answers, and the difference between the last two is the whole
/// point:
///
/// * `None` -- no such entry. The peer answered without greeting us; this
///   is an ordinary answer and nothing is owed.
/// * `Some(Err(_))` -- an entry was there and could not be read. **Never
///   collapsed into `None`**: doing so would read a framing bug, a
///   truncated frame or a future x402 version as "no payment required" and
///   hand the far side a free-ride verdict it never gave.
/// * `Some(Ok(terms))` -- the terms this connector has to satisfy.
///
/// The parse itself is [`connector_domain::x402::parse_greeting`], shared
/// with whatever reads the HTTP carriage's 402: the greeting is not a BTP
/// concept, only its carriage is.
pub fn payment_required(
    protocol_data: &[ProtocolData],
) -> Option<Result<X402PaymentRequired, GreetingError>> {
    let entry = protocol_data
        .iter()
        .find(|pd| pd.name == PAYMENT_REQUIRED_PROTOCOL)?;
    Some(parse_greeting(&entry.data))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, data: &[u8]) -> ProtocolData {
        ProtocolData {
            name: name.to_string(),
            content_type: CONTENT_TYPE_TEXT,
            data: data.to_vec(),
        }
    }

    /// §5.2: always emitted on a REJECT, even at zero, so "absent" never
    /// has to carry meaning in the direction that matters.
    #[test]
    fn accumulated_cost_round_trips_and_absent_reads_as_zero() {
        assert_eq!(accumulated_cost(&[]), 0);
        assert_eq!(accumulated_cost(&[accumulated_cost_protocol_data(0)]), 0);
        assert_eq!(accumulated_cost(&[accumulated_cost_protocol_data(41)]), 41);
    }

    fn greeting(body: &[u8]) -> Vec<ProtocolData> {
        vec![entry(PAYMENT_REQUIRED_PROTOCOL, body)]
    }

    const TERMS: &[u8] = br#"{"x402Version":2,"resource":{"url":"g.toon.relay"},
        "extensions":{"toon":{"info":{"amount":"2000","ilpAddress":"g.toon.relay"}}}}"#;

    #[test]
    fn an_answer_with_no_greeting_asks_for_nothing() {
        assert!(payment_required(&[]).is_none());
        assert!(payment_required(&[accumulated_cost_protocol_data(41)]).is_none());
    }

    #[test]
    fn a_greeting_yields_the_terms_to_satisfy() {
        let terms = payment_required(&greeting(TERMS))
            .expect("the entry is there")
            .expect("and it reads");
        assert_eq!(terms.price(), Some(2000));
        assert_eq!(terms.ilp_address(), Some("g.toon.relay"));
    }

    /// Issue #874's load-bearing distinction: an unreadable greeting is an
    /// error, never the `None` that means "nothing was asked for".
    #[test]
    fn an_unreadable_greeting_is_an_error_not_an_absence() {
        for garbage in [&b""[..], b"{", b"null", br#"{"error":"no route"}"#] {
            let read = payment_required(&greeting(garbage));
            assert!(
                matches!(read, Some(Err(_))),
                "{garbage:?} was not read as a malformed greeting: {read:?}"
            );
        }
    }
}
