//! Real-chain proof for issue #522 ("charge the price"): a claim's value
//! is checked against genuine, on-chain-funded value -- never a synthetic
//! literal invented by the test -- and pay-to-write is absolute, with no
//! configuration, flag or build profile that disables it.
//!
//! Per ADR 0007, the value-binding rule itself (`connector_domain::validate_price`)
//! is pure and already covered by property tests in `connector-domain` and
//! by HTTP-seam unit tests in this crate's own `src/lib.rs`; a chain is not
//! genuinely involved in that logic. What a chain-backed test proves that
//! those cannot is that the number this connector charges against is one a
//! real settlement backend actually moved on a real (if disposable) chain
//! -- an `anvil` instance, opened and funded via `EvmSettlementBackend`
//! (issue #459) -- rather than a number this test merely wrote down.

mod support;

use std::sync::Arc;

use axum::body::{Body, Bytes};
use axum::http::{Request, StatusCode};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use chrono::{Duration, TimeZone, Utc};
use tower::ServiceExt;

use libsecp256k1::{Message, PublicKey, SecretKey};

use connector_client_edge::{
    router_with_gate, ClientChannelRegistry, ClientClaimGate, DepositFloor, EvmChannel,
};
use connector_config::StaticRoute;
use connector_domain::{EnvelopeRequest, EnvelopeResponse, Fulfill, Prepare, Reject};
use connector_runtime::{
    AppOutcome, Connector, FakeAppClient, InProcessPeerTransport, TestClock, PAYER_HEADER,
};
use connector_settlement::SettlementBackend;
use connector_settlement_evm::EvmSettlementBackend;
use connector_signer::giftwrap::seal_request;
use connector_signer::{
    derive_evm_address, evm_balance_proof_digest, to_hex, EvmBalanceProof, LocalSigner, Signer,
};

use support::{require_anvil, Anvil, DEPLOYER_PRIVATE_KEY};

const HANDLER_URL: &str = "http://localhost:4000";
const CLAIM_HEADER: &str = "ilp-payment-channel-claim";
const CHAIN_ID: u64 = 8453;
const TOKEN_NETWORK_ADDRESS: [u8; 20] = [0x42; 20];

fn test_clock() -> Arc<TestClock> {
    Arc::new(TestClock::new(
        Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap(),
    ))
}

/// Per ADR 0018/issue #524, a `Prepare`'s `data` is a gift wrap sealed to
/// the terminating connector's identity key, opened above the `AppClient`
/// boundary (issue #521) -- what a genuine sender does before ever
/// transmitting a packet, the termination deriving its fulfilment from this
/// same sealed secret (ADR 0019, issue #525).
fn sealed_sample_prepare(receiver_public: &connector_signer::PublicKeyBytes) -> Prepare {
    let plaintext = EnvelopeRequest {
        method: "POST".to_string(),
        target: "/".to_string(),
        headers: vec![],
        body: b"hello app".to_vec(),
    }
    .encode();
    let (data, _shared_secret) = seal_request(&plaintext, receiver_public).expect("seal");
    Prepare {
        amount: 0,
        expires_at: Utc.with_ymd_and_hms(2031, 1, 1, 0, 0, 0).unwrap(),
        greeting: false,
        destination: "g.example.app".to_string(),
        data,
    }
}

/// Sign `digest` exactly the way a real EVM wallet would (a 65-byte
/// `r || s || v` signature, `v` in the conventional `{27, 28}` range).
fn sign_evm(secret: &SecretKey, digest: &[u8; 32]) -> Vec<u8> {
    let message = Message::parse(digest);
    let (signature, recovery_id) = libsecp256k1::sign(&message, secret);
    let mut bytes = signature.serialize().to_vec();
    let recovery_byte: u8 = recovery_id.into();
    bytes.push(recovery_byte + 27);
    bytes
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The channel counterparty every claim below is signed by: the same real
/// 20-byte EVM address the channels are genuinely opened to on chain
/// (issue #558 -- a claim signed by anyone else is refused, so a test that
/// signed with an unrelated key would be proving nothing about price).
fn counterparty_secret() -> SecretKey {
    SecretKey::parse(&[11u8; 32]).expect("valid secret key")
}

/// A **second** payer, for the second channel this file opens. Since ADR
/// 0059 (issue #1158) a channel id is derived from its participant pair, so
/// this node has at most one live channel with any one counterparty and a
/// second `openChannel` for the same pair reverts `ChannelAlreadyExists`.
/// Two concurrently live channels therefore means two payers -- which is
/// what a real deployment looks like anyway.
fn second_counterparty_secret() -> SecretKey {
    SecretKey::parse(&[12u8; 32]).expect("valid secret key")
}

fn address_of(secret: &SecretKey) -> [u8; 20] {
    derive_evm_address(&PublicKey::from_secret_key(secret).serialize())
}

fn counterparty_address() -> [u8; 20] {
    address_of(&counterparty_secret())
}

/// An EVM claim JSON with a genuine EIP-712 signature over its own fields
/// (issue #506/#544), produced by the channel's real on-chain counterparty
/// (issue #558) -- a forged or unsigned claim would be refused before ever
/// reaching the price check this file exists to prove.
fn evm_claim_json(channel_id_hex: &str, nonce: u64, transferred_amount: u128) -> String {
    evm_claim_json_signed_by(
        &counterparty_secret(),
        channel_id_hex,
        nonce,
        transferred_amount,
    )
}

/// [`evm_claim_json`] from a named payer -- the second channel's own
/// counterparty rather than the first's.
fn evm_claim_json_signed_by(
    secret: &SecretKey,
    channel_id_hex: &str,
    nonce: u64,
    transferred_amount: u128,
) -> String {
    evm_claim_json_under(
        secret,
        channel_id_hex,
        nonce,
        transferred_amount,
        CHAIN_ID,
        TOKEN_NETWORK_ADDRESS,
    )
}

/// [`evm_claim_json`] under a caller-chosen EIP-712 domain -- what a claim
/// on a channel **resolved from the chain** has to be signed under, since
/// the resolution reports the real deployment's own `chainId` and
/// `verifyingContract` rather than this file's synthetic pair.
fn evm_claim_json_under(
    secret: &SecretKey,
    channel_id_hex: &str,
    nonce: u64,
    transferred_amount: u128,
    chain_id: u64,
    token_network: [u8; 20],
) -> String {
    let address = address_of(secret);

    let mut channel_id = [0u8; 32];
    channel_id.copy_from_slice(
        &hex::decode(channel_id_hex.trim_start_matches("0x")).expect("valid hex channel id"),
    );
    let proof = EvmBalanceProof {
        channel_id,
        nonce,
        transferred_amount,
        locked_amount: 0,
        locks_root: [0u8; 32],
        chain_id,
        token_network_address: token_network,
    };
    let signature = sign_evm(secret, &evm_balance_proof_digest(&proof));

    format!(
        r#"{{
            "version": "1.0",
            "blockchain": "evm",
            "messageId": "msg-{nonce}",
            "timestamp": "2026-02-02T12:00:00.000Z",
            "senderId": "peer-bob",
            "channelId": "{channel_id_hex}",
            "nonce": {nonce},
            "transferredAmount": "{transferred_amount}",
            "lockedAmount": "0",
            "locksRoot": "0x{zeros}",
            "signature": "0x{signature}",
            "signerAddress": "{address}",
            "chainId": {chain_id},
            "tokenNetworkAddress": "{token_network_address}"
        }}"#,
        zeros = "0".repeat(64),
        signature = hex_encode(&signature),
        address = to_hex(&address),
        token_network_address = to_hex(&token_network),
    )
}

fn deliverable_connector(
    route: StaticRoute,
    app_client: Arc<FakeAppClient>,
    identity_signer: Arc<dyn Signer>,
) -> Arc<Connector> {
    app_client.respond(
        route.handler_url(),
        AppOutcome::Answered {
            response: EnvelopeResponse {
                status: 200,
                headers: vec![],
                body: b"ok".to_vec(),
            },
        },
    );
    Arc::new(
        Connector::new(
            vec![route],
            vec![],
            app_client,
            Arc::new(InProcessPeerTransport::new()),
            test_clock(),
        )
        .with_identity_signer(identity_signer),
    )
}

fn test_signer() -> Arc<dyn Signer> {
    Arc::new(LocalSigner::generate("test-signer"))
}

/// A registry recording `channel_id` as a channel of this connector's,
/// with the address the chain itself holds as its counterparty (issue
/// #558) -- read back from the settlement backend rather than assumed, so
/// the key a claim must be signed by is the one the chain would settle
/// against.
fn channels_recording(channel_id: &str, counterparty: &[u8]) -> ClientChannelRegistry {
    let mut address = [0u8; 20];
    address.copy_from_slice(counterparty);
    let mut channels = ClientChannelRegistry::new();
    channels
        .record_evm(
            channel_id,
            EvmChannel {
                counterparty: address,
                chain_id: CHAIN_ID,
                token_network_address: TOKEN_NETWORK_ADDRESS,
                deposit_floor: DepositFloor::Unknown,
            },
        )
        .expect("a real on-chain channel id is a 32-byte hex identifier");
    channels
}

async fn post_claim(
    connector: Arc<Connector>,
    signer: Arc<dyn Signer>,
    claim_json: &str,
    channels: ClientChannelRegistry,
) -> (StatusCode, Bytes) {
    // An in-memory journal: this test drives one process, and what a
    // watermark does across a *restart* is `claim_gate`'s own durability
    // module (issue #605).
    let gate = ClientClaimGate::restore(
        channels,
        Arc::new(connector_runtime::InMemoryJournal::new()),
    )
    .expect("a fresh in-memory journal has nothing to replay");
    post_claim_through(connector, signer, claim_json, gate).await
}

/// [`post_claim`], but against a router the caller keeps -- so two requests
/// can hit the *same* registry, which is the only way a memoised deposit
/// floor and its refresh (issue #646) are observable at all. A `Router` is
/// cheap to clone and `oneshot` consumes one, so the caller clones per
/// request rather than rebuilding the gate (which would resolve the channel
/// afresh and prove nothing about the memo).
async fn post_claim_through(
    connector: Arc<Connector>,
    signer: Arc<dyn Signer>,
    claim_json: &str,
    gate: ClientClaimGate,
) -> (StatusCode, Bytes) {
    let app = router_with_gate(connector, signer.clone(), None, gate);
    post_claim_to(app, signer, claim_json).await
}

async fn post_claim_to(
    app: axum::Router,
    signer: Arc<dyn Signer>,
    claim_json: &str,
) -> (StatusCode, Bytes) {
    let prepare = sealed_sample_prepare(&signer.public_key().unwrap());
    let request = Request::builder()
        .method("POST")
        .uri("/ilp")
        .header(CLAIM_HEADER, BASE64.encode(claim_json.as_bytes()))
        .body(Body::from(prepare.encode()))
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
    (status, bytes)
}

#[tokio::test]
async fn a_claim_backed_by_real_on_chain_funding_is_charged_the_routes_price() {
    if !require_anvil() {
        return;
    }

    let anvil = Anvil::spawn().await;
    let token =
        EvmSettlementBackend::deploy_mock_token(&anvil.rpc_url, DEPLOYER_PRIVATE_KEY, 1_000_000)
            .await
            .expect("deploy mock USDC");
    let backend = EvmSettlementBackend::deploy(&anvil.rpc_url, DEPLOYER_PRIVATE_KEY, token)
        .await
        .expect("deploy a TokenNetwork through a fresh registry");

    // A channel genuinely opened and funded with real (anvil-minted mock
    // USDC) value -- `counterparty_deposited` below is read back from the
    // chain's own receipt, never a number this test invents. The
    // counterparty must be a real 20-byte EVM address (issue #576):
    // `TokenNetwork` requires one able to sign balance proofs, not an
    // arbitrary peer name.
    //
    // It is the *counterparty's* side that is funded here, and only that
    // side: a claim this node redeems is drawn from the payer's
    // collateral, never the node's own. `fund_counterparty` is the
    // fixture-only delegate deposit standing in for the payer's own
    // wallet (issue #1118) -- `fund` itself would credit the node's side
    // and buy nothing.
    let counterparty = counterparty_address().to_vec();
    let paid_channel = backend
        .open(counterparty.clone(), Duration::hours(1))
        .await
        .expect("open a real channel");
    let paid_state = backend
        .fund_counterparty(&paid_channel, 1_000)
        .await
        .expect("fund the channel with real ERC-20 value");
    assert_eq!(
        paid_state.counterparty_deposited, 1_000,
        "a real transaction genuinely moved this value on chain"
    );

    // A second channel, to a SECOND payer: one live channel per pair since
    // ADR 0059, so reusing `counterparty` here would revert on chain.
    let underpaid_counterparty = address_of(&second_counterparty_secret()).to_vec();
    let underpaid_channel = backend
        .open(underpaid_counterparty, Duration::hours(1))
        .await
        .expect("open a second real channel, to a second payer");
    let underpaid_state = backend
        .fund_counterparty(&underpaid_channel, 40)
        .await
        .expect("fund the second channel with real ERC-20 value, less than the route's price");
    assert_eq!(underpaid_state.counterparty_deposited, 40);

    let route = StaticRoute::new_priced("g.example.app", HANDLER_URL, 100).unwrap();

    // A claim advancing by the full, genuinely-deposited 1_000 covers this
    // route's price of 100 and the packet is delivered (AC1).
    let app_client = Arc::new(FakeAppClient::new());
    let signer = test_signer();
    let connector = deliverable_connector(route.clone(), app_client.clone(), signer.clone());
    let (status, bytes) = post_claim(
        connector,
        signer,
        &evm_claim_json(&paid_channel.0, 1, paid_state.counterparty_deposited),
        channels_recording(&paid_channel.0, &paid_state.counterparty),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    Fulfill::decode(&bytes).expect("a claim covering the real on-chain deposit is fulfilled");
    assert_eq!(app_client.deliveries().len(), 1);

    // ADR 0040 (issue #994), joined to a real chain: the payer the app is
    // told about is the channel this claim was actually *verified*
    // against, not anything the sender wrote. Asserted here rather than
    // only in `connector-runtime`'s own unit tests because this is the
    // whole path -- a real signature, checked against a real on-chain
    // channel, whose key then rides the delivery.
    let delivery = app_client.deliveries().remove(0);
    let payer = delivery
        .request
        .headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(PAYER_HEADER))
        .map(|(_, value)| value.clone());
    assert_eq!(
        payer.as_deref(),
        Some(
            format!(
                "evm:0x{}",
                paid_channel.0.trim_start_matches("0x").to_ascii_lowercase()
            )
            .as_str()
        ),
        "the app is told which channel paid for this delivery"
    );

    // A second, freshly funded channel that genuinely received only 40 --
    // less than the route's price of 100 -- is refused as underpayment
    // (F03), never reaching the app, with no flag or config to bypass it
    // (AC2, AC4).
    let app_client_two = Arc::new(FakeAppClient::new());
    let signer_two = test_signer();
    let connector_two = deliverable_connector(route, app_client_two.clone(), signer_two.clone());
    let (status_two, bytes_two) = post_claim(
        connector_two,
        signer_two,
        &evm_claim_json_signed_by(
            &second_counterparty_secret(),
            &underpaid_channel.0,
            1,
            underpaid_state.counterparty_deposited,
        ),
        channels_recording(&underpaid_channel.0, &underpaid_state.counterparty),
    )
    .await;
    assert_eq!(status_two, StatusCode::OK);
    let reject = Reject::decode(&bytes_two).expect("decode reject");
    assert_eq!(reject.code.as_str(), "F03");
    assert!(app_client_two.deliveries().is_empty());
}

/// Issue #646, against a real chain: a claim naming more than its channel's
/// counterparty has genuinely deposited buys nothing, and the very same
/// claim becomes good once a real `setTotalDeposit` covers it.
///
/// The claim above already built its amount to *equal* the deposited value,
/// honouring an invariant nothing enforced. This is that invariant enforced:
/// `TokenNetwork.claimFromChannel` reverts `InsufficientChannelBalance`
/// (`TokenNetwork.sol:316-318`) for exactly this claim, so accepting it
/// would be work the operator could provably never redeem.
///
/// The channel is resolved **from the chain**, not declared: a declared
/// `[[client_channels]]` record carries no deposit and is deliberately
/// exempt, so the registry here is the anonymous-buyer path (#502) where the
/// cap actually applies.
#[tokio::test]
async fn a_claim_above_the_real_on_chain_deposit_is_refused_until_a_real_deposit_covers_it() {
    if !require_anvil() {
        return;
    }

    let anvil = Anvil::spawn().await;
    let token =
        EvmSettlementBackend::deploy_mock_token(&anvil.rpc_url, DEPLOYER_PRIVATE_KEY, 1_000_000)
            .await
            .expect("deploy mock USDC");
    let backend = Arc::new(
        EvmSettlementBackend::deploy(&anvil.rpc_url, DEPLOYER_PRIVATE_KEY, token)
            .await
            .expect("deploy a TokenNetwork through a fresh registry"),
    );

    let channel = backend
        .open(counterparty_address().to_vec(), Duration::hours(1))
        .await
        .expect("open a real channel");
    let state = backend
        .fund_counterparty(&channel, 1_000)
        .await
        .expect("fund the channel with real ERC-20 value");
    assert_eq!(state.counterparty_deposited, 1_000);

    // The claim is signed under the chain's *own* EIP-712 domain, since
    // that is what the resolution reports -- a synthetic domain would fail
    // the signature check long before the collateral one.
    let chain_id = backend.chain_id();
    let token_network = backend.address().to_fixed_bytes();
    let route = StaticRoute::new_priced("g.example.app", HANDLER_URL, 100).unwrap();
    let app_client = Arc::new(FakeAppClient::new());
    let signer = test_signer();
    let connector = deliverable_connector(route, app_client.clone(), signer.clone());
    let gate = ClientClaimGate::restore(
        ClientChannelRegistry::new().with_source(Arc::new(BackendChannelSource {
            backend: backend.clone(),
        })),
        Arc::new(connector_runtime::InMemoryJournal::new()),
    )
    .expect("a fresh in-memory journal has nothing to replay");
    let app = router_with_gate(connector, signer.clone(), None, gate);

    // One base unit above what the chain holds: fresh, well-formed,
    // correctly signed, and covering the route's price of 100 -- refused
    // purely because it could never be redeemed.
    let over = evm_claim_json_under(
        &counterparty_secret(),
        &channel.0,
        1,
        1_001,
        chain_id,
        token_network,
    );
    let (status, bytes) = post_claim_to(app.clone(), signer.clone(), &over).await;
    assert_eq!(status, StatusCode::OK);
    let reject = Reject::decode(&bytes).expect("decode reject");
    assert_eq!(reject.code.as_str(), "F03");
    assert!(
        reject.message.contains("deposited on chain"),
        "the refusal must say what it is: {}",
        reject.message
    );
    assert_eq!(
        reject.accumulated_cost, 0,
        "the route's price was covered -- this refusal is not about a price"
    );
    assert!(
        app_client.deliveries().is_empty(),
        "an unredeemable claim buys no work from the app"
    );

    // A real on-chain top-up of one base unit. The memoised floor is now
    // stale *low*, which is the only direction it can be stale in -- and
    // the identical claim, at the identical nonce, is honoured on
    // resubmission because the breach provokes a re-read rather than a
    // permanent refusal.
    let topped_up = backend
        .fund_counterparty(&channel, 1)
        .await
        .expect("a real second setTotalDeposit");
    assert_eq!(topped_up.counterparty_deposited, 1_001);

    // Under the policy a production node actually runs, and deliberately:
    // the re-read that notices the deposit is rate-limited per channel, so
    // a refusal that consumes no nonce cannot be re-presented as an
    // unlimited free chain read. Retrying is the point -- it is what shows
    // the interval is a delay of seconds rather than a wall, without this
    // test reaching for a policy no operator would run.
    let mut fulfilled = false;
    for _ in 0..40 {
        let (status, bytes) = post_claim_to(app.clone(), signer.clone(), &over).await;
        assert_eq!(status, StatusCode::OK);
        if Fulfill::decode(&bytes).is_ok() {
            fulfilled = true;
            break;
        }
        assert_eq!(
            Reject::decode(&bytes).expect("decode reject").code.as_str(),
            "F03",
            "still the same refusal, for the same reason, until the re-read happens"
        );
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    assert!(
        fulfilled,
        "the identical claim redeems now, so it is accepted now -- no restart needed"
    );
    assert_eq!(app_client.deliveries().len(), 1);
}

/// The client edge's chain-resolution port over a real
/// [`EvmSettlementBackend`] -- `connector-cli`'s own `SettlementChannelSource`,
/// restated here because this crate cannot depend on `connector-cli` (it is
/// the other way round). The two make the same two reads.
struct BackendChannelSource {
    backend: Arc<EvmSettlementBackend>,
}

/// Hand-written for the same reason `connector-cli`'s is: an
/// [`EvmSettlementBackend`] holds contract handles and a signing client
/// that are not `Debug`.
impl std::fmt::Debug for BackendChannelSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BackendChannelSource")
            .field("token_network", &self.backend.address())
            .finish()
    }
}

#[async_trait::async_trait]
impl connector_client_edge::ClientChannelSource for BackendChannelSource {
    async fn evm_channel(
        &self,
        channel_id: &[u8; 32],
    ) -> Result<Option<EvmChannel>, connector_client_edge::ChannelLookupFailed> {
        let resolved = self
            .backend
            .channel_counterparty_deposit(*channel_id)
            .await
            .map_err(|error| connector_client_edge::ChannelLookupFailed(error.to_string()))?;
        Ok(resolved.map(|(counterparty, deposit)| EvmChannel {
            counterparty: counterparty.to_fixed_bytes(),
            chain_id: self.backend.chain_id(),
            token_network_address: self.backend.address().to_fixed_bytes(),
            deposit_floor: DepositFloor::AtLeast(deposit.as_u64()),
        }))
    }
}
