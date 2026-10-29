//! The claim acknowledgement on the wire (`peer-carriage-spec.md` §6).
//!
//! A `claim-ack` is a **field on the response the carriage already
//! requires** for the claim-bearing frame -- never a frame of its own
//! (§6.1). On BTP it is a `claim-ack` protocolData entry on the RESPONSE
//! under the claim-bearing MESSAGE's or TRANSFER's `requestId`, carrying
//! raw UTF-8 JSON:
//!
//! ```json
//! { "result": "accepted", "network": "eip155:8453" }
//! { "result": "rejected", "reason": "signature_invalid", "network": "eip155:8453" }
//! ```
//!
//! **`network` names the network of the voucher judged** (ADR 0086
//! decision 1, `schema_version` 11), so a payer on several networks reads a
//! verdict only for the voucher it sent: [`decode`] takes the network that
//! voucher named and reads an ack naming any other -- or none -- as not
//! acknowledged.
//!
//! **One refusal taxonomy** (spec I3): [`ClaimRejectReason`] → this JSON is
//! the single function [`encode`], called by whichever carriage is
//! answering. A fifth variant added to that enum without a corresponding
//! arm here does not compile, so it cannot appear on one carriage and not
//! the other.
//!
//! **Absence and malformation both mean NOT ACKNOWLEDGED** (§6.3). That is
//! the honest loss of moving CLAIM_ACK from a frame type to a field: as an
//! entry it is omissible, where a distinct frame type made "the peer sent
//! no ack" inexpressible. [`decode`] therefore returns `None` for every
//! shape that is not exactly one of the two above -- undecodable JSON, an
//! unknown `result`, an unknown `reason`, a `rejected` with no `reason` --
//! and a caller must never read `None` as either verdict.

use connector_btp::{ProtocolData, CLAIM_ACK_PROTOCOL, CONTENT_TYPE_TEXT};
use connector_domain::Network;
use connector_runtime::{ClaimAckOutcome, ClaimRejectReason};

/// The wire spelling of each of `peer-semantics-pre-868.md` §3.4's four reasons
/// (§6.1). Not extensible without a spec change and a vector.
pub fn reason_name(reason: ClaimRejectReason) -> &'static str {
    match reason {
        ClaimRejectReason::SignatureInvalid => "signature_invalid",
        ClaimRejectReason::NonceNotAdvancing => "nonce_not_advancing",
        ClaimRejectReason::AmountNotAdvancing => "amount_not_advancing",
        ClaimRejectReason::UnknownChannel => "unknown_channel",
    }
}

/// The inverse of [`reason_name`]. `None` for a spelling this build does
/// not know, which §6.3 makes "not acknowledged" rather than an error.
pub fn reason_from_name(name: &str) -> Option<ClaimRejectReason> {
    match name {
        "signature_invalid" => Some(ClaimRejectReason::SignatureInvalid),
        "nonce_not_advancing" => Some(ClaimRejectReason::NonceNotAdvancing),
        "amount_not_advancing" => Some(ClaimRejectReason::AmountNotAdvancing),
        "unknown_channel" => Some(ClaimRejectReason::UnknownChannel),
        _ => None,
    }
}

/// The ack JSON for a claim that was judged. `None` for
/// [`ClaimAckOutcome::NotSent`]: there was no claim to acknowledge, and
/// §6.2 forbids a `claim-ack` on a response answering a frame that carried
/// none.
/// **Field order is the specification's**, `result` first (§6.1). Written
/// here rather than left to `serde_json::json!`, whose default object is a
/// `BTreeMap` and therefore renders `reason` before `result` -- valid JSON
/// that no reader can tell apart, and a byte string that does not match the
/// two examples §6.1 pins or the ones this module's own header quotes.
/// §10.2 vectors the ack, and a vector that disagreed with the prose it
/// vectors would be a spec bug found the expensive way.
///
/// `network` is the judged voucher's own: an ack names it (ADR 0086), and
/// with none -- no voucher was judged -- there is nothing to acknowledge.
pub fn encode(outcome: ClaimAckOutcome, network: Option<&Network>) -> Option<Vec<u8>> {
    let network = network?;
    let json = match outcome {
        ClaimAckOutcome::NotSent => return None,
        ClaimAckOutcome::Accepted => {
            format!(r#"{{"result":"accepted","network":"{network}"}}"#)
        }
        ClaimAckOutcome::Rejected(reason) => format!(
            r#"{{"result":"rejected","reason":"{}","network":"{network}"}}"#,
            reason_name(reason)
        ),
    };
    Some(json.into_bytes())
}

/// The `claim-ack` protocolData entry for a judged claim, or `None` when
/// there is nothing to acknowledge.
pub fn protocol_data(outcome: ClaimAckOutcome, network: Option<&Network>) -> Option<ProtocolData> {
    encode(outcome, network).map(|data| ProtocolData {
        name: CLAIM_ACK_PROTOCOL.to_string(),
        content_type: CONTENT_TYPE_TEXT,
        data,
    })
}

/// Read the verdict a peer sent back. `None` is **not acknowledged**
/// (§6.3) and covers every shape that is not exactly one of the two the
/// spec names -- it is never a verdict of its own, and a caller that turns
/// it into `Accepted` or `Rejected` has broken the rule that stands between
/// a lost ack and a permanently wedged peering.
///
/// `sent_on` is the network the answered voucher named: an ack naming
/// another, or none, is not acknowledged (ADR 0086).
pub fn decode(bytes: &[u8], sent_on: &Network) -> Option<ClaimAckOutcome> {
    let json: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    let network: Network = json.get("network")?.as_str()?.parse().ok()?;
    if network != *sent_on {
        return None;
    }
    match json.get("result")?.as_str()? {
        "accepted" => Some(ClaimAckOutcome::Accepted),
        "rejected" => {
            let reason = json.get("reason")?.as_str()?;
            reason_from_name(reason).map(ClaimAckOutcome::Rejected)
        }
        _ => None,
    }
}

/// The verdict carried by `protocol_data`, if it carries one at all.
/// **Absent means not acknowledged**, exactly as malformed does, so a
/// caller cannot distinguish "the peer chose to say nothing" from "the peer
/// said something we could not read" -- and §6.3 requires it does not have
/// to.
pub fn from_protocol_data(
    protocol_data: &[ProtocolData],
    sent_on: &Network,
) -> Option<ClaimAckOutcome> {
    let entry = protocol_data
        .iter()
        .find(|pd| pd.name == CLAIM_ACK_PROTOCOL)?;
    decode(&entry.data, sent_on)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn network() -> Network {
        Network::Eip155(84_532)
    }

    #[test]
    fn an_accepted_ack_is_the_json_the_spec_pins() {
        let bytes = encode(ClaimAckOutcome::Accepted, Some(&network()))
            .expect("a judged claim is acknowledged");
        assert_eq!(
            String::from_utf8(bytes.clone()).expect("utf-8"),
            r#"{"result":"accepted","network":"eip155:84532"}"#
        );
        let json: serde_json::Value = serde_json::from_slice(&bytes).expect("valid JSON");
        assert!(json.get("reason").is_none(), "accepted carries no reason");
    }

    /// ADR 0086: an ack names the network of the voucher it judged, and is
    /// read as a verdict only for that network. One naming another, or
    /// none -- an ack from a node that predates `schema_version` 11 -- is
    /// not acknowledged.
    #[test]
    fn an_ack_is_a_verdict_only_on_the_network_its_voucher_was_sent_on() {
        let rejected = ClaimAckOutcome::Rejected(ClaimRejectReason::AmountNotAdvancing);
        let bytes = encode(rejected, Some(&network())).expect("judged");
        assert_eq!(
            String::from_utf8(bytes.clone()).expect("utf-8"),
            r#"{"result":"rejected","reason":"amount_not_advancing","network":"eip155:84532"}"#
        );
        assert_eq!(decode(&bytes, &network()), Some(rejected));
        assert_eq!(decode(&bytes, &Network::Eip155(31_337)), None);
        assert_eq!(decode(br#"{"result":"accepted"}"#, &network()), None);
    }

    /// An ack answers a voucher, and a voucher always names its network:
    /// with no network to name there is nothing to acknowledge.
    #[test]
    fn there_is_no_ack_without_a_network() {
        assert_eq!(encode(ClaimAckOutcome::Accepted, None), None);
    }

    #[test]
    fn every_reject_reason_round_trips_through_its_wire_spelling() {
        for reason in [
            ClaimRejectReason::SignatureInvalid,
            ClaimRejectReason::NonceNotAdvancing,
            ClaimRejectReason::AmountNotAdvancing,
            ClaimRejectReason::UnknownChannel,
        ] {
            let bytes =
                encode(ClaimAckOutcome::Rejected(reason), Some(&network())).expect("judged");
            assert_eq!(
                decode(&bytes, &network()),
                Some(ClaimAckOutcome::Rejected(reason))
            );
        }
    }

    /// §6.2: a `claim-ack` must not appear on a response answering a frame
    /// that carried no claim, so there is no encoding for `NotSent`.
    #[test]
    fn a_frame_that_carried_no_claim_is_answered_with_no_ack_entry() {
        assert_eq!(encode(ClaimAckOutcome::NotSent, Some(&network())), None);
        assert_eq!(
            protocol_data(ClaimAckOutcome::NotSent, Some(&network())),
            None
        );
    }

    /// §6.3: every malformed shape is *not acknowledged* -- never a
    /// verdict, and never an error.
    #[test]
    fn every_malformed_ack_decodes_as_not_acknowledged() {
        for malformed in [
            &b"not json at all"[..],
            br#"{}"#,
            br#"{"result":"maybe"}"#,
            br#"{"result":"rejected"}"#,
            br#"{"result":"rejected","reason":"the vibes were off"}"#,
            br#"{"result":123}"#,
            br#"[]"#,
        ] {
            assert_eq!(
                decode(malformed, &network()),
                None,
                "got a verdict from {malformed:?}"
            );
        }
    }

    /// §6.3: absence is the same "not acknowledged" as malformation, and
    /// the two are deliberately indistinguishable to the payer.
    #[test]
    fn an_absent_ack_entry_is_not_acknowledged() {
        let unrelated = vec![ProtocolData {
            name: "toon-accumulated-cost".to_string(),
            content_type: CONTENT_TYPE_TEXT,
            data: b"7".to_vec(),
        }];

        assert_eq!(from_protocol_data(&unrelated, &network()), None);
        assert_eq!(from_protocol_data(&[], &network()), None);
    }

    #[test]
    fn an_ack_entry_is_read_back_off_the_response_it_rode() {
        let entry = protocol_data(ClaimAckOutcome::Accepted, Some(&network())).expect("judged");
        assert_eq!(entry.name, CLAIM_ACK_PROTOCOL);

        assert_eq!(
            from_protocol_data(&[entry], &network()),
            Some(ClaimAckOutcome::Accepted)
        );
    }
}
