//! The frame grammar: `type`/`requestId`/body, encode and decode.
//!
//! The deployed `@toon-protocol/client` dialect (`btp/protocol.ts`), extended
//! additively with RFC-0023's symmetric grammar as of issue #697 -- TRANSFER
//! (type 7) in both directions, and this connector's own ability to originate
//! a MESSAGE or TRANSFER. See `docs/protocol/client-edge-spec.md` §1.9 for
//! the layout, and this module's unit vectors for the bytes -- the vectors
//! are what a change here has to answer to (ADR 0021).

/// BTP frame types (client `btp/protocol.ts` `BTPMessageType`, plus RFC-0023's
/// TRANSFER -- issue #697). The deployed `@toon-protocol/client` dialect only
/// ever sends/receives MESSAGE and answers RESPONSE or ERROR; extending to
/// RFC-23's symmetric grammar adds TRANSFER (settlement value + protocolData)
/// and the ability for this connector to originate a MESSAGE or TRANSFER of
/// its own -- both additive, so a client that never sends TRANSFER and never
/// receives a server-originated MESSAGE observes no change (client-edge-spec
/// §1.9).
pub const BTP_RESPONSE: u8 = 1;
pub const BTP_ERROR: u8 = 2;
pub const BTP_MESSAGE: u8 = 6;
pub const BTP_TRANSFER: u8 = 7;

/// protocolData entry names (client `BtpRuntimeClient.ts` /
/// `IsomorphicBtpClient.ts`). The claim is raw UTF-8 claim JSON -- no
/// base64 layer; that is an HTTP-header artifact (§1.3 vs §1.9).
///
/// They live in the codec, not in a carriage, for the same reason the type
/// bytes do: ADR 0027's frame-carriage table reuses these exact names on the
/// peer carriage, and a second declaration of `"payment-channel-claim"`
/// somewhere else is a fork waiting to happen.
pub const AUTH_PROTOCOL: &str = "auth";
pub const CLAIM_PROTOCOL: &str = "payment-channel-claim";
/// BTP analogues of the HTTP `Payment-Required` / `TOON-Accumulated-Cost`
/// headers (§1.4, §1.6): the same bytes, riding as protocolData on a REJECT
/// RESPONSE since a websocket frame has no headers.
pub const PAYMENT_REQUIRED_PROTOCOL: &str = "payment-required";
pub const ACCUMULATED_COST_PROTOCOL: &str = "toon-accumulated-cost";
/// A payout TRANSFER's protocolData entry name (issue #699): the signed
/// claim this connector owes the counterparty on that channel. Building the
/// entry itself is the carriage's job -- it needs a claim type this crate
/// deliberately cannot see -- but the name it goes out under is grammar.
pub const PAYOUT_CLAIM_PROTOCOL: &str = "payout-claim";
/// The claim acknowledgement (`peer-carriage-spec.md` §3, §6.1): a field on
/// the RESPONSE that already answers the claim-bearing MESSAGE or TRANSFER,
/// never a frame of its own. Its HTTP twin is `Toon-Claim-Ack` (#728).
pub const CLAIM_ACK_PROTOCOL: &str = "claim-ack";
/// The **peer-role challenge** (ADR 0075 decision 5, `peer-carriage-spec.md`
/// §1.4): the voucher claim-state challenge a peer's packet that moves no
/// value carries in place of a voucher, to prove the peer role. Raw UTF-8
/// JSON, like the claim entry. Its own entry rather than the claim's,
/// because it is not a claim: it moves nothing and advances no watermark,
/// and a slot that could hold either would make "was this a payment?" a
/// question about the bytes rather than about where they rode.
pub const PEER_CHALLENGE_PROTOCOL: &str = "peer-role-challenge";
/// The HTTP twin of each entry name above, in its canonical lower-case form
/// (`peer-carriage-spec.md` §3; header names match case-insensitively per
/// RFC 9110, and the canonical form is the one the vectors pin).
///
/// Declared **here, beside the protocolData entry names**, because spec I2
/// requires the BTP name and the HTTP name for one concept to be declared
/// once, *as a pair*, in one shared module -- the spec offers this crate or
/// `connector-domain`, and the entry halves already live here after issue
/// #713. "Adding a header without its protocolData twin must be impossible
/// to express, not merely noticed in review": [`CARRIAGE_NAMES`] is the
/// table that makes it so, and [`tests::every_carriage_field_declares_both_halves`]
/// is what fails when a half goes missing.
///
/// That this crate is named for BTP is an artefact of where the codec was
/// extracted to, not a claim that these strings are BTP's. Nothing here
/// encodes an HTTP request; the ILP-over-HTTP carriage (issue #728) reads
/// these names and does that.
///
/// `ilp-payment-channel-claim` is the **deployed** spelling
/// (`client-edge-spec.md` §1.3, and `connector-client-edge`'s own header
/// constant, which is this one). ADR 0027's table originally wrote
/// `Payment-Channel-Claim`, mirroring the entry name; peer-carriage-spec
/// §12.1 pinned the deployed name instead under the ADR's own "one codec,
/// reused verbatim" rule -- a new header name would need a second decoder on
/// the HTTP path, which is the drift I2 exists to prevent -- and the ADR was
/// amended.
pub const CLAIM_HEADER: &str = "ilp-payment-channel-claim";
/// The HTTP twin of [`CLAIM_ACK_PROTOCOL`] (§3, §6.1).
pub const CLAIM_ACK_HEADER: &str = "toon-claim-ack";
/// The HTTP twin of [`PEER_CHALLENGE_PROTOCOL`] (§1.4): the same JSON,
/// base64 in the header as the claim's is.
pub const PEER_CHALLENGE_HEADER: &str = "toon-peer-role-challenge";
/// The HTTP twin of [`ACCUMULATED_COST_PROTOCOL`] (§3, §5.2) -- already
/// implemented on the client edge and reused verbatim.
pub const ACCUMULATED_COST_HEADER: &str = "toon-accumulated-cost";
/// The HTTP twin of [`PAYMENT_REQUIRED_PROTOCOL`] (§3.1, `client-edge-spec.md`
/// §1.4) -- the client edge's own `402` header spelling, reused verbatim by
/// the peer carriage that answers an uncovered peer PREPARE with the same
/// greeting (issue #880). Declared here rather than spelled at either use
/// site so the two halves stay a pair, exactly like the three above.
pub const PAYMENT_REQUIRED_HEADER: &str = "payment-required";
/// **The one field with no BTP twin** (§3, §6.4): the flush prompt a payee
/// that cannot originate MAY set on a response. BTP needs none, because on
/// BTP the payee can originate a request of its own -- so this constant
/// deliberately does not appear in [`CARRIAGE_NAMES`], and §10.2 item 17
/// requires the vector set record that absence explicitly rather than
/// leaving the pair incomplete.
pub const FLUSH_REQUESTED_HEADER: &str = "toon-flush-requested";

/// One concept's two names: what it is called on each carriage.
///
/// A pair rather than two constants because the pairing *is* the invariant
/// (spec I2). A carriage reads a name out of this table; it never spells one
/// itself, so a second `const` naming half a concept somewhere else is
/// exactly the fork issue #713 was opened to prevent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CarriageNames {
    /// What the concept is called, for a diagnostic that has to name it.
    pub concept: &'static str,
    /// The BTP `protocolData` entry name.
    pub btp_protocol_entry: &'static str,
    /// The HTTP header name, canonical lower-case.
    pub http_header: &'static str,
}

/// Every §3 field that rides both carriages, as pairs.
///
/// The peer credential is the one pair declared elsewhere --
/// `connector_peer_auth::PEER_AUTH_NAMES` -- because the crate that *decides
/// role* must not be able to name a frame or a socket (§1.3), so it cannot
/// depend on this one. Its own test asserts its BTP half is
/// [`AUTH_PROTOCOL`], which is the same pairing discipline reached from the
/// other side.
pub const CARRIAGE_NAMES: &[CarriageNames] = &[
    CarriageNames {
        concept: "claim",
        btp_protocol_entry: CLAIM_PROTOCOL,
        http_header: CLAIM_HEADER,
    },
    CarriageNames {
        concept: "claim-ack",
        btp_protocol_entry: CLAIM_ACK_PROTOCOL,
        http_header: CLAIM_ACK_HEADER,
    },
    CarriageNames {
        concept: "accumulated-cost",
        btp_protocol_entry: ACCUMULATED_COST_PROTOCOL,
        http_header: ACCUMULATED_COST_HEADER,
    },
    CarriageNames {
        concept: "payment-required",
        btp_protocol_entry: PAYMENT_REQUIRED_PROTOCOL,
        http_header: PAYMENT_REQUIRED_HEADER,
    },
    CarriageNames {
        concept: "peer-role-challenge",
        btp_protocol_entry: PEER_CHALLENGE_PROTOCOL,
        http_header: PEER_CHALLENGE_HEADER,
    },
];

/// The contentType the client itself uses for its JSON claim entry (its
/// parser never reads the field back). Emitted on every server entry for
/// consistency rather than meaning.
pub const CONTENT_TYPE_TEXT: u16 = 1;

/// One decoded protocolData entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolData {
    pub name: String,
    pub content_type: u16,
    pub data: Vec<u8>,
}

/// A decoded MESSAGE/RESPONSE/TRANSFER body: the protocolData list plus the
/// ILP packet riding beside it (empty = none; the dialect writes a zero
/// length rather than omitting the field) and, for a TRANSFER, the
/// settlement amount RFC-0023's `Transfer ::= SEQUENCE { amount, protocolData
/// }` carries (`None` for every other frame type -- `ilp_packet` is likewise
/// always empty on a TRANSFER, which has no ILP-packet field in either RFC-23
/// or this dialect's extension of it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BtpFrame {
    pub frame_type: u8,
    pub request_id: u32,
    pub amount: Option<u64>,
    pub protocol_data: Vec<ProtocolData>,
    pub ilp_packet: Vec<u8>,
}

/// Why a frame did not decode. `TooShort` means not even the 5-byte
/// `type + requestId` prefix was readable, so there is no id to answer an
/// ERROR to; `Malformed` carries the id so the answer can correlate.
#[derive(Debug, PartialEq, Eq)]
pub enum BtpDecodeError {
    TooShort,
    Malformed { request_id: u32, reason: String },
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
    request_id: u32,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize, what: &str) -> Result<&'a [u8], BtpDecodeError> {
        let end = self.pos.checked_add(n).filter(|end| *end <= self.buf.len());
        let Some(end) = end else {
            return Err(BtpDecodeError::Malformed {
                request_id: self.request_id,
                reason: format!("frame truncated reading {what}"),
            });
        };
        let slice = &self.buf[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn u8(&mut self, what: &str) -> Result<u8, BtpDecodeError> {
        Ok(self.take(1, what)?[0])
    }

    fn u16(&mut self, what: &str) -> Result<u16, BtpDecodeError> {
        let bytes = self.take(2, what)?;
        Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
    }

    fn u32(&mut self, what: &str) -> Result<u32, BtpDecodeError> {
        let bytes = self.take(4, what)?;
        Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn u64(&mut self, what: &str) -> Result<u64, BtpDecodeError> {
        let bytes = self.take(8, what)?;
        Ok(u64::from_be_bytes(bytes.try_into().expect("8 bytes read")))
    }

    fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }
}

/// Decode one frame. MESSAGE/RESPONSE/TRANSFER bodies are given structure --
/// an ERROR frame from a client answers nothing and is skipped by the
/// session loop on its type alone, so its body is preserved undecoded in
/// `ilp_packet`-less form (empty protocolData, raw bytes discarded).
/// Trailing bytes beyond the declared ILP length are ignored, exactly as
/// the client's own parser ignores them. Adding TRANSFER (issue #697) does
/// not alter how any other frame_type is read -- this is purely a new match
/// arm on the leading type byte, so the MESSAGE/RESPONSE/ERROR paths below
/// are unchanged from before RFC-23's symmetric grammar landed.
pub fn decode_frame(buf: &[u8]) -> Result<BtpFrame, BtpDecodeError> {
    if buf.len() < 5 {
        return Err(BtpDecodeError::TooShort);
    }
    let frame_type = buf[0];
    let request_id = u32::from_be_bytes([buf[1], buf[2], buf[3], buf[4]]);
    if frame_type == BTP_ERROR {
        return Ok(BtpFrame {
            frame_type,
            request_id,
            amount: None,
            protocol_data: Vec::new(),
            ilp_packet: Vec::new(),
        });
    }
    let mut reader = Reader {
        buf,
        pos: 5,
        request_id,
    };
    // RFC-0023's `Transfer ::= SEQUENCE { amount, protocolData }`: the
    // amount precedes the protocolData list and there is no ILP-packet
    // trailer (unlike MESSAGE/RESPONSE, TRANSFER carries settlement value,
    // not a routed packet).
    let amount = if frame_type == BTP_TRANSFER {
        Some(reader.u64("TRANSFER amount")?)
    } else {
        None
    };
    let count = reader.u8("protocolData count")?;
    let mut protocol_data = Vec::with_capacity(usize::from(count));
    for _ in 0..count {
        let name_len = reader.u8("protocolData name length")?;
        let name_bytes = reader.take(usize::from(name_len), "protocolData name")?;
        let name =
            String::from_utf8(name_bytes.to_vec()).map_err(|_| BtpDecodeError::Malformed {
                request_id,
                reason: "protocolData name is not valid UTF-8".to_string(),
            })?;
        let content_type = reader.u16("protocolData contentType")?;
        let data_len = reader.u32("protocolData length")?;
        let data = reader
            .take(data_len as usize, "protocolData bytes")?
            .to_vec();
        protocol_data.push(ProtocolData {
            name,
            content_type,
            data,
        });
    }
    // The dialect always writes the trailing ILP length on MESSAGE/RESPONSE,
    // but the client's own parser tolerates its absence -- mirror that
    // tolerance. TRANSFER has no such trailer at all (RFC-23's Transfer
    // packet ends at protocolData), so it is never read here.
    let ilp_packet = if frame_type != BTP_TRANSFER && reader.remaining() >= 4 {
        let ilp_len = reader.u32("ILP packet length")?;
        reader.take(ilp_len as usize, "ILP packet bytes")?.to_vec()
    } else {
        Vec::new()
    };
    Ok(BtpFrame {
        frame_type,
        request_id,
        amount,
        protocol_data,
        ilp_packet,
    })
}

/// Append `protocolData count` + each entry (`nameLen name contentType
/// dataLen data`), the layout every frame type below shares.
fn write_protocol_data(out: &mut Vec<u8>, protocol_data: &[ProtocolData]) {
    out.push(protocol_data.len() as u8);
    for pd in protocol_data {
        let name = pd.name.as_bytes();
        out.push(name.len() as u8);
        out.extend_from_slice(name);
        out.extend_from_slice(&pd.content_type.to_be_bytes());
        out.extend_from_slice(&(pd.data.len() as u32).to_be_bytes());
        out.extend_from_slice(&pd.data);
    }
}

/// Encode a RESPONSE frame answering `request_id`, carrying `protocol_data`
/// and `ilp_packet` (empty = none; the length field is always written, as
/// the client serializer does).
pub fn encode_response(
    request_id: u32,
    protocol_data: &[ProtocolData],
    ilp_packet: &[u8],
) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(BTP_RESPONSE);
    out.extend_from_slice(&request_id.to_be_bytes());
    write_protocol_data(&mut out, protocol_data);
    out.extend_from_slice(&(ilp_packet.len() as u32).to_be_bytes());
    out.extend_from_slice(ilp_packet);
    out
}

/// Encode a MESSAGE frame (issue #697): identical body layout to
/// [`encode_response`], under the MESSAGE type byte. This is what a
/// server-originated request looks like on the wire -- the connector
/// addresses `request_id` (allocated by
/// [`OutboundRequests::reserve`](crate::OutboundRequests::reserve)) the same
/// way the client addresses one of its own.
pub fn encode_message(
    request_id: u32,
    protocol_data: &[ProtocolData],
    ilp_packet: &[u8],
) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(BTP_MESSAGE);
    out.extend_from_slice(&request_id.to_be_bytes());
    write_protocol_data(&mut out, protocol_data);
    out.extend_from_slice(&(ilp_packet.len() as u32).to_be_bytes());
    out.extend_from_slice(ilp_packet);
    out
}

/// Encode a TRANSFER frame (issue #697, RFC-0023 `Transfer ::= SEQUENCE {
/// amount, protocolData }`): the settlement `amount` immediately follows
/// `request_id`, then the protocolData list -- no ILP-packet trailer, unlike
/// MESSAGE/RESPONSE. Used to originate one outright via
/// [`BtpSessionHandle::send_transfer`](crate::BtpSessionHandle::send_transfer);
/// the client edge's inbound TRANSFER ack answers with an empty RESPONSE,
/// not one of these.
pub fn encode_transfer(request_id: u32, amount: u64, protocol_data: &[ProtocolData]) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(BTP_TRANSFER);
    out.extend_from_slice(&request_id.to_be_bytes());
    out.extend_from_slice(&amount.to_be_bytes());
    write_protocol_data(&mut out, protocol_data);
    out
}

/// Encode an ERROR frame (§1.9 step 5): `code`/`name`/`triggeredAt` as
/// 1-byte-length-prefixed strings, then the diagnostic text as a
/// u32-length-prefixed trailer, per the client's `parseBtpMessage` ERROR
/// arm. `triggeredAt` is deliberately empty: this crate carries no clock,
/// and the client surfaces it only inside a human-readable message.
pub fn encode_error(request_id: u32, code: &str, name: &str, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(BTP_ERROR);
    out.extend_from_slice(&request_id.to_be_bytes());
    for field in [code, name, ""] {
        let bytes = field.as_bytes();
        out.push(bytes.len() as u8);
        out.extend_from_slice(bytes);
    }
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(data);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// §1.9's frame grammar, byte-for-byte: the exact bytes
    /// `@toon-protocol/client`'s `serializeBtpMessage` produces for its auth
    /// frame (`serializeBtpMessage({type: 6, requestId: 1, data:
    /// {protocolData: [{protocolName: 'auth', contentType: 0, data:
    /// utf8('{"peerId":"p","secret":""}')}], ilpPacket: new Uint8Array(0)}})`).
    /// These vectors pin the dialect; prose is not the thing to conform to
    /// (ADR 0021).
    fn auth_frame_vector() -> Vec<u8> {
        let auth_json = br#"{"peerId":"p","secret":""}"#;
        let mut frame = vec![
            6, // MESSAGE
            0, 0, 0, 1, // requestId 1
            1, // one protocolData entry
            4, // nameLen
        ];
        frame.extend_from_slice(b"auth");
        frame.extend_from_slice(&[0, 0]); // contentType 0
        frame.extend_from_slice(&(auth_json.len() as u32).to_be_bytes());
        frame.extend_from_slice(auth_json);
        frame.extend_from_slice(&[0, 0, 0, 0]); // ilpPacket length 0
        frame
    }

    #[test]
    fn the_clients_auth_frame_decodes_exactly() {
        let frame = decode_frame(&auth_frame_vector()).expect("the client's own bytes decode");
        assert_eq!(frame.frame_type, BTP_MESSAGE);
        assert_eq!(frame.request_id, 1);
        assert_eq!(frame.protocol_data.len(), 1);
        assert_eq!(frame.protocol_data[0].name, "auth");
        assert_eq!(frame.protocol_data[0].content_type, 0);
        assert_eq!(
            frame.protocol_data[0].data,
            br#"{"peerId":"p","secret":""}"#.to_vec()
        );
        assert!(frame.ilp_packet.is_empty());
    }

    #[test]
    fn a_message_carrying_claim_and_packet_decodes_both() {
        let claim = br#"{"version":"1.0"}"#;
        let ilp = [12u8, 0, 1, 2, 3];
        let mut frame = vec![6, 0, 0, 0, 42, 1];
        frame.push(CLAIM_PROTOCOL.len() as u8);
        frame.extend_from_slice(CLAIM_PROTOCOL.as_bytes());
        frame.extend_from_slice(&1u16.to_be_bytes());
        frame.extend_from_slice(&(claim.len() as u32).to_be_bytes());
        frame.extend_from_slice(claim);
        frame.extend_from_slice(&(ilp.len() as u32).to_be_bytes());
        frame.extend_from_slice(&ilp);

        let decoded = decode_frame(&frame).expect("decodes");
        assert_eq!(decoded.request_id, 42);
        assert_eq!(decoded.protocol_data[0].name, CLAIM_PROTOCOL);
        assert_eq!(decoded.protocol_data[0].data, claim.to_vec());
        assert_eq!(decoded.ilp_packet, ilp.to_vec());
    }

    #[test]
    fn an_encoded_response_is_the_dialects_bytes() {
        let encoded = encode_response(7, &[], b"\x0d\x01\x02");
        // type RESPONSE, requestId 7, no protocolData, ilpLen 3, packet.
        assert_eq!(
            encoded,
            vec![1, 0, 0, 0, 7, 0, 0, 0, 0, 3, 0x0d, 0x01, 0x02]
        );
    }

    #[test]
    fn a_response_round_trips_through_the_decoder() {
        let pd = vec![ProtocolData {
            name: ACCUMULATED_COST_PROTOCOL.to_string(),
            content_type: CONTENT_TYPE_TEXT,
            data: b"1000".to_vec(),
        }];
        let encoded = encode_response(9, &pd, b"reject-bytes");
        let decoded = decode_frame(&encoded).expect("round-trips");
        assert_eq!(decoded.frame_type, BTP_RESPONSE);
        assert_eq!(decoded.request_id, 9);
        assert_eq!(decoded.protocol_data, pd);
        assert_eq!(decoded.ilp_packet, b"reject-bytes".to_vec());
    }

    #[test]
    fn an_encoded_transfer_is_amount_then_protocol_data_with_no_ilp_trailer() {
        let pd = vec![ProtocolData {
            name: "payout-claim".to_string(),
            content_type: CONTENT_TYPE_TEXT,
            data: b"{}".to_vec(),
        }];
        let encoded = encode_transfer(11, 1_000_000, &pd);
        // type TRANSFER, requestId 11, amount 1_000_000, one protocolData
        // entry, no trailing ILP length -- RFC-23's Transfer packet has no
        // ILP-packet field.
        let mut expected = vec![7, 0, 0, 0, 11];
        expected.extend_from_slice(&1_000_000u64.to_be_bytes());
        expected.push(1);
        expected.push(12); // "payout-claim".len()
        expected.extend_from_slice(b"payout-claim");
        expected.extend_from_slice(&1u16.to_be_bytes());
        expected.extend_from_slice(&2u32.to_be_bytes());
        expected.extend_from_slice(b"{}");
        assert_eq!(encoded, expected);
    }

    #[test]
    fn a_transfer_round_trips_through_the_decoder() {
        let pd = vec![ProtocolData {
            name: "payout-claim".to_string(),
            content_type: CONTENT_TYPE_TEXT,
            data: b"claim-bytes".to_vec(),
        }];
        let encoded = encode_transfer(5, 42, &pd);
        let decoded = decode_frame(&encoded).expect("round-trips");
        assert_eq!(decoded.frame_type, BTP_TRANSFER);
        assert_eq!(decoded.request_id, 5);
        assert_eq!(decoded.amount, Some(42));
        assert_eq!(decoded.protocol_data, pd);
        assert!(
            decoded.ilp_packet.is_empty(),
            "TRANSFER carries no ILP packet"
        );
    }

    #[test]
    fn a_transfer_with_no_protocol_data_decodes_to_an_empty_list() {
        let encoded = encode_transfer(6, 0, &[]);
        assert_eq!(encoded, vec![7, 0, 0, 0, 6, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        let decoded = decode_frame(&encoded).expect("decodes");
        assert_eq!(decoded.amount, Some(0));
        assert!(decoded.protocol_data.is_empty());
    }

    #[test]
    fn a_truncated_transfer_reports_malformed_with_its_request_id() {
        // Type TRANSFER, requestId 8, five of the eight amount bytes.
        let frame = [7u8, 0, 0, 0, 8, 0, 0, 0, 0, 0];
        assert_eq!(
            decode_frame(&frame),
            Err(BtpDecodeError::Malformed {
                request_id: 8,
                reason: "frame truncated reading TRANSFER amount".to_string(),
            })
        );
    }

    #[test]
    fn a_message_still_decodes_exactly_as_before_transfer_was_added() {
        // Non-regression for issue #697: a MESSAGE's `amount` is `None`,
        // and its bytes decode identically to pre-TRANSFER behavior.
        let decoded = decode_frame(&auth_frame_vector()).expect("decodes");
        assert_eq!(decoded.amount, None);
    }

    #[test]
    fn an_error_frame_matches_the_clients_error_parser_layout() {
        let encoded = encode_error(3, "F00", "NotAcceptedError", b"boom");
        // The client's parseBtpMessage ERROR arm: 1-byte-length-prefixed
        // code, name, triggeredAt (empty), then u32-length-prefixed data.
        let mut expected = vec![2, 0, 0, 0, 3];
        expected.push(3);
        expected.extend_from_slice(b"F00");
        expected.push(16);
        expected.extend_from_slice(b"NotAcceptedError");
        expected.push(0); // empty triggeredAt
        expected.extend_from_slice(&4u32.to_be_bytes());
        expected.extend_from_slice(b"boom");
        assert_eq!(encoded, expected);
    }

    #[test]
    fn a_truncated_frame_reports_malformed_with_its_request_id() {
        // Claims one protocolData entry, provides nothing after the count.
        let frame = [6u8, 0, 0, 0, 5, 1];
        assert_eq!(
            decode_frame(&frame),
            Err(BtpDecodeError::Malformed {
                request_id: 5,
                reason: "frame truncated reading protocolData name length".to_string(),
            })
        );
    }

    #[test]
    fn a_frame_shorter_than_its_header_is_too_short() {
        assert_eq!(decode_frame(&[6, 0, 0]), Err(BtpDecodeError::TooShort));
    }

    /// Spec I2, mechanically: every §3 field that rides both carriages
    /// declares **both** halves, and both are non-empty. A header added
    /// without its protocolData twin cannot even be written down here.
    #[test]
    fn every_carriage_field_declares_both_halves() {
        assert!(!CARRIAGE_NAMES.is_empty(), "the pairing table is the point");
        for names in CARRIAGE_NAMES {
            assert!(
                !names.btp_protocol_entry.is_empty(),
                "'{}' has no BTP entry name",
                names.concept
            );
            assert!(
                !names.http_header.is_empty(),
                "'{}' has no HTTP header name",
                names.concept
            );
            assert_eq!(
                names.http_header,
                names.http_header.to_ascii_lowercase(),
                "'{}' declares its header in a non-canonical case (§3)",
                names.concept
            );
        }
    }

    /// The pairs are the constants, not copies of them: a carriage reading
    /// the table reads exactly what a carriage reading the constant reads.
    #[test]
    fn the_pairing_table_names_the_declared_constants() {
        let by_concept = |concept: &str| {
            *CARRIAGE_NAMES
                .iter()
                .find(|names| names.concept == concept)
                .expect("a declared concept")
        };

        assert_eq!(by_concept("claim").btp_protocol_entry, CLAIM_PROTOCOL);
        assert_eq!(by_concept("claim").http_header, CLAIM_HEADER);
        assert_eq!(
            by_concept("claim-ack").btp_protocol_entry,
            CLAIM_ACK_PROTOCOL
        );
        assert_eq!(by_concept("claim-ack").http_header, CLAIM_ACK_HEADER);
        assert_eq!(
            by_concept("accumulated-cost").http_header,
            ACCUMULATED_COST_HEADER
        );
        assert_eq!(
            by_concept("payment-required").btp_protocol_entry,
            PAYMENT_REQUIRED_PROTOCOL
        );
        assert_eq!(
            by_concept("payment-required").http_header,
            PAYMENT_REQUIRED_HEADER
        );
        assert_eq!(
            by_concept("peer-role-challenge").btp_protocol_entry,
            PEER_CHALLENGE_PROTOCOL
        );
        assert_eq!(
            by_concept("peer-role-challenge").http_header,
            PEER_CHALLENGE_HEADER
        );
    }

    /// §6.4/§10.2 item 17: the flush prompt exists on HTTP only, and the
    /// absence of a BTP twin is recorded rather than left as an incomplete
    /// pair. BTP needs none -- there the payee can originate a request of
    /// its own.
    #[test]
    fn the_flush_prompt_is_http_only_and_pairs_with_nothing() {
        assert_eq!(FLUSH_REQUESTED_HEADER, "toon-flush-requested");
        assert!(
            !CARRIAGE_NAMES
                .iter()
                .any(|names| names.http_header == FLUSH_REQUESTED_HEADER),
            "the flush prompt has no BTP twin and must not claim one"
        );
    }

    #[test]
    fn a_missing_trailing_ilp_length_is_tolerated_as_no_packet() {
        // The client's own parser treats an absent trailing length as no
        // packet; the server mirrors it.
        let frame = [6u8, 0, 0, 0, 8, 0];
        let decoded = decode_frame(&frame).expect("decodes");
        assert!(decoded.ilp_packet.is_empty());
        assert!(decoded.protocol_data.is_empty());
    }
}
