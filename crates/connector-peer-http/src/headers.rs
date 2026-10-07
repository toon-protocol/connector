//! The request and response this carriage sees, and §3's fields as they
//! ride on them.
//!
//! # One semantic value, two encodings (spec I1)
//!
//! Every function here is a *wrapper* over the decoder the BTP carriage
//! already uses, not a second decoder:
//!
//! | Field | What actually parses it |
//! | ----- | ----------------------- |
//! | claim | [`connector_peer_btp::claim_json`] -- the client edge's own claim validator (I4) |
//! | claim ack | [`connector_peer_btp::ack`] -- the one `ClaimRejectReason` → JSON function (I3) |
//! | `accumulatedCost` | [`connector_peer_btp::fields::accumulated_cost`] |
//!
//! Those functions take a `protocolData` entry, so this module builds one
//! from the header value and hands it over. Doing it that way rather than
//! writing "a small decimal parser, it is only four lines" is the whole
//! point: a decoder written twice is a rule enforced once, and a rule
//! enforced once is a rule the two carriages cannot disagree about. The
//! header/entry *names* are never spelled here either -- they come from
//! [`connector_btp::CARRIAGE_NAMES`]'s declared pairs.
//!
//! Base64 wraps the claim and the claim ack because base64 is a header
//! artifact and nothing else (§4). The value inside is the same JSON the BTP
//! entry carries raw.
//!
//! There is no credential row, and there was one: `Toon-Peer-Auth` carried a
//! `base64({peerId, secret})`. ADR 0060 deleted it. Nothing here reads that
//! header, and a request still setting one is read exactly as one that does
//! not -- ignored, never refused, so the two ends of a peering may be
//! upgraded in either order.

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use connector_btp::{
    ProtocolData, ACCUMULATED_COST_HEADER, ACCUMULATED_COST_PROTOCOL, CLAIM_ACK_HEADER,
    CLAIM_HEADER, CONTENT_TYPE_TEXT, FLUSH_REQUESTED_HEADER, PEER_CHALLENGE_HEADER,
};
use connector_peer_btp::{ack, fields};
use connector_runtime::ClaimAckOutcome;

/// One request's or response's headers, in arrival order and **with their
/// multiplicity intact**.
///
/// Multiplicity is load-bearing twice over: §1.5 refuses more than one
/// `Toon-Peer-Auth` rather than resolving it, and §6.4 lets
/// `Toon-Flush-Requested` appear once per channel. A map keyed by name would
/// quietly answer the first question wrong.
///
/// Names are compared case-insensitively per RFC 9110; what is stored is
/// whatever the caller wrote, and what this carriage writes is always the
/// canonical lower-case form the vectors pin (§3).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Headers(Vec<(String, String)>);

impl Headers {
    #[must_use]
    pub fn new() -> Self {
        Headers(Vec::new())
    }

    /// Add a header. Repeated names are kept, never merged into a list form.
    pub fn push(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.0.push((name.into(), value.into()));
    }

    /// Every value under `name`, case-insensitively.
    #[must_use]
    pub fn get_all(&self, name: &str) -> Vec<&str> {
        self.0
            .iter()
            .filter(|(header, _)| header.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
            .collect()
    }

    /// The first value under `name`, or `None`.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(header, _)| header.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// Every header, in order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl<'a> IntoIterator for &'a Headers {
    type Item = (&'a str, &'a str);
    type IntoIter = Box<dyn Iterator<Item = (&'a str, &'a str)> + 'a>;

    fn into_iter(self) -> Self::IntoIter {
        Box::new(self.iter())
    }
}

/// One peer request: a `POST` whose body is the OER PREPARE, or -- for a
/// FLUSH -- **empty** (§3).
///
/// There is no method and no path here on purpose. Which path a peer POSTs
/// to is the listener's business (issue #678), and the carriage's behaviour
/// must be provable without one; what §3 makes normative is the body and the
/// headers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PeerRequest {
    pub headers: Headers,
    pub body: Vec<u8>,
}

/// One peer response: the status, the headers §3 names, and the OER FULFILL
/// or REJECT as the body.
///
/// **The status is `200` regardless of the claim's verdict** (§6.2).
/// `4xx`/`5xx` are reserved for a malformed request or a connector fault --
/// cases where there is no ILP answer at all -- and a rejected claim is
/// never one of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerResponse {
    pub status: u16,
    pub headers: Headers,
    pub body: Vec<u8>,
}

impl PeerResponse {
    /// A `200` carrying `body` as the ILP answer.
    #[must_use]
    pub fn ok(body: Vec<u8>) -> Self {
        PeerResponse {
            status: 200,
            headers: Headers::new(),
            body,
        }
    }

    /// A refusal with **no ILP body**: §6.2's statuses, reserved for "there
    /// is no ILP answer at all" -- an undecodable ILP packet, §1.5's
    /// ambiguous claim, or §1.10's dedicated-listener `401`.
    #[must_use]
    pub fn refused(status: u16) -> Self {
        PeerResponse {
            status,
            headers: Headers::new(),
            body: Vec::new(),
        }
    }

    /// A refusal with no ILP body that names its reason in a `text/plain`
    /// body -- a `toon-channel` claim, refused by name (ADR 0075, issue
    /// #1384). Still not an ILP answer: the status is what says so.
    #[must_use]
    pub fn refused_naming(status: u16, reason: &[u8]) -> Self {
        PeerResponse {
            status,
            headers: Headers::new(),
            body: reason.to_vec(),
        }
    }

    /// Whether this response carries an ILP answer at all. A non-`200` does
    /// not, so nothing on it -- including a `Toon-Claim-Ack` -- is read as a
    /// verdict.
    #[must_use]
    pub fn answers_the_packet(&self) -> bool {
        self.status == 200
    }

    /// The x402 terms a `402` response quotes in `Payment-Required`, when it
    /// is one and they read. A `402` is the client edge's greeting (a far
    /// node that decided the client role for this sender): it has no ILP
    /// body, but it *is* an answer -- the peer was reached and named its
    /// price. `None` for any other status, a missing header, or terms that
    /// do not parse; none of those is an answer, and none is read as one.
    #[must_use]
    pub fn quoted_terms(&self) -> Option<connector_domain::x402::X402PaymentRequired> {
        if self.status != 402 {
            return None;
        }
        let value = self.headers.get(connector_btp::PAYMENT_REQUIRED_HEADER)?;
        let bytes = STANDARD.decode(value.trim()).ok()?;
        connector_domain::x402::parse_greeting(&bytes).ok()
    }
}

/// A claim header whose base64 layer would not decode.
///
/// Not one of §6.1's four reasons -- those judge a claim this connector
/// could read. An unreadable one is **not acknowledged** (§6.3): the payer's
/// claim stays pending and its retransmission is read the same way, rather
/// than a verdict being recorded that was never reached.
#[derive(Debug, PartialEq, Eq)]
pub struct ClaimHeaderNotBase64;

/// The `ILP-Payment-Channel-Claim` header value for `json` (§4):
/// `base64(JSON)`, over exactly the JSON the BTP entry carries raw.
#[must_use]
pub fn claim_header_value(json: &str) -> String {
    STANDARD.encode(json)
}

/// The [`connector_btp::PAYMENT_REQUIRED_HEADER`] value for `terms` (issue
/// #880): `base64(JSON)`, the HTTP twin of the BTP carriage's raw
/// protocolData entry, carrying the identical bytes
/// [`connector_domain::x402::terms_body`] emits under the identical name
/// the client edge's own `402` uses.
#[must_use]
pub fn payment_required_header_value(terms: &[u8]) -> String {
    STANDARD.encode(terms)
}

/// The claim JSON a request carries, if it carries one.
///
/// A request with no claim is legal on both carriages (§10.2 item 6), so
/// `None` is an ordinary outcome and not a refusal.
///
/// **First-wins, and only safe once §1.5's ambiguity check has run.** More
/// than one claim header on one request is refused (`400`, no ILP body) by
/// [`crate::PeerHttpState::handle`] before this is reached,
/// the twin of the BTP carriage's
/// [`connector_peer_btp::claim_json::present_from_protocol_data`]; a caller
/// reaching for this without that check answers "which claim did we
/// verify?" with "whichever came first".
///
/// **The privacy-wrapped carriage is not part of the peer carriage** (§4):
/// `ILP-Payment-Channel-Claim-Wrapped` is not read here at all, and a
/// peer-role request carrying one is treated as carrying no claim -- a
/// peering is configured by operators who know each other's channel
/// identity, so the anonymity it buys has no peer use.
pub fn claim_json(headers: &Headers) -> Option<Result<Vec<u8>, ClaimHeaderNotBase64>> {
    let value = headers.get(CLAIM_HEADER)?;
    Some(STANDARD.decode(value).map_err(|_| ClaimHeaderNotBase64))
}

/// The `Toon-Peer-Role-Challenge` header value for a challenge's JSON (ADR
/// 0075 decision 5): base64, as the claim header's is.
#[must_use]
pub fn peer_challenge_header_value(json: &str) -> String {
    STANDARD.encode(json.as_bytes())
}

/// The peer-role challenge a request carries, base64-decoded. `None` when
/// the header is absent; the caller counts duplicates first (§1.5).
#[must_use]
pub fn peer_challenge_json(headers: &Headers) -> Option<Result<Vec<u8>, ClaimHeaderNotBase64>> {
    let value = headers.get(PEER_CHALLENGE_HEADER)?;
    Some(STANDARD.decode(value).map_err(|_| ClaimHeaderNotBase64))
}

/// The `Toon-Claim-Ack` header value for a judged claim, or `None` when
/// there is nothing to acknowledge (§6.2 forbids one on a response answering
/// a request that carried no claim).
///
/// The JSON is [`connector_peer_btp::ack::encode`]'s -- the single
/// `ClaimRejectReason` → ack function both carriages call (I3), so a fifth
/// reason cannot appear on one carriage and not the other.
#[must_use]
pub fn claim_ack_header_value(outcome: ClaimAckOutcome) -> Option<String> {
    ack::encode(outcome).map(|json| STANDARD.encode(json))
}

/// The verdict a response carries. **Absence and malformation both mean NOT
/// ACKNOWLEDGED** (§6.3), and so does a base64 layer that will not decode:
/// every shape that is not exactly one of the two the spec names returns
/// `None`, which a caller must never read as either verdict.
#[must_use]
pub fn claim_ack(headers: &Headers) -> Option<ClaimAckOutcome> {
    let value = headers.get(CLAIM_ACK_HEADER)?;
    let json = STANDARD.decode(value).ok()?;
    ack::decode(&json)
}

/// A REJECT's running cost (§5.2). **Absent means zero on receipt**, and a
/// relaying hop still adds its own fee to that zero.
#[must_use]
pub fn accumulated_cost(headers: &Headers) -> u64 {
    let entries: Vec<ProtocolData> = headers
        .get(ACCUMULATED_COST_HEADER)
        .map(|value| entry(ACCUMULATED_COST_PROTOCOL, value.as_bytes()))
        .into_iter()
        .collect();
    fields::accumulated_cost(&entries)
}

/// The channel ids a response prompts a flush for (§6.4), one per
/// occurrence.
///
/// It is **a hint, and only a hint**: a payer with no pending claim for a
/// named channel, or that does not recognise it, ignores it, and a payer
/// that ignores every hint is not in violation of the specification. It is
/// never answered, acknowledged, or errored on.
///
/// No carriage in this connector emits the header any more: it prompted a
/// flush of a pending `toon-channel` claim, and since ADR 0075 (#1380) a
/// peer pays with a voucher riding the PREPARE it covers. The parser stays
/// because the `peer_carriage` wire vectors (`connector-vectors`) still
/// pin the header's shape.
#[must_use]
pub fn flush_requested(headers: &Headers) -> Vec<String> {
    headers
        .get_all(FLUSH_REQUESTED_HEADER)
        // A comma-separated list form MUST NOT be used (§6.4), so a value
        // containing one is not split into channels here: it is one
        // (unrecognised) channel id, which a payer ignores.
        .into_iter()
        .map(str::to_string)
        .collect()
}

fn entry(name: &str, data: &[u8]) -> ProtocolData {
    ProtocolData {
        name: name.to_string(),
        content_type: CONTENT_TYPE_TEXT,
        data: data.to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use connector_runtime::ClaimRejectReason;

    fn with(name: &str, value: &str) -> Headers {
        let mut headers = Headers::new();
        headers.push(name, value);
        headers
    }

    #[test]
    fn header_lookup_is_case_insensitive_and_keeps_multiplicity() {
        let mut headers = Headers::new();
        headers.push("Toon-Flush-Requested", "0xaa");
        headers.push("toon-flush-requested", "0xbb");

        assert_eq!(
            headers.get_all("TOON-FLUSH-REQUESTED"),
            vec!["0xaa", "0xbb"]
        );
        assert_eq!(headers.get("toon-flush-requested"), Some("0xaa"));
    }

    /// §4: the header is `base64(JSON)` over exactly the JSON the BTP entry
    /// carries raw -- base64 is a header artifact and nothing more.
    #[test]
    fn the_claim_header_wraps_exactly_the_json_the_btp_entry_carries_raw() {
        let json = r#"{"version":"1.0","blockchain":"evm"}"#;

        let headers = with(CLAIM_HEADER, &claim_header_value(json));

        assert_eq!(
            claim_json(&headers).expect("a claim rode"),
            Ok(json.as_bytes().to_vec())
        );
    }

    #[test]
    fn a_request_with_no_claim_header_carries_no_claim() {
        assert!(claim_json(&Headers::new()).is_none());
    }

    /// §6.3: an unreadable claim is *not acknowledged*, not an error and not
    /// a verdict.
    #[test]
    fn a_claim_header_that_is_not_base64_is_reported_rather_than_guessed_at() {
        let headers = with(CLAIM_HEADER, "!!! not base64 !!!");

        assert_eq!(claim_json(&headers), Some(Err(ClaimHeaderNotBase64)));
    }

    /// §4: the privacy-wrapped header is not part of the peer carriage on
    /// either wire, and a peer-role request carrying one carries no claim.
    #[test]
    fn a_privacy_wrapped_claim_header_is_ignored_on_a_peer_request() {
        let headers = with("ilp-payment-channel-claim-wrapped", "d2hhdGV2ZXI=");

        assert!(claim_json(&headers).is_none());
    }

    /// I3/§6.1: the ack JSON is the BTP carriage's, wrapped -- one refusal
    /// taxonomy, two encodings.
    #[test]
    fn every_verdict_round_trips_through_the_ack_header() {
        for outcome in [
            ClaimAckOutcome::Accepted,
            ClaimAckOutcome::Rejected(ClaimRejectReason::SignatureInvalid),
            ClaimAckOutcome::Rejected(ClaimRejectReason::NonceNotAdvancing),
            ClaimAckOutcome::Rejected(ClaimRejectReason::AmountNotAdvancing),
            ClaimAckOutcome::Rejected(ClaimRejectReason::UnknownChannel),
        ] {
            let value = claim_ack_header_value(outcome).expect("a judged claim is acknowledged");
            let decoded = STANDARD.decode(&value).expect("standard base64");

            assert_eq!(decoded, ack::encode(outcome).expect("the same JSON"));
            assert_eq!(claim_ack(&with(CLAIM_ACK_HEADER, &value)), Some(outcome));
        }
    }

    /// §6.2: no ack rides a response answering a request that carried no
    /// claim, so `NotSent` has no header value at all.
    #[test]
    fn a_request_that_carried_no_claim_is_answered_with_no_ack_header() {
        assert_eq!(claim_ack_header_value(ClaimAckOutcome::NotSent), None);
    }

    /// §6.3: absence and malformation are the same "not acknowledged", and
    /// the base64 layer is one more way to be malformed.
    #[test]
    fn an_absent_or_malformed_ack_header_is_not_acknowledged() {
        assert_eq!(claim_ack(&Headers::new()), None);
        assert_eq!(claim_ack(&with(CLAIM_ACK_HEADER, "!!!")), None);
        assert_eq!(
            claim_ack(&with(CLAIM_ACK_HEADER, &STANDARD.encode("not json"))),
            None
        );
        assert_eq!(
            claim_ack(&with(
                CLAIM_ACK_HEADER,
                &STANDARD.encode(r#"{"result":"maybe"}"#)
            )),
            None
        );
        assert_eq!(
            claim_ack(&with(
                CLAIM_ACK_HEADER,
                &STANDARD.encode(r#"{"result":"rejected"}"#)
            )),
            None
        );
    }

    #[test]
    fn accumulated_cost_reads_back_and_absent_is_zero() {
        assert_eq!(accumulated_cost(&Headers::new()), 0);
        assert_eq!(accumulated_cost(&with(ACCUMULATED_COST_HEADER, "0")), 0);
        assert_eq!(accumulated_cost(&with(ACCUMULATED_COST_HEADER, "41")), 41);
    }

    /// §6.4: one channel id per occurrence, and a comma-separated list form
    /// is not a list -- it is one id nobody recognises, which a payer
    /// ignores.
    #[test]
    fn a_flush_prompt_is_one_channel_per_occurrence() {
        let mut headers = Headers::new();
        headers.push(FLUSH_REQUESTED_HEADER, "0xaa");
        headers.push(FLUSH_REQUESTED_HEADER, "0xbb");

        assert_eq!(flush_requested(&headers), vec!["0xaa", "0xbb"]);
        assert!(flush_requested(&Headers::new()).is_empty());
        assert_eq!(
            flush_requested(&with(FLUSH_REQUESTED_HEADER, "0xaa,0xbb")),
            vec!["0xaa,0xbb"]
        );
    }
}
