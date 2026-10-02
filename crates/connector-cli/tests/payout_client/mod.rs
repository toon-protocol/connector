//! The client half of the client-payout end-to-end tests (ADR 0075
//! decision 7, issue #1381): a real BTP websocket session against a served
//! node, written against client-edge-spec.md §1.9's grammar independently
//! of the server's own codec, and the payout voucher read back off it into
//! what the client lands on chain.

#![allow(dead_code)]

use std::net::SocketAddr;
use std::time::Duration;

use chrono::{TimeZone, Utc};
use connector_domain::{Fulfill, Prepare};
use connector_settlement::batch::{ChannelPresentation, EvmChannelConfig, Voucher};
use connector_settlement::ChannelId;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message as WsMessage;

pub type Session =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

const BTP_RESPONSE: u8 = 1;
const BTP_MESSAGE: u8 = 6;
const BTP_TRANSFER: u8 = 7;

fn message(request_id: u32, protocol_data: &[(&str, &[u8])], ilp_packet: &[u8]) -> Vec<u8> {
    let mut out = vec![BTP_MESSAGE];
    out.extend_from_slice(&request_id.to_be_bytes());
    out.push(protocol_data.len() as u8);
    for (name, data) in protocol_data {
        out.push(name.len() as u8);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&1u16.to_be_bytes());
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(data);
    }
    out.extend_from_slice(&(ilp_packet.len() as u32).to_be_bytes());
    out.extend_from_slice(ilp_packet);
    out
}

fn response(request_id: u32, ilp_packet: &[u8]) -> Vec<u8> {
    let mut out = vec![BTP_RESPONSE];
    out.extend_from_slice(&request_id.to_be_bytes());
    out.push(0);
    out.extend_from_slice(&(ilp_packet.len() as u32).to_be_bytes());
    out.extend_from_slice(ilp_packet);
    out
}

pub struct Frame {
    pub frame_type: u8,
    pub request_id: u32,
    pub amount: Option<u64>,
    pub protocol_data: Vec<(String, Vec<u8>)>,
    pub ilp_packet: Vec<u8>,
}

fn parse(buf: &[u8]) -> Frame {
    let frame_type = buf[0];
    let request_id = u32::from_be_bytes(buf[1..5].try_into().unwrap());
    let mut pos = 5;
    let amount = (frame_type == BTP_TRANSFER).then(|| {
        let amount = u64::from_be_bytes(buf[pos..pos + 8].try_into().unwrap());
        pos += 8;
        amount
    });
    let count = usize::from(buf[pos]);
    pos += 1;
    let mut protocol_data = Vec::new();
    for _ in 0..count {
        let name_len = usize::from(buf[pos]);
        pos += 1;
        let name = String::from_utf8(buf[pos..pos + name_len].to_vec()).unwrap();
        pos += name_len + 2;
        let len = u32::from_be_bytes(buf[pos..pos + 4].try_into().unwrap()) as usize;
        pos += 4;
        protocol_data.push((name, buf[pos..pos + len].to_vec()));
        pos += len;
    }
    let ilp_packet = if frame_type == BTP_TRANSFER {
        Vec::new()
    } else {
        let len = u32::from_be_bytes(buf[pos..pos + 4].try_into().unwrap()) as usize;
        buf[pos + 4..pos + 4 + len].to_vec()
    };
    Frame {
        frame_type,
        request_id,
        amount,
        protocol_data,
        ilp_packet,
    }
}

async fn next_frame(session: &mut Session) -> Frame {
    loop {
        let next = tokio::time::timeout(Duration::from_secs(60), session.next())
            .await
            .expect("the node sends the next frame within a minute");
        if let WsMessage::Binary(bytes) = next.expect("open").expect("a frame") {
            return parse(&bytes);
        }
    }
}

async fn send(session: &mut Session, frame: Vec<u8>) {
    session
        .send(WsMessage::Binary(frame))
        .await
        .expect("the socket takes the frame");
}

/// Connect to the node at `node`, bind `address`, and pay the node
/// `voucher` on the client's own channel -- riding a zero-value packet to
/// `g.toon.nowhere` -- a route the node's config must serve, since a voucher
/// riding a packet to a destination nothing serves is never looked at (issue
/// #1446) -- so its answer says the voucher has been judged and the session
/// knows its payee.
pub async fn client_session(node: SocketAddr, address: &str, voucher: &str) -> Session {
    let (mut session, _) = tokio_tungstenite::connect_async(format!("ws://{node}/ilp/btp"))
        .await
        .expect("the upgrade succeeds");
    let auth = format!(r#"{{"peerId":"{address}","secret":""}}"#);
    send(&mut session, message(1, &[("auth", auth.as_bytes())], &[])).await;
    assert_eq!(next_frame(&mut session).await.frame_type, BTP_RESPONSE);
    let probe = Prepare {
        amount: 0,
        expires_at: Utc.with_ymd_and_hms(2031, 1, 1, 0, 0, 0).unwrap(),
        greeting: false,
        destination: "g.toon.nowhere".to_string(),
        data: Vec::new(),
    };
    send(
        &mut session,
        message(
            2,
            &[("payment-channel-claim", voucher.as_bytes())],
            &probe.encode(),
        ),
    )
    .await;
    let answer = next_frame(&mut session).await;
    assert_eq!((answer.frame_type, answer.request_id), (BTP_RESPONSE, 2));
    session
}

/// A buyer's PREPARE for `amount` to `address`, over HTTP; the client's
/// session fulfils it. Returns the payout TRANSFER that follows on the
/// client's socket, acknowledged.
pub async fn earn(
    node: SocketAddr,
    session: &mut Session,
    address: &str,
    amount: u64,
    tag: u8,
) -> Frame {
    let prepare = Prepare {
        amount,
        expires_at: Utc.with_ymd_and_hms(2031, 1, 1, 0, 0, 0).unwrap(),
        greeting: false,
        destination: address.to_string(),
        data: vec![tag],
    };
    let body = prepare.encode();
    let buyer = tokio::spawn(async move {
        let response = hyper::Client::new()
            .request(
                hyper::Request::builder()
                    .method("POST")
                    .uri(format!("http://{node}/ilp"))
                    .body(hyper::Body::from(body))
                    .unwrap(),
            )
            .await
            .expect("the node answers");
        hyper::body::to_bytes(response.into_body()).await.unwrap()
    });

    let forwarded = next_frame(session).await;
    assert_eq!(forwarded.frame_type, BTP_MESSAGE);
    let fulfillment = [tag; 32];
    send(
        session,
        response(
            forwarded.request_id,
            &Fulfill {
                fulfillment,
                data: Vec::new(),
            }
            .encode(),
        ),
    )
    .await;

    let transfer = next_frame(session).await;
    assert_eq!(
        transfer.frame_type, BTP_TRANSFER,
        "the payout rides a TRANSFER"
    );
    send(session, response(transfer.request_id, &[])).await;
    let answered = Fulfill::decode(&buyer.await.unwrap()).expect("the buyer's packet fulfilled");
    assert_eq!(answered.fulfillment, fulfillment);
    transfer
}

/// The payout voucher a TRANSFER carries, read back into what the client
/// lands: its JSON, the channel as the port presents it, and the voucher.
pub fn payout_of(transfer: &Frame) -> (serde_json::Value, ChannelPresentation, Voucher) {
    let data = &transfer
        .protocol_data
        .iter()
        .find(|(name, _)| name == "payout-claim")
        .expect("the TRANSFER carries the payout voucher")
        .1;
    let json: serde_json::Value = serde_json::from_slice(data).expect("JSON");
    assert_eq!(json["scheme"], "batch-settlement");
    let channel = ChannelId(json["channelId"].as_str().unwrap().to_string());
    let cumulative_amount: u128 = json["maxClaimableAmount"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let hex = |text: &serde_json::Value| -> Vec<u8> {
        let text = text.as_str().unwrap().trim_start_matches("0x");
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
            .collect()
    };
    let (presentation, signature) = match json["blockchain"].as_str().unwrap() {
        "evm" => {
            let config = &json["channelConfig"];
            let address = |field: &str| -> [u8; 20] { hex(&config[field]).try_into().unwrap() };
            let presentation = ChannelPresentation::Evm {
                channel,
                config: EvmChannelConfig {
                    payer: address("payer"),
                    payer_authorizer: address("payerAuthorizer"),
                    receiver: address("receiver"),
                    receiver_authorizer: address("receiverAuthorizer"),
                    token: address("token"),
                    withdraw_delay: config["withdrawDelay"].as_u64().unwrap(),
                    salt: hex(&config["salt"]).try_into().unwrap(),
                },
            };
            (presentation, hex(&json["signature"]))
        }
        "solana" => {
            assert_eq!(json["expiresAt"], 0);
            let signature = bs58::decode(json["signature"].as_str().unwrap())
                .into_vec()
                .unwrap();
            (ChannelPresentation::Solana { channel }, signature)
        }
        other => panic!("unexpected chain {other}"),
    };
    assert!(
        json.get("nonce").is_none(),
        "a payout is a voucher, never a toon-channel claim: {json}"
    );
    (
        json,
        presentation,
        Voucher {
            cumulative_amount,
            signature,
        },
    )
}
