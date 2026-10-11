//! ADR 0075 decision 7, issue #1381: a client's payout is a **voucher** on
//! an x402 channel this connector opened toward it, delivered over the
//! client's own BTP session -- driven through the client edge's served
//! router, a real websocket, the real claim gate and the real payout ledger
//! over the settlement port's in-memory fake (ADR 0007: a fake that upholds
//! the port's contract suite, not a mock).
//!
//! The client is a real key on both chains. It pays this node a voucher on
//! its own channel -- verified cryptographically by the gate against the
//! signer the (fake) chain records -- which teaches the gate who the
//! session is paid as. The operator has opened a payout channel toward that
//! key. A PREPARE to the client's address is then fulfilled by the client's
//! session, and the payout voucher that comes back over the same socket is
//! one the client lands on the fake chain itself, with nothing more from
//! this node.

use std::net::SocketAddr;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use connector_client_edge::{
    AdmittedEvmVoucherChannel, AdmittedSolanaVoucherChannel, BatchSettlementChannels,
    ChannelResolutionError, ClientClaimGate, ClientPayoutLedger,
};
use connector_domain::{Fulfill, Prepare};
use connector_runtime::{
    Connector, FakeAppClient, FileJournal, InProcessPeerTransport, Journal, OutboundChannels,
    SettlementChain, TestClock,
};
use connector_settlement::batch::{
    BatchSettlementBackend, BatchSettlementPayer, ChannelPresentation, EvmChannelConfig,
    EvmReceiverTerms, InMemoryBatchChain, InMemoryBatchSettlement, PayerExit, ReceiverTerms,
    SolanaReceiverTerms, Voucher,
};
use connector_settlement::ChannelId;
use connector_signer::{
    derive_evm_address, evm_batch_channel_id, evm_voucher_digest, solana_voucher_message,
    BatchChannelConfig, BatchSettlementDomain, LocalSigner, Signer,
};
use ed25519_dalek::Signer as _;
use futures_util::{SinkExt, StreamExt};
use hyper::{Body as HttpBody, Client as HttpClient, Request as HttpRequest, StatusCode};
use libsecp256k1::{Message, PublicKey, SecretKey};
use tokio_tungstenite::tungstenite::Message as WsMessage;

const CHAIN_ID: u64 = 84_532;
const ONE_DAY: u64 = 86_400;
/// The fake chain's settled token, as `InMemoryBatchSettlement` names it.
const TOKEN: u8 = 0x70;
/// This connector, on the fake chain.
const CONNECTOR: u8 = 0x01;
/// What the client may put in its own channel toward this node.
const CLIENT_COLLATERAL: u64 = 1_000;
/// What the operator funded the payout channel with.
const PAYOUT_DEPOSIT: u128 = 5_000;
const ADDRESS: &str = "g.toon.earner";

// ─── the client's keys, and its own channel toward this node ───

fn client_secret() -> SecretKey {
    SecretKey::parse(&[0x45; 32]).expect("valid secret")
}

fn client_address() -> [u8; 20] {
    derive_evm_address(&PublicKey::from_secret_key(&client_secret()).serialize())
}

fn client_ed25519() -> ed25519_dalek::Keypair {
    let secret = ed25519_dalek::SecretKey::from_bytes(&[0x46; 32]).expect("seed");
    let public = (&secret).into();
    ed25519_dalek::Keypair { secret, public }
}

/// A second client, with a key of its own, that declares the same `peerId`
/// as the first (issue #1396).
fn other_secret() -> SecretKey {
    SecretKey::parse(&[0x48; 32]).expect("valid secret")
}

fn address_of(secret: &SecretKey) -> [u8; 20] {
    derive_evm_address(&PublicKey::from_secret_key(secret).serialize())
}

fn domain() -> BatchSettlementDomain {
    BatchSettlementDomain::x402(CHAIN_ID)
}

/// The EVM channel toward this node that the holder of `secret` pays on,
/// signing its vouchers with its own key.
fn channel_config_of(secret: &SecretKey) -> BatchChannelConfig {
    let key = address_of(secret);
    BatchChannelConfig {
        payer: key,
        payer_authorizer: key,
        receiver: [0x33; 20],
        receiver_authorizer: [0x33; 20],
        token: [0x55; 20],
        withdraw_delay: ONE_DAY,
        salt: [0x66; 32],
    }
}

const CLIENT_SOLANA_CHANNEL: [u8; 32] = [0xc3; 32];

fn hex20(bytes: &[u8; 20]) -> String {
    format!("0x{}", hex::encode(bytes))
}

/// The client's voucher for `amount` on its own EVM channel, as it sends one.
fn client_evm_voucher(amount: u64) -> String {
    evm_voucher_by(&client_secret(), amount)
}

/// The voucher for `amount` the holder of `secret` signs on its own EVM
/// channel toward this node.
fn evm_voucher_by(secret: &SecretKey, amount: u64) -> String {
    let config = channel_config_of(secret);
    let channel = evm_batch_channel_id(&domain(), &config);
    let digest = evm_voucher_digest(&domain(), &channel, u128::from(amount));
    let (signature, recovery) = libsecp256k1::sign(&Message::parse(&digest), secret);
    let mut bytes = signature.serialize().to_vec();
    bytes.push(recovery.serialize() + 27);
    serde_json::json!({
        "version": "1.0",
        "blockchain": "evm",
        "scheme": "batch-settlement",
        "messageId": format!("voucher-{amount}"),
        "timestamp": "2026-09-27T12:00:00.000Z",
        "senderId": "earner",
        "channelId": format!("0x{}", hex::encode(channel)),
        "maxClaimableAmount": amount.to_string(),
        "signature": format!("0x{}", hex::encode(bytes)),
        "channelConfig": {
            "payer": hex20(&config.payer),
            "payerAuthorizer": hex20(&config.payer_authorizer),
            "receiver": hex20(&config.receiver),
            "receiverAuthorizer": hex20(&config.receiver_authorizer),
            "token": hex20(&config.token),
            "withdrawDelay": config.withdraw_delay,
            "salt": format!("0x{}", hex::encode(config.salt)),
        },
    })
    .to_string()
}

/// The client's voucher for `amount` on its own Solana channel.
fn client_solana_voucher(amount: u64) -> String {
    let signature =
        client_ed25519().sign(&solana_voucher_message(&CLIENT_SOLANA_CHANNEL, amount, 0));
    serde_json::json!({
        "version": "1.0",
        "blockchain": "solana",
        "scheme": "batch-settlement",
        "messageId": format!("voucher-{amount}"),
        "timestamp": "2026-09-27T12:00:00Z",
        "senderId": "earner",
        "channelId": bs58::encode(CLIENT_SOLANA_CHANNEL).into_string(),
        "maxClaimableAmount": amount.to_string(),
        "expiresAt": 0,
        "signature": bs58::encode(signature.to_bytes()).into_string(),
    })
    .to_string()
}

/// The client's inbound channels as the gate's backend finds them: the
/// signer is the chain's record, never the voucher's say-so.
#[derive(Debug)]
struct ClientChannels;

#[async_trait]
impl BatchSettlementChannels for ClientChannels {
    fn evm_domain(&self) -> Option<BatchSettlementDomain> {
        Some(domain())
    }

    fn accepts_solana(&self) -> bool {
        true
    }

    async fn evm(
        &self,
        channel_id: &[u8; 32],
        _presented_config: Option<&BatchChannelConfig>,
    ) -> Result<Option<AdmittedEvmVoucherChannel>, ChannelResolutionError> {
        Ok([client_secret(), other_secret()]
            .iter()
            .map(channel_config_of)
            .find(|config| *channel_id == evm_batch_channel_id(&domain(), config))
            .map(|config| AdmittedEvmVoucherChannel {
                config,
                max_cumulative: u128::from(CLIENT_COLLATERAL),
            }))
    }

    async fn solana(
        &self,
        channel_account: &[u8; 32],
    ) -> Result<Option<AdmittedSolanaVoucherChannel>, ChannelResolutionError> {
        Ok(
            (*channel_account == CLIENT_SOLANA_CHANNEL).then_some(AdmittedSolanaVoucherChannel {
                authorized_signer: client_ed25519().public.to_bytes(),
                max_cumulative: CLIENT_COLLATERAL,
            }),
        )
    }
}

// ─── one chain, per test: the payout channel and the client's landing ───

/// The fake chain, this connector's paying half on it, and the terms the
/// client publishes for the channel it is paid on.
struct PayoutChain {
    chain: Arc<InMemoryBatchChain>,
    exit: PayerExit,
    terms: ReceiverTerms,
    /// The client's party on the fake chain: the first byte of its key.
    client_party: u8,
}

impl PayoutChain {
    fn evm() -> PayoutChain {
        let receiver = client_address();
        PayoutChain::on(
            PayerExit::Withdrawal,
            ReceiverTerms::Evm(EvmReceiverTerms {
                receiver,
                token: [TOKEN; 20],
                min_withdraw_delay_secs: ONE_DAY,
            }),
            receiver[0],
        )
    }

    fn solana() -> PayoutChain {
        let receiver = client_ed25519().public.to_bytes();
        PayoutChain::on(
            PayerExit::Close,
            ReceiverTerms::Solana(SolanaReceiverTerms {
                // The client's own sponsor: it is fee payer, `rent_payer`
                // and `payee` of the channel that pays it (ADR 0075
                // decision 3).
                sponsor: receiver,
                receiver,
                mint: [TOKEN; 32],
                min_grace_period_secs: ONE_DAY,
                min_deposit: 1,
                sponsor_endpoint: "https://earner.example/ilp/batch-settlement/solana/open"
                    .to_string(),
            }),
            receiver[0],
        )
    }

    fn on(exit: PayerExit, terms: ReceiverTerms, client_party: u8) -> PayoutChain {
        // The fake names a party by one byte; a key starting on a byte the
        // fake reserves, or on this connector's, would be someone else.
        assert!(
            ![CONNECTOR, 0xaa, 0xbb, 0xcc, 0xdd, 0x70, 0x71].contains(&client_party),
            "pick another client key: {client_party:#04x} is a party the fake already uses"
        );
        let chain = InMemoryBatchChain::new(exit);
        InMemoryBatchSettlement::on(Arc::clone(&chain), CONNECTOR, ONE_DAY).fund(100_000);
        PayoutChain {
            chain,
            exit,
            terms,
            client_party,
        }
    }

    fn settlement_chain(&self) -> SettlementChain {
        match self.exit {
            PayerExit::Withdrawal => SettlementChain::Evm,
            PayerExit::Close => SettlementChain::Solana,
        }
    }

    /// This node's outbound channels as a process booting over `journal`
    /// has them.
    async fn outbound(&self, journal: Arc<dyn Journal>) -> Arc<OutboundChannels> {
        let payer = InMemoryBatchSettlement::on(Arc::clone(&self.chain), CONNECTOR, ONE_DAY);
        Arc::new(
            OutboundChannels::restore(
                journal,
                vec![(
                    self.settlement_chain(),
                    Arc::new(payer) as Arc<dyn BatchSettlementPayer>,
                )],
            )
            .await
            .expect("the outbound journal replays"),
        )
    }

    /// The client's receiving half: how it lands a voucher itself.
    fn client(&self) -> InMemoryBatchSettlement {
        InMemoryBatchSettlement::on(Arc::clone(&self.chain), self.client_party, ONE_DAY)
    }
}

// ─── the served edge, and the client's socket ───

async fn serve(outbound: Arc<OutboundChannels>) -> SocketAddr {
    let clock = Arc::new(TestClock::new(
        Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap(),
    ));
    let signer: Arc<dyn Signer> = Arc::new(LocalSigner::generate("payout-edge"));
    // The probe below needs somewhere to go: a voucher riding a packet to a
    // destination nothing serves is never looked at (issue #1446).
    let connector = Arc::new(Connector::new(
        vec![
            connector_config::StaticRoute::new("g.toon.nowhere", "http://localhost:4000").unwrap(),
        ],
        vec![],
        Arc::new(FakeAppClient::new()),
        Arc::new(InProcessPeerTransport::new()),
        clock,
    ));
    let gate = ClientClaimGate::restore(Arc::new(connector_runtime::InMemoryJournal::new()))
        .expect("an empty journal")
        .with_batch_settlement(Arc::new(ClientChannels))
        .with_payout_ledger(Arc::new(ClientPayoutLedger::new(outbound)));
    let app = connector_client_edge::router_with_gate(connector, signer, None, gate);
    let server = axum::Server::bind(&"127.0.0.1:0".parse().unwrap()).serve(app.into_make_service());
    let addr = server.local_addr();
    tokio::spawn(server);
    addr
}

type Session =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

const BTP_RESPONSE: u8 = 1;
const BTP_MESSAGE: u8 = 6;
const BTP_TRANSFER: u8 = 7;

/// A MESSAGE in the deployed client's dialect (client-edge-spec.md §1.9).
fn btp_message(request_id: u32, protocol_data: &[(&str, &[u8])], ilp_packet: &[u8]) -> Vec<u8> {
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

fn btp_response(request_id: u32, ilp_packet: &[u8]) -> Vec<u8> {
    let mut out = vec![BTP_RESPONSE];
    out.extend_from_slice(&request_id.to_be_bytes());
    out.push(0);
    out.extend_from_slice(&(ilp_packet.len() as u32).to_be_bytes());
    out.extend_from_slice(ilp_packet);
    out
}

/// A frame the connector originated: `(type, requestId, amount,
/// protocolData, ilpPacket)` -- a TRANSFER carries an amount and no packet.
struct Frame {
    frame_type: u8,
    request_id: u32,
    amount: Option<u64>,
    protocol_data: Vec<(String, Vec<u8>)>,
    ilp_packet: Vec<u8>,
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
        let next = tokio::time::timeout(std::time::Duration::from_secs(10), session.next())
            .await
            .expect("the connector sends the next frame within 10s");
        match next.expect("open").expect("a frame") {
            WsMessage::Binary(bytes) => return parse(&bytes),
            _ => continue,
        }
    }
}

async fn send(session: &mut Session, frame: Vec<u8>) {
    session
        .send(WsMessage::Binary(frame))
        .await
        .expect("the socket takes the frame");
}

/// The client connects, binds its address, and pays this node `voucher` on
/// its own channel -- a standalone claim, which the client contract answers
/// with nothing (§1.9 step 5).
async fn client_session(addr: SocketAddr, voucher: &str) -> Session {
    let (mut session, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ilp/btp"))
        .await
        .expect("the upgrade succeeds");
    let auth = format!(r#"{{"peerId":"{ADDRESS}","secret":""}}"#);
    send(
        &mut session,
        btp_message(1, &[("auth", auth.as_bytes())], &[]),
    )
    .await;
    assert_eq!(next_frame(&mut session).await.frame_type, BTP_RESPONSE);
    // The voucher rides a zero-value packet to a route that answers nothing
    // useful, so its answer says
    // the voucher has been judged -- a standalone claim is answered with
    // nothing, and a buyer racing it could arrive before it is.
    let probe = Prepare {
        amount: 0,
        expires_at: Utc.with_ymd_and_hms(2031, 1, 1, 0, 0, 0).unwrap(),
        greeting: false,
        destination: "g.toon.nowhere".to_string(),
        data: Vec::new(),
    };
    send(
        &mut session,
        btp_message(
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

/// A buyer's PREPARE for `amount` to the client's address, over HTTP; the
/// client's session fulfils it. Returns the payout TRANSFER that follows on
/// the client's socket, acknowledged.
async fn earn(addr: SocketAddr, session: &mut Session, amount: u64, tag: u8) -> Frame {
    let prepare = Prepare {
        amount,
        expires_at: Utc.with_ymd_and_hms(2031, 1, 1, 0, 0, 0).unwrap(),
        greeting: false,
        destination: ADDRESS.to_string(),
        data: vec![tag],
    };
    let body = prepare.encode();
    let buyer = tokio::spawn(async move {
        let response = HttpClient::new()
            .request(
                HttpRequest::builder()
                    .method("POST")
                    .uri(format!("http://{addr}/ilp"))
                    .body(HttpBody::from(body))
                    .unwrap(),
            )
            .await
            .expect("the connector answers");
        assert_eq!(response.status(), StatusCode::OK);
        hyper::body::to_bytes(response.into_body()).await.unwrap()
    });

    let forwarded = next_frame(session).await;
    assert_eq!(forwarded.frame_type, BTP_MESSAGE);
    assert_eq!(
        Prepare::decode(&forwarded.ilp_packet).unwrap().amount,
        amount
    );
    let fulfillment = [tag; 32];
    send(
        session,
        btp_response(
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
    send(session, btp_response(transfer.request_id, &[])).await;

    let answered = Fulfill::decode(&buyer.await.unwrap()).expect("the buyer's packet fulfilled");
    assert_eq!(answered.fulfillment, fulfillment);
    transfer
}

/// The payout voucher a TRANSFER carries, read back into what the client
/// lands: the channel as the port presents it, and the signed voucher.
fn payout_of(transfer: &Frame) -> (serde_json::Value, ChannelPresentation, Voucher) {
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
    let (presentation, signature) = match json["blockchain"].as_str().unwrap() {
        "evm" => {
            let config = &json["channelConfig"];
            let address = |field: &str| -> [u8; 20] {
                hex::decode(config[field].as_str().unwrap().trim_start_matches("0x"))
                    .unwrap()
                    .try_into()
                    .unwrap()
            };
            let presentation = ChannelPresentation::Evm {
                channel,
                config: EvmChannelConfig {
                    payer: address("payer"),
                    payer_authorizer: address("payerAuthorizer"),
                    receiver: address("receiver"),
                    receiver_authorizer: address("receiverAuthorizer"),
                    token: address("token"),
                    withdraw_delay: config["withdrawDelay"].as_u64().unwrap(),
                    salt: hex::decode(config["salt"].as_str().unwrap().trim_start_matches("0x"))
                        .unwrap()
                        .try_into()
                        .unwrap(),
                },
            };
            let signature =
                hex::decode(json["signature"].as_str().unwrap().trim_start_matches("0x")).unwrap();
            (presentation, signature)
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
    (
        json,
        presentation,
        Voucher {
            cumulative_amount,
            signature,
        },
    )
}

/// The acceptance criteria end to end on one chain: a payout reaches the
/// client as a voucher it lands itself; nothing in it is a `toon-channel`
/// claim; and the payout watermark survives a restart.
async fn a_client_is_paid_a_voucher_it_lands_itself(payout: PayoutChain, voucher: String) {
    let dir = tempfile::tempdir().unwrap();
    let journal_path = dir.path().join("outbound-channels.log");
    let journal = || -> Arc<dyn Journal> { Arc::new(FileJournal::open(&journal_path).unwrap()) };

    // The operator's `POST /channels` toward the terms the client publishes.
    let outbound = payout.outbound(journal()).await;
    let (opened, _) = outbound
        .open(payout.terms.clone(), PAYOUT_DEPOSIT)
        .await
        .expect("the payout channel opens");
    let payout_channel = opened.on_chain.id.clone();

    let addr = serve(outbound).await;
    let mut session = client_session(addr, &voucher).await;
    let transfer = earn(addr, &mut session, 300, 1).await;
    let (json, presentation, first) = payout_of(&transfer);
    assert_eq!(transfer.amount, Some(300));
    assert_eq!(presentation.channel(), &payout_channel);
    assert_eq!(first.cumulative_amount, 300);
    assert!(
        json.get("nonce").is_none() && json.get("cumulativeAmount").is_none(),
        "nothing about a payout is a toon-channel claim: {json}"
    );

    let transfer = earn(addr, &mut session, 200, 2).await;
    let (_, _, second) = payout_of(&transfer);
    assert_eq!(second.cumulative_amount, 500, "vouchers are cumulative");

    // The client lands its latest payout on chain itself.
    let client = payout.client();
    client
        .admit(presentation.clone())
        .await
        .expect("the payout channel pays the client");
    let landed = client
        .land(presentation.channel(), second)
        .await
        .expect("the client lands its payout voucher");
    assert_eq!(landed.landed, 500);

    // A restart: the payout channel and its watermark come back from the
    // journal, and the next payout carries on above what was signed.
    drop(session);
    let restored = payout.outbound(journal()).await;
    let addr = serve(restored).await;
    let mut session = client_session(addr, &voucher).await;
    let transfer = earn(addr, &mut session, 100, 3).await;
    let (_, presentation, third) = payout_of(&transfer);
    assert_eq!(presentation.channel(), &payout_channel);
    assert_eq!(
        third.cumulative_amount, 600,
        "the payout watermark survived the restart"
    );
    let landed = client
        .land(presentation.channel(), third)
        .await
        .expect("the post-restart voucher lands above the last");
    assert_eq!(landed.landed, 600);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_evm_client_is_paid_a_voucher_it_lands_itself() {
    a_client_is_paid_a_voucher_it_lands_itself(PayoutChain::evm(), client_evm_voucher(100)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_solana_client_is_paid_a_voucher_it_lands_itself() {
    a_client_is_paid_a_voucher_it_lands_itself(PayoutChain::solana(), client_solana_voucher(100))
        .await;
}

/// A session that never proved a payee -- no voucher, no channel-control
/// proof -- is paid nothing, even with a channel open toward the key it
/// might have proved.
#[tokio::test(flavor = "multi_thread")]
async fn a_session_that_proved_no_payee_is_paid_nothing() {
    let payout = PayoutChain::evm();
    let outbound = payout
        .outbound(Arc::new(connector_runtime::InMemoryJournal::new()))
        .await;
    outbound
        .open(payout.terms.clone(), PAYOUT_DEPOSIT)
        .await
        .expect("the payout channel opens");
    let addr = serve(Arc::clone(&outbound)).await;

    let (mut session, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ilp/btp"))
        .await
        .unwrap();
    let auth = format!(r#"{{"peerId":"{ADDRESS}","secret":""}}"#);
    send(
        &mut session,
        btp_message(1, &[("auth", auth.as_bytes())], &[]),
    )
    .await;
    assert_eq!(next_frame(&mut session).await.frame_type, BTP_RESPONSE);

    let prepare = Prepare {
        amount: 300,
        expires_at: Utc.with_ymd_and_hms(2031, 1, 1, 0, 0, 0).unwrap(),
        greeting: false,
        destination: ADDRESS.to_string(),
        data: Vec::new(),
    };
    let body = prepare.encode();
    let buyer = tokio::spawn(async move {
        HttpClient::new()
            .request(
                HttpRequest::builder()
                    .method("POST")
                    .uri(format!("http://{addr}/ilp"))
                    .body(HttpBody::from(body))
                    .unwrap(),
            )
            .await
            .unwrap()
    });
    let forwarded = next_frame(&mut session).await;
    send(
        &mut session,
        btp_response(
            forwarded.request_id,
            &Fulfill {
                fulfillment: [9; 32],
                data: Vec::new(),
            }
            .encode(),
        ),
    )
    .await;
    assert_eq!(buyer.await.unwrap().status(), StatusCode::OK);

    let silent = tokio::time::timeout(std::time::Duration::from_millis(300), session.next()).await;
    assert!(silent.is_err(), "no payout TRANSFER follows: {silent:?}");
    let views = outbound.views().await;
    assert_eq!(views[0].watermark, 0, "nothing was signed");
}

// ─── issue #1396: a payee belongs to the session that proved it ───

/// Terms for a payout channel toward the EVM key `receiver`.
fn evm_terms_toward(receiver: [u8; 20]) -> ReceiverTerms {
    ReceiverTerms::Evm(EvmReceiverTerms {
        receiver,
        token: [TOKEN; 20],
        min_withdraw_delay_secs: ONE_DAY,
    })
}

/// What this node has signed on its outbound channel `channel` so far.
async fn signed_on(outbound: &OutboundChannels, channel: &ChannelId) -> u128 {
    outbound
        .views()
        .await
        .into_iter()
        .find(|view| view.id == channel.0)
        .map(|view| view.watermark)
        .expect("the payout channel is tracked")
}

/// An EVM payout chain with a channel open toward each of the two clients,
/// and the edge serving over it: `(edge, outbound, first client's payout
/// channel, second client's payout channel)`.
async fn two_payees() -> (SocketAddr, Arc<OutboundChannels>, ChannelId, ChannelId) {
    let payout = PayoutChain::evm();
    let other = address_of(&other_secret());
    assert_ne!(
        other[0], payout.client_party,
        "the fake chain tells parties apart by first byte"
    );
    let outbound = payout
        .outbound(Arc::new(connector_runtime::InMemoryJournal::new()))
        .await;
    let (victims, _) = outbound
        .open(payout.terms.clone(), PAYOUT_DEPOSIT)
        .await
        .expect("the payout channel toward the first client opens");
    let (others, _) = outbound
        .open(evm_terms_toward(other), PAYOUT_DEPOSIT)
        .await
        .expect("the payout channel toward the second client opens");
    let addr = serve(Arc::clone(&outbound)).await;
    (addr, outbound, victims.on_chain.id, others.on_chain.id)
}

/// A buyer's PREPARE for `amount` to [`ADDRESS`], over HTTP, handed to
/// `session`: the buyer's task, and the PREPARE as the session received it.
async fn job_for(
    addr: SocketAddr,
    session: &mut Session,
    amount: u64,
    tag: u8,
) -> (tokio::task::JoinHandle<StatusCode>, Frame) {
    let body = Prepare {
        amount,
        expires_at: Utc.with_ymd_and_hms(2031, 1, 1, 0, 0, 0).unwrap(),
        greeting: false,
        destination: ADDRESS.to_string(),
        data: vec![tag],
    }
    .encode();
    let buyer = tokio::spawn(async move {
        HttpClient::new()
            .request(
                HttpRequest::builder()
                    .method("POST")
                    .uri(format!("http://{addr}/ilp"))
                    .body(HttpBody::from(body))
                    .unwrap(),
            )
            .await
            .expect("the connector answers")
            .status()
    });
    let forwarded = next_frame(session).await;
    assert_eq!(forwarded.frame_type, BTP_MESSAGE);
    (buyer, forwarded)
}

fn fulfil(forwarded: &Frame, tag: u8) -> Vec<u8> {
    btp_response(
        forwarded.request_id,
        &Fulfill {
            fulfillment: [tag; 32],
            data: Vec::new(),
        }
        .encode(),
    )
}

/// A client reconnecting on a new socket and proving itself with
/// `voucher`: the connected session, and the payout TRANSFER the proof
/// earns it, acknowledged. The TRANSFER is a resend spawned when the
/// voucher teaches the session its payee, so it races the voucher's own
/// answer; either order is fine.
async fn reconnect_and_collect(addr: SocketAddr, voucher: &str) -> (Session, Frame) {
    let (mut session, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ilp/btp"))
        .await
        .expect("the upgrade succeeds");
    let auth = format!(r#"{{"peerId":"{ADDRESS}","secret":""}}"#);
    send(
        &mut session,
        btp_message(1, &[("auth", auth.as_bytes())], &[]),
    )
    .await;
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
        btp_message(
            2,
            &[("payment-channel-claim", voucher.as_bytes())],
            &probe.encode(),
        ),
    )
    .await;
    let (mut answered, mut transfer) = (false, None);
    while !answered || transfer.is_none() {
        let frame = next_frame(&mut session).await;
        match frame.frame_type {
            BTP_TRANSFER => {
                send(&mut session, btp_response(frame.request_id, &[])).await;
                transfer = Some(frame);
            }
            BTP_RESPONSE if frame.request_id == 2 => answered = true,
            other => panic!("unexpected frame type {other}"),
        }
    }
    (session, transfer.expect("the resend arrived"))
}

async fn assert_silent(session: &mut Session, why: &str) {
    let silent = tokio::time::timeout(std::time::Duration::from_millis(300), session.next()).await;
    assert!(silent.is_err(), "{why}: {silent:?}");
}

/// Issue #1396: a second session declaring the same `peerId` and proving a
/// key of its own changes nothing about where the first session's work is
/// paid. A job the first session was handed, and fulfils after the second
/// session bound, is paid toward the first session's key -- not redirected
/// to the second's, and not withheld: it is signed, and the first client,
/// reconnecting and proving its key again, is handed it.
#[tokio::test(flavor = "multi_thread")]
async fn another_session_at_the_same_address_neither_redirects_nor_withholds_a_payout() {
    let (addr, outbound, victims, others) = two_payees().await;

    let mut victim = client_session(addr, &client_evm_voucher(100)).await;
    let (buyer, forwarded) = job_for(addr, &mut victim, 300, 1).await;

    // A second session binds the same `peerId`, and proves its own channel.
    let mut other = client_session(addr, &evm_voucher_by(&other_secret(), 100)).await;

    // The first session finishes the job it was handed.
    send(&mut victim, fulfil(&forwarded, 1)).await;
    assert_eq!(buyer.await.unwrap(), StatusCode::OK);

    assert_eq!(
        signed_on(&outbound, &others).await,
        0,
        "nothing is signed toward the second session's key for the first's work"
    );
    assert_eq!(
        signed_on(&outbound, &victims).await,
        300,
        "the job is paid toward the key of the session that did it"
    );
    assert_silent(&mut other, "the second session is handed no payout").await;

    // The first client reconnects and proves its key again: the payout the
    // second session's bind left undelivered reaches it.
    drop(victim);
    let (_victim, transfer) = reconnect_and_collect(addr, &client_evm_voucher(200)).await;
    let (_, presentation, voucher) = payout_of(&transfer);
    assert_eq!(presentation.channel(), &victims);
    assert_eq!(voucher.cumulative_amount, 300);
    assert_eq!(signed_on(&outbound, &others).await, 0);
}

/// Issue #1396, the stranded-resend half: a payout left pending when its
/// session could not take it is resent to a later session only once that
/// session proves the key it is owed to. A session at the same address
/// that proves a different key is resent nothing; the client itself,
/// reconnecting and proving its key, is.
#[tokio::test(flavor = "multi_thread")]
async fn a_stranded_payout_is_resent_only_to_a_session_that_proves_its_key() {
    let (addr, outbound, victims, others) = two_payees().await;

    // The client earns, but refuses the payout TRANSFER, then drops: the
    // voucher stays pending for its key.
    let mut victim = client_session(addr, &client_evm_voucher(100)).await;
    let (buyer, forwarded) = job_for(addr, &mut victim, 300, 1).await;
    send(&mut victim, fulfil(&forwarded, 1)).await;
    let transfer = next_frame(&mut victim).await;
    assert_eq!(transfer.frame_type, BTP_TRANSFER);
    send(
        &mut victim,
        connector_btp::encode_error(transfer.request_id, "F00", "NotAcceptedError", b"not now"),
    )
    .await;
    assert_eq!(buyer.await.unwrap(), StatusCode::OK);
    assert_eq!(signed_on(&outbound, &victims).await, 300);
    drop(victim);

    // A session at the same address proving a different key.
    let mut other = client_session(addr, &evm_voucher_by(&other_secret(), 100)).await;
    assert_silent(
        &mut other,
        "a session proving another key is resent nothing",
    )
    .await;
    drop(other);

    // The client reconnects and proves its own key.
    let (_victim, transfer) = reconnect_and_collect(addr, &client_evm_voucher(200)).await;
    let (_, presentation, voucher) = payout_of(&transfer);
    assert_eq!(presentation.channel(), &victims);
    assert_eq!(voucher.cumulative_amount, 300);
    assert_eq!(signed_on(&outbound, &others).await, 0);
}
