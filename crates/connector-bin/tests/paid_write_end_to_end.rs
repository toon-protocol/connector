//! Issue #528: a paid write, proven end to end, against a real chain.
//!
//! Nothing before this file joined "money genuinely moved" to "the write
//! genuinely landed." Settlement was exercised against a real chain in
//! isolation (`connector-settlement-evm`'s own suite); the packet path was
//! exercised with fakes (`connector-client-edge`'s `price_charging_real_chain.rs`
//! uses an in-process `tower::Service` and a `FakeAppClient`); the one test
//! that drove the real binary (`two_connectors_and_a_stub_app.rs`, since
//! deleted with the raw-TCP transport it proved -- ADR 0027, issue #679)
//! used a zero-priced route and asserted delivery, never a claim.
//!
//! This test spawns a real `anvil` chain, deploys a real `TokenNetworkRegistry`/
//! `TokenNetwork`/mock ERC-20 onto it, mints and deposits real value into a
//! real channel, spawns a real compiled `connector` binary fronting a real
//! HTTP app (bound to its own socket, recording what it receives), and sends
//! a real sealed, claim-bearing packet over a real TCP connection to the
//! connector's client edge. It asserts the app's recorded write and the
//! claim's accepted value together, and that an underpaid claim reaches
//! neither.

use std::io::Write;
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::Router;
use chrono::Duration as ChronoDuration;
use ed25519_dalek::Keypair;
use libsecp256k1::{Message, PublicKey, SecretKey};
use rand::rngs::OsRng;

use connector_domain::{EnvelopeResponse, Fulfill, Prepare, Reject};
use connector_operator::test_support::sign_request;
use connector_runtime::ChannelView;
use connector_settlement::SettlementBackend;
use connector_settlement_evm::test_support::{require_anvil, Anvil, DEPLOYER_PRIVATE_KEY};
use connector_settlement_evm::EvmSettlementBackend;
use connector_signer::giftwrap::open_response;
use connector_signer::{derive_evm_address, evm_balance_proof_digest, to_hex, EvmBalanceProof};

mod support;
use support::{
    identity_from_key_seed, sample_prepare, sealed_prepare_data, spawn_connector, write_config,
    write_raw_key_file,
};

/// `anvil`'s own default chain id (`Anvil::spawn`'s `--chain-id 31337`), and
/// so the EIP-712 domain a claim against its deployed `TokenNetwork` must be
/// signed under -- matching `connector-cli/tests/settlement_lifecycle.rs`'s
/// own constant of the same name and value.
const ANVIL_CHAIN_ID: u64 = 31_337;

const CLAIM_HEADER: &str = "ilp-payment-channel-claim";

/// This test binary's own base port for [`Anvil::spawn`] -- distinct from
/// every other test binary's base in this workspace (`connector-bin`'s own
/// `devnet_configs_load.rs` uses `18_500`; `connector-settlement-evm`'s
/// tests use `18_600`; `connector-cli`'s use `18_700`/`18_800`;
/// `connector-client-edge`'s `price_charging_real_chain.rs` uses `18_900`)
/// so concurrent binaries under `cargo test --workspace` don't contend for
/// the same port range.
const ANVIL_BASE_PORT: u16 = 19_000;

/// The route's price, in the same integer units the settlement backend
/// deposits/redeems in -- deliberately small so the "advances by exactly
/// the price" claim below is easy to read.
const ROUTE_PRICE: u128 = 100;

/// The Solana twin of [`ROUTE_PRICE`] -- `u64` because
/// `packages/solana-program`'s own wire (`SolanaClientClaim::transferred_amount`,
/// `wire::pack_deposit`) moves value in `u64` SPL-token base units, never
/// `u128`.
const SOLANA_ROUTE_PRICE: u64 = 100;

/// Sign `digest` exactly the way the production signing path does
/// (`connector_signer::crypto::sign_digest`): a 65-byte `r || s || v`
/// signature with `v` in libsecp256k1's raw `{0, 1}` range, not the
/// `{27, 28}` an EVM wallet would append. `EvmSettlementBackend` itself
/// normalizes that before submitting to `claimFromChannel` (issue #590),
/// and the client edge's own verification accepts either range -- signing
/// the wallet range here would exercise neither of those paths for real
/// (issue #594).
fn sign_evm(secret: &SecretKey, digest: &[u8; 32]) -> Vec<u8> {
    let message = Message::parse(digest);
    let (signature, recovery_id) = libsecp256k1::sign(&message, secret);
    let mut bytes = signature.serialize().to_vec();
    let recovery_byte: u8 = recovery_id.into();
    bytes.push(recovery_byte);
    bytes
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn channel_id_bytes(id: &str) -> [u8; 32] {
    let hex_digits = id.trim_start_matches("0x");
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex_digits[i * 2..i * 2 + 2], 16)
            .expect("channel id is 0x-prefixed 64-hex");
    }
    out
}

/// A genuinely EIP-712-signed EVM claim (issue #506/#544, #575) over the
/// real deployed `TokenNetwork`'s own chain id and address -- not arbitrary
/// constants -- so a reader can tell this claim actually refers to the
/// chain this test just funded a channel on. Returns the claim JSON
/// alongside the raw signature bytes, so a caller can also redeem the same
/// claim against the chain (issue #594) without re-deriving it.
fn evm_claim_json(
    secret: &SecretKey,
    channel_id_hex: &str,
    nonce: u64,
    transferred_amount: u128,
    token_network_address: [u8; 20],
) -> (String, Vec<u8>) {
    let public = PublicKey::from_secret_key(secret);
    let address = derive_evm_address(&public.serialize());

    let proof = EvmBalanceProof {
        channel_id: channel_id_bytes(channel_id_hex),
        nonce,
        transferred_amount,
        locked_amount: 0,
        locks_root: [0u8; 32],
        chain_id: ANVIL_CHAIN_ID,
        token_network_address,
    };
    let signature = sign_evm(secret, &evm_balance_proof_digest(&proof));

    let json = format!(
        r#"{{
            "version": "1.0",
            "blockchain": "evm",
            "messageId": "msg-{nonce}",
            "timestamp": "2026-02-02T12:00:00.000Z",
            "senderId": "buyer",
            "channelId": "{channel_id_hex}",
            "nonce": {nonce},
            "transferredAmount": "{transferred_amount}",
            "lockedAmount": "0",
            "locksRoot": "0x{zeros}",
            "signature": "0x{signature}",
            "signerAddress": "{address}",
            "chainId": {ANVIL_CHAIN_ID},
            "tokenNetworkAddress": "{token_network_address}"
        }}"#,
        zeros = "0".repeat(64),
        signature = hex_encode(&signature),
        address = to_hex(&address),
        token_network_address = to_hex(&token_network_address),
    );
    (json, signature)
}

/// A Solana claim's JSON wire shape (`connector_domain::client_claim::SolanaClientClaim`),
/// the Solana twin of [`evm_claim_json`] -- `signature_base64` and
/// `signer_public_key_base58` are supplied rather than derived here, since
/// a genuine claim and a forged one differ only in which key signs, not in
/// how the JSON is shaped.
///
/// `program_id_base58` is the settlement program the channel lives under
/// (`client-edge-spec.md` §1.3), which is the same program id the balance
/// proof the caller signed binds at offset 16 (ADR 0053). It is a parameter
/// rather than a literal because this fixture used to write the payer's own
/// public key there -- a value no channel could ever live under -- and a
/// conforming payer is what these tests are supposed to be modelling
/// (issue #1127).
fn solana_claim_json(
    channel_account_base58: &str,
    program_id_base58: &str,
    nonce: u64,
    transferred_amount: u64,
    signature_base64: &str,
    signer_public_key_base58: &str,
) -> String {
    format!(
        r#"{{
            "version": "1.0",
            "blockchain": "solana",
            "messageId": "msg-{nonce}",
            "timestamp": "2026-02-02T12:00:00.000Z",
            "senderId": "buyer",
            "programId": "{program_id_base58}",
            "channelAccount": "{channel_account_base58}",
            "nonce": {nonce},
            "transferredAmount": "{transferred_amount}",
            "signature": "{signature_base64}",
            "signerPublicKey": "{signer_public_key_base58}"
        }}"#
    )
}

/// One request the recording app below actually received, over its own
/// socket: the body it was sent, and the headers that came with it. The
/// headers are part of the record because ADR 0040's attribution
/// (`X-TOON-Payer`/`-Amount`/`-Chain`) is only real if it survives the whole
/// path -- a real connector binary, a real HTTP hop -- and a test reading
/// them off an in-process fake would not prove that.
#[derive(Clone)]
struct RecordedWrite {
    headers: axum::http::HeaderMap,
    body: Bytes,
}

/// What `header` this write carried, as a string, or `None` if it carried
/// none -- the two outcomes ADR 0040 distinguishes.
fn recorded_header<'a>(write: &'a RecordedWrite, header: &str) -> Option<&'a str> {
    write
        .headers
        .get(header)
        .map(|value| value.to_str().expect("header is valid ASCII"))
}

/// A real, independently queryable app: a genuine `axum` HTTP server bound
/// to its own OS-assigned socket, reachable only over that socket (never
/// invoked in-process) -- so "the app recorded the write" and "the app
/// recorded nothing" are facts about a second real process boundary, not
/// trust in the packet's own echoed response.
async fn spawn_recording_app() -> (String, Arc<Mutex<Vec<RecordedWrite>>>) {
    #[derive(Clone)]
    struct RecordingState {
        recorded: Arc<Mutex<Vec<RecordedWrite>>>,
    }

    async fn record(
        State(state): State<RecordingState>,
        headers: axum::http::HeaderMap,
        body: Bytes,
    ) -> StatusCode {
        state
            .recorded
            .lock()
            .expect("recorded-writes lock poisoned")
            .push(RecordedWrite { headers, body });
        StatusCode::OK
    }

    let recorded = Arc::new(Mutex::new(Vec::new()));
    let state = RecordingState {
        recorded: recorded.clone(),
    };
    let router = Router::new().route("/", post(record)).with_state(state);

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind recording app");
    let addr = listener.local_addr().expect("recording app addr");
    tokio::spawn(async move {
        axum::Server::from_tcp(listener)
            .expect("axum server from tcp listener")
            .serve(router.into_make_service())
            .await
            .expect("recording app server");
    });

    (addr.to_string(), recorded)
}

/// POST a sealed `Prepare`, base64-carrying `claim` in the claim header, to
/// a real connector's client edge, and return the raw response body -- the
/// paid and underpaid cases below differ only in what they decode that body
/// as (`Fulfill` vs `Reject`).
async fn post_ilp_packet(
    client: &reqwest::Client,
    client_edge_addr: &str,
    claim: &str,
    prepare: &Prepare,
    expect_msg: &str,
) -> Bytes {
    let response = client
        .post(format!("http://{client_edge_addr}/ilp"))
        .header(CLAIM_HEADER, base64_encode(claim.as_bytes()))
        .body(prepare.encode())
        .send()
        .await
        .expect(expect_msg);
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    response.bytes().await.expect("response body")
}

#[tokio::test]
async fn a_paid_write_lands_on_the_app_with_the_claim_advanced_by_the_routes_price() {
    // AC5: fail loudly in CI when the chain this test needs is unavailable,
    // never silently skip and report success (issue #471's own policy).
    if !require_anvil() {
        return;
    }

    // AC1/AC6: a real local chain, a real registry-resolved `TokenNetwork`,
    // and tokens genuinely minted for this test -- never assumed pre-funded.
    let anvil = Anvil::spawn(ANVIL_BASE_PORT).await;
    let token =
        EvmSettlementBackend::deploy_mock_token(&anvil.rpc_url, DEPLOYER_PRIVATE_KEY, 1_000_000)
            .await
            .expect("mint a fresh mock ERC-20 for this test");
    let backend = EvmSettlementBackend::deploy(&anvil.rpc_url, DEPLOYER_PRIVATE_KEY, token)
        .await
        .expect("deploy a TokenNetwork through a fresh registry");
    let token_network_address = backend.address().to_fixed_bytes();

    // The buyer: a real secp256k1 identity able to sign balance proofs
    // `TokenNetwork.claimFromChannel` would recover on chain -- not a mocked
    // signer, and not a placeholder identifier hashed into address shape
    // (issue #576 refuses exactly that).
    let buyer_secret = SecretKey::parse(&[21u8; 32]).expect("valid secret key");
    let buyer_public = PublicKey::from_secret_key(&buyer_secret);
    let buyer_address = derive_evm_address(&buyer_public.serialize());

    // A funded channel with real, on-chain-deposited value -- read back from
    // the chain's own receipt, never invented by this test. The value goes
    // on the *buyer's* side, because the buyer is who signs the claims this
    // node redeems; on a real deployment the buyer deposits it from their
    // own wallet (as the Solana half of this file has them do below), and
    // here the fixture-only delegate deposit stands in (issue #1118).
    let paid_channel = backend
        .open(buyer_address.to_vec(), ChronoDuration::hours(1))
        .await
        .expect("open a real channel");
    let paid_state = backend
        .fund_counterparty(&paid_channel, 10 * ROUTE_PRICE)
        .await
        .expect("fund the channel with real ERC-20 value");
    assert_eq!(
        paid_state.counterparty_deposited,
        10 * ROUTE_PRICE,
        "a real transaction genuinely moved this value on chain"
    );

    // A second, separately funded channel that received real value below
    // the route's price -- proving the underpayment case is refused against
    // genuine on-chain funding too, not a synthetic shortfall.
    //
    // A SECOND BUYER IDENTITY, not the one above (ADR 0059, issue #1158): a
    // channel id is now derived from its participant pair, so this node and
    // one buyer have exactly one live channel between them and a second
    // `openChannel` for the same pair reverts `ChannelAlreadyExists`. Two
    // concurrently live channels means two payers, which is what a real
    // deployment looks like anyway.
    let underpaid_buyer_secret = SecretKey::parse(&[22u8; 32]).expect("valid secret key");
    let underpaid_buyer_address =
        derive_evm_address(&PublicKey::from_secret_key(&underpaid_buyer_secret).serialize());
    let underpaid_channel = backend
        .open(underpaid_buyer_address.to_vec(), ChronoDuration::hours(1))
        .await
        .expect("open a second real channel, with a second buyer");
    let underpaid_state = backend
        .fund_counterparty(&underpaid_channel, ROUTE_PRICE / 2)
        .await
        .expect("fund the second channel with real ERC-20 value, less than the route's price");
    assert_eq!(underpaid_state.counterparty_deposited, ROUTE_PRICE / 2);

    // A real app: its own socket, its own process-independent record of
    // what it received.
    let (app_addr, recorded) = spawn_recording_app().await;

    // A real, spawned, compiled `connector` binary -- started from a config
    // file, exactly as a deployment would be, fronting the app above at a
    // priced route. AC1: a real `[settlement]` section, connected to the
    // same registry/`TokenNetwork` this test just deployed and funded --
    // the connector's own value-moving side of the chain, not merely a
    // chain sitting next to it -- plus an `[operator]` section so the test
    // can redeem the accepted claim through the same surface a real
    // operator would use, never by reaching around the connector into the
    // settlement backend directly.
    let key_file = write_raw_key_file(9);
    let mut settlement_key_file = tempfile::NamedTempFile::new().expect("temp settlement key file");
    settlement_key_file
        .write_all(DEPLOYER_PRIVATE_KEY.as_bytes())
        .expect("write settlement key file");
    let write_keypair = Keypair::generate(&mut OsRng);
    let write_key_hex = hex_encode(&write_keypair.public.to_bytes());
    let registry_address = backend.registry_address();
    // Issue #605: a node with `[[client_channels]]` must name a durable
    // `state_dir` -- config load refuses one that does not, because its
    // claim watermarks would live only in memory and every spent claim
    // would be replayable after a restart.
    let state_dir_handle = tempfile::tempdir().expect("temp state dir");
    let state_dir = state_dir_handle.path().display();
    let config = write_config(&format!(
        r#"
client_edge_addr = "127.0.0.1:0"
state_dir = "{state_dir}"

[signer]
key_file = "{key_file}"

[operator]
bearer_token = "operator-secret"
write_keys = ["{write_key_hex}"]

[settlement]
chain = "evm"
rpc_url = "{rpc_url}"
contract_address = "{registry_address:?}"
token_address = "{token:?}"
decimals = 6

[settlement.key]
key_file = "{settlement_key_file}"

[[routes]]
prefix = "g.example.app"
handler_url = "http://{app_addr}"
price = {ROUTE_PRICE}

# Issue #558: whose signature this node accepts on each of the two channels
# opened above -- each channel's own buyer address, the one the chain itself
# holds as this node's counterparty there. Without this the connector has a
# record of no channel and refuses every claim, rather than believing what a
# claim says about its own signer. Two channels, two buyers: one live channel
# per pair (ADR 0059).
[[client_channels]]
channel_id = "{paid_channel_id}"
counterparty = "{buyer}"
chain_id = {ANVIL_CHAIN_ID}
token_network_address = "{token_network}"

[[client_channels]]
channel_id = "{underpaid_channel_id}"
counterparty = "{underpaid_buyer}"
chain_id = {ANVIL_CHAIN_ID}
token_network_address = "{token_network}"
"#,
        key_file = key_file.path().display(),
        state_dir = state_dir,
        settlement_key_file = settlement_key_file.path().display(),
        rpc_url = anvil.rpc_url,
        paid_channel_id = paid_channel.0,
        underpaid_channel_id = underpaid_channel.0,
        buyer = to_hex(&buyer_address),
        underpaid_buyer = to_hex(&underpaid_buyer_address),
        token_network = to_hex(&token_network_address),
    ));
    let connector = spawn_connector(config.path());
    let connector_identity = identity_from_key_seed(9);

    let client = reqwest::Client::new();

    // AC2/AC3/AC6: a sealed, paid packet -- signed for exactly the route's
    // price, so a fulfilled response proves the claim's cumulative amount
    // advanced by exactly the price, never merely "some amount >= price."
    let (data, shared_secret) = sealed_prepare_data(b"paid write", &connector_identity);
    let prepare = sample_prepare("g.example.app", data);
    let (claim, claim_signature) = evm_claim_json(
        &buyer_secret,
        &paid_channel.0,
        1,
        ROUTE_PRICE,
        token_network_address,
    );
    let body = post_ilp_packet(
        &client,
        &connector.client_edge_addr,
        &claim,
        &prepare,
        "POST /ilp: paid write",
    )
    .await;
    let fulfill =
        Fulfill::decode(&body).expect("a claim covering exactly the route's price fulfils");
    let opened = open_response(&shared_secret, &fulfill.data).expect("open sealed response");
    let response_envelope = EnvelopeResponse::decode(&opened).expect("decode envelope");
    assert_eq!(response_envelope.status, 200);

    // Both halves, asserted together (AC3): the app genuinely recorded the
    // write, over its own socket, independent of the packet's own echo.
    let after_paid = recorded.lock().expect("recorded lock").clone();
    assert_eq!(
        after_paid.len(),
        1,
        "the app recorded exactly one write for the fulfilled, correctly-priced packet"
    );
    assert_eq!(after_paid[0].body.as_ref(), b"paid write");

    // ADR 0040 (issue #994), over the whole real path: the app is told
    // which channel paid for this write, how much, and on what chain --
    // and the payer is the channel the claim above was actually verified
    // against, not anything the sender wrote.
    assert_eq!(
        recorded_header(&after_paid[0], "x-toon-payer"),
        Some(format!("evm:{}", paid_channel.0.to_ascii_lowercase()).as_str()),
        "the app is told which channel paid for this write"
    );
    assert_eq!(
        recorded_header(&after_paid[0], "x-toon-amount"),
        Some(ROUTE_PRICE.to_string().as_str())
    );
    assert_eq!(recorded_header(&after_paid[0], "x-toon-chain"), Some("evm"));

    // AC2/AC3: redeem that exact claim against the real chain, through the
    // connector's own operator surface -- not inferred from the fulfil, and
    // not a second, hand-built settlement backend reaching around the
    // connector under test. A signature `TokenNetwork.claimFromChannel`
    // would refuse to recover (e.g. against the wrong deposit) fails this
    // redeem for real; joining it to the same test as the fulfilled write
    // is what proves the accept decision and the on-chain value are the
    // same claim, not two coincidentally-matching half-proofs.
    let redeem_body = serde_json::to_vec(&serde_json::json!({
        "nonce": 1,
        "cumulative_amount": ROUTE_PRICE,
        "signature_hex": format!("0x{}", hex_encode(&claim_signature)),
    }))
    .expect("encode redeem body");
    let redeem_path = format!("/channels/{}/redeem", paid_channel.0);
    let (sig_input, sig, digest) = sign_request(
        &write_keypair,
        "POST",
        &redeem_path,
        &redeem_body,
        1_000,
        Some(9_999_999_999),
    );
    let response = client
        .post(format!(
            "http://{}{redeem_path}",
            connector.client_edge_addr
        ))
        .header("signature-input", sig_input)
        .header("signature", sig)
        .header("content-digest", digest)
        .body(redeem_body)
        .send()
        .await
        .expect("POST /channels/:id/redeem");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let redeemed: ChannelView = response.json().await.expect("decode ChannelView");
    assert_eq!(
        redeemed.redeemed, ROUTE_PRICE,
        "the claim's cumulative amount, read back from the chain through the operator \
         surface, advanced by exactly the route's price -- not inferred from the fulfil"
    );

    // AC4: an underpaid packet -- a fresh claim on a channel genuinely
    // funded with less than the route's price -- is refused, and the app
    // records nothing for it.
    let (underpaid_data, _underpaid_secret) =
        sealed_prepare_data(b"underpaid write attempt", &connector_identity);
    let underpaid_prepare = sample_prepare("g.example.app", underpaid_data);
    let (underpaid_claim, _) = evm_claim_json(
        &underpaid_buyer_secret,
        &underpaid_channel.0,
        1,
        underpaid_state.counterparty_deposited,
        token_network_address,
    );
    let body = post_ilp_packet(
        &client,
        &connector.client_edge_addr,
        &underpaid_claim,
        &underpaid_prepare,
        "POST /ilp: underpaid write",
    )
    .await;
    let reject = Reject::decode(&body).expect("decode reject");
    assert_eq!(reject.code.as_str(), "F03");

    let after_underpaid = recorded.lock().expect("recorded lock").clone();
    assert_eq!(
        after_underpaid.len(),
        1,
        "the underpaid packet never reached the app -- still exactly the one prior write"
    );
}

/// Issues #556/#502: **an unaffiliated buyer pays with no config edit.**
///
/// Everything about this test is the test above minus one thing: the
/// config file has no `[[client_channels]]` section at all. The buyer has
/// never registered with this operator, is named nowhere in the node's
/// configuration, and the node was started before their channel existed.
/// They open a channel with the connector on chain, fund it, sign a claim,
/// and the write lands -- because the connector resolves the channel's
/// counterparty from the deployed `TokenNetwork` its `[settlement]`
/// section already names.
///
/// That is issue #502's *"anonymity is a first-class path, not a fallback:
/// it is how an unaffiliated buyer pays for a terminated route without
/// registering with the operator first"*. On a tree without this change
/// this test fails: the connector holds a record of no channel, refuses
/// the claim F01 "no record of", and the app records nothing.
///
/// Note what is deliberately *not* relaxed: the claim is still verified
/// against the counterparty the chain holds, never against the
/// `signerAddress` it declares for itself. The second half of this test
/// signs an otherwise perfect claim with a key the chain does not know and
/// asserts it is still refused.
#[tokio::test]
async fn an_unaffiliated_buyer_pays_for_a_write_with_no_client_channels_configured() {
    if !require_anvil() {
        return;
    }

    let anvil = Anvil::spawn(ANVIL_BASE_PORT).await;
    let token =
        EvmSettlementBackend::deploy_mock_token(&anvil.rpc_url, DEPLOYER_PRIVATE_KEY, 1_000_000)
            .await
            .expect("mint a fresh mock ERC-20 for this test");
    let backend = EvmSettlementBackend::deploy(&anvil.rpc_url, DEPLOYER_PRIVATE_KEY, token)
        .await
        .expect("deploy a TokenNetwork through a fresh registry");
    let token_network_address = backend.address().to_fixed_bytes();
    let registry_address = backend.registry_address();

    // A buyer the operator has never heard of.
    let buyer_secret = SecretKey::parse(&[37u8; 32]).expect("valid secret key");
    let buyer_address = derive_evm_address(&PublicKey::from_secret_key(&buyer_secret).serialize());

    let (app_addr, recorded) = spawn_recording_app().await;

    // The node is configured and started *before* the buyer's channel
    // exists, and names no channel of anyone's.
    let key_file = write_raw_key_file(11);
    let mut settlement_key_file = tempfile::NamedTempFile::new().expect("temp settlement key file");
    settlement_key_file
        .write_all(DEPLOYER_PRIVATE_KEY.as_bytes())
        .expect("write settlement key file");
    // Issue #1186: this node declares no `[[client_channels]]` -- that is the
    // whole point of the test -- and resolves its buyer's channel from chain
    // instead. That is exactly the shape whose watermarks used to be allowed
    // to live in memory, so config load now requires a `state_dir` of it.
    let state_dir_handle = tempfile::tempdir().expect("temp state dir");
    let state_dir = state_dir_handle.path().display();
    let config = write_config(&format!(
        r#"
client_edge_addr = "127.0.0.1:0"
state_dir = "{state_dir}"

[signer]
key_file = "{key_file}"

[settlement]
chain = "evm"
rpc_url = "{rpc_url}"
contract_address = "{registry_address:?}"
token_address = "{token:?}"
decimals = 6

[settlement.key]
key_file = "{settlement_key_file}"

[[routes]]
prefix = "g.example.app"
handler_url = "http://{app_addr}"
price = {ROUTE_PRICE}

# Deliberately no `[[client_channels]]`: nothing in this file names the
# buyer, their channel, or the signing domain. Everything the claim is
# checked against is read from the chain named above.
"#,
        key_file = key_file.path().display(),
        settlement_key_file = settlement_key_file.path().display(),
        rpc_url = anvil.rpc_url,
    ));
    let connector = spawn_connector(config.path());
    let connector_identity = identity_from_key_seed(11);

    // Only now does the buyer open and fund their channel -- after the
    // node is already serving, so nothing about it could have been read at
    // startup even in principle.
    let channel = backend
        .open(buyer_address.to_vec(), ChronoDuration::hours(1))
        .await
        .expect("the buyer opens a channel with this connector");
    let state = backend
        .fund_counterparty(&channel, 10 * ROUTE_PRICE)
        .await
        .expect("fund it with real ERC-20 value");
    assert_eq!(state.counterparty_deposited, 10 * ROUTE_PRICE);

    let client = reqwest::Client::new();
    let (data, _shared_secret) = sealed_prepare_data(b"unaffiliated write", &connector_identity);
    let prepare = sample_prepare("g.example.app", data);
    let (claim, _signature) = evm_claim_json(
        &buyer_secret,
        &channel.0,
        1,
        ROUTE_PRICE,
        token_network_address,
    );
    let body = post_ilp_packet(
        &client,
        &connector.client_edge_addr,
        &claim,
        &prepare,
        "POST /ilp: unaffiliated paid write",
    )
    .await;
    Fulfill::decode(&body).expect(
        "a buyer whose channel exists only on chain pays without the operator editing config",
    );

    let after_paid = recorded.lock().expect("recorded lock").clone();
    assert_eq!(
        after_paid.len(),
        1,
        "the app recorded the unaffiliated buyer's write"
    );
    assert_eq!(after_paid[0].body.as_ref(), b"unaffiliated write");

    // And the guarantee #607 established is intact: a claim on the same,
    // genuinely on-chain channel, signed by somebody who is not its
    // counterparty, is still refused. Reading the record from a chain
    // changed where the counterparty comes from, not whether one is
    // required.
    let forger_secret = SecretKey::parse(&[38u8; 32]).expect("valid secret key");
    let (forged_data, _forged_shared) =
        sealed_prepare_data(b"forged write attempt", &connector_identity);
    let forged_prepare = sample_prepare("g.example.app", forged_data);
    let (forged_claim, _) = evm_claim_json(
        &forger_secret,
        &channel.0,
        2,
        2 * ROUTE_PRICE,
        token_network_address,
    );
    let body = post_ilp_packet(
        &client,
        &connector.client_edge_addr,
        &forged_claim,
        &forged_prepare,
        "POST /ilp: forged write",
    )
    .await;
    let reject = Reject::decode(&body).expect("decode reject");
    assert_eq!(reject.code.as_str(), "F01");

    let after_forged = recorded.lock().expect("recorded lock").clone();
    assert_eq!(
        after_forged.len(),
        1,
        "the forged claim never reached the app -- still exactly the one genuine write"
    );
}

/// Issue #631, the Solana twin of
/// `an_unaffiliated_buyer_pays_for_a_write_with_no_client_channels_configured`
/// above: a buyer holding only Solana devnet assets pays a write through
/// the official Rust edge with zero operator config. Everything about this
/// test is the EVM one minus the chain: a real, disposable
/// `solana-test-validator` running the real `packages/solana-program`
/// artifact, a real SPL mint, a real channel opened by the connector's own
/// on-chain identity naming an unaffiliated buyer as counterparty, and a
/// real deposit the buyer signs for themselves (the deployed program's
/// `Deposit` instruction requires the depositing participant's own
/// signature, unlike `TokenNetwork.setTotalDeposit`'s permissionless
/// EVM twin -- so this test cannot reuse `EvmSettlementBackend::fund`'s
/// shape and instead submits that one instruction directly).
///
/// On a tree without issue #631 this test fails: the connector holds a
/// record of no channel, refuses the claim F01 "no record of", and the app
/// records nothing. The second half proves the guarantee issue #558
/// established carries over to Solana: a claim on the same, genuinely
/// on-chain channel, signed by somebody who is not its counterparty, is
/// still refused.
#[tokio::test]
async fn an_unaffiliated_solana_buyer_pays_for_a_write_with_no_client_channels_configured() {
    use connector_settlement_solana::test_support::{
        fund, require_solana_test_validator, SolanaValidator, LOCAL_TEST_PROGRAM_ID,
    };
    use connector_settlement_solana::wire;
    use connector_settlement_solana::SolanaSettlementBackend;
    use solana_rpc_client::nonblocking::rpc_client::RpcClient;
    use solana_sdk::commitment_config::CommitmentConfig;
    use solana_sdk::instruction::Instruction;
    use solana_sdk::program_pack::Pack;
    use solana_sdk::pubkey::Pubkey;
    use solana_sdk::signature::{Keypair as SolanaSdkKeypair, Signer as SolanaSdkSigner};
    use solana_sdk::transaction::Transaction;
    use std::str::FromStr;

    // AC5's own policy, Solana-flavored: fail loudly in CI when the chain
    // this test needs is unavailable, never silently skip and report
    // success.
    if !require_solana_test_validator() {
        return;
    }

    let validator = SolanaValidator::spawn().await;
    let program_id = Pubkey::from_str(LOCAL_TEST_PROGRAM_ID).expect("valid local test program id");
    let rpc =
        RpcClient::new_with_commitment(validator.rpc_url.clone(), CommitmentConfig::confirmed());

    // Three real identities: this test's own mint-authority admin role, the
    // buyer -- an identity the operator has never heard of -- and the
    // connector's own on-chain identity, known ahead of time (a raw 32-byte
    // seed) so it can be reused as `[settlement.solana].key`'s key file
    // below and produce the exact same signing address.
    let mint_authority = SolanaSdkKeypair::new();
    let buyer = SolanaSdkKeypair::new();
    let production_seed = [61u8; 32];
    let production_payer = solana_sdk::signer::keypair::keypair_from_seed(&production_seed)
        .expect("derive the production identity from its seed");
    for pubkey in [
        mint_authority.pubkey(),
        buyer.pubkey(),
        production_payer.pubkey(),
    ] {
        fund(&rpc, &pubkey).await;
    }

    // A fresh 6-decimal SPL mint, matching every token this fleet settles
    // in (`docs/usdc-cross-chain-settlement.md`) -- never assumed
    // pre-funded.
    let mint = SolanaSdkKeypair::new();
    let rent = rpc
        .get_minimum_balance_for_rent_exemption(spl_token::state::Mint::LEN)
        .await
        .expect("rent exemption for a mint account");
    let create_mint_account = solana_sdk::system_instruction::create_account(
        &mint_authority.pubkey(),
        &mint.pubkey(),
        rent,
        spl_token::state::Mint::LEN as u64,
        &spl_token::id(),
    );
    let initialize_mint = spl_token::instruction::initialize_mint2(
        &spl_token::id(),
        &mint.pubkey(),
        &mint_authority.pubkey(),
        None,
        6,
    )
    .expect("pack initialize_mint2");
    let recent_blockhash = rpc.get_latest_blockhash().await.expect("recent blockhash");
    let create_mint_tx = Transaction::new_signed_with_payer(
        &[create_mint_account, initialize_mint],
        Some(&mint_authority.pubkey()),
        &[&mint_authority, &mint],
        recent_blockhash,
    );
    rpc.send_and_confirm_transaction(&create_mint_tx)
        .await
        .expect("create and initialize the mint");

    // The buyer's own associated token account, genuinely minted into --
    // exactly what a buyer who already holds this asset would have before
    // ever talking to this connector.
    let buyer_ata =
        spl_associated_token_account::get_associated_token_address(&buyer.pubkey(), &mint.pubkey());
    let create_buyer_ata =
        spl_associated_token_account::instruction::create_associated_token_account_idempotent(
            &mint_authority.pubkey(),
            &buyer.pubkey(),
            &mint.pubkey(),
            &spl_token::id(),
        );
    let mint_to_buyer = spl_token::instruction::mint_to(
        &spl_token::id(),
        &mint.pubkey(),
        &buyer_ata,
        &mint_authority.pubkey(),
        &[],
        1_000_000_000,
    )
    .expect("pack mint_to");
    let recent_blockhash = rpc.get_latest_blockhash().await.expect("recent blockhash");
    let fund_buyer_tx = Transaction::new_signed_with_payer(
        &[create_buyer_ata, mint_to_buyer],
        Some(&mint_authority.pubkey()),
        &[&mint_authority],
        recent_blockhash,
    );
    rpc.send_and_confirm_transaction(&fund_buyer_tx)
        .await
        .expect("mint real SPL-token value into the buyer's own account");

    // The connector's own settlement identity opens the channel -- the
    // production-shaped path (`InitializeChannel` needs only this side's
    // signature), naming the buyer as counterparty. This is exactly the
    // `SolanaSettlementBackend::connect` the spawned binary below builds
    // from config, sharing its seed, so the channel proven against here is
    // the one the running node will recognise as its own.
    let production_side = SolanaSettlementBackend::connect(
        &validator.rpc_url,
        &production_seed,
        program_id,
        mint.pubkey(),
        6,
    )
    .await
    .expect("connect the production-shaped identity");
    let channel = production_side
        .open(buyer.pubkey().to_bytes().to_vec(), ChronoDuration::hours(1))
        .await
        .expect("the connector opens a real channel naming the buyer as counterparty");

    // The buyer deposits real SPL-token value into their own side, signed
    // by themselves: `packages/solana-program`'s `Deposit` instruction
    // requires the depositing participant's own signature, so this is
    // exactly what a real buyer's wallet would submit -- never something
    // the connector could do on their behalf.
    let channel_pubkey = Pubkey::from_str(&channel.0).expect("channel id is a Solana pubkey");
    let (vault, _bump) = wire::vault_pda(&channel_pubkey, &program_id);
    let deposit_amount: u64 = 10 * SOLANA_ROUTE_PRICE;
    let deposit_instruction = Instruction::new_with_bytes(
        program_id,
        &wire::pack_deposit(deposit_amount),
        wire::Accounts::deposit(&buyer.pubkey(), &buyer_ata, &vault, &channel_pubkey),
    );
    let recent_blockhash = rpc.get_latest_blockhash().await.expect("recent blockhash");
    let deposit_tx = Transaction::new_signed_with_payer(
        &[deposit_instruction],
        Some(&buyer.pubkey()),
        &[&buyer],
        recent_blockhash,
    );
    rpc.send_and_confirm_transaction(&deposit_tx)
        .await
        .expect("the buyer funds their own deposit with a real confirmed transaction");

    let state = production_side
        .channel_state(&channel)
        .await
        .expect("read the funded channel back from the chain");
    assert_eq!(
        state.counterparty_deposited,
        u128::from(deposit_amount),
        "a real transaction genuinely moved this value on chain"
    );
    drop(production_side);

    // A real app: its own socket, its own process-independent record of
    // what it received.
    let (app_addr, recorded) = spawn_recording_app().await;

    // A real, spawned, compiled `connector` binary, started from a config
    // file naming no `[[client_channels]]` at all -- the only possible
    // record of the buyer's channel is what this node resolves live from
    // the chain named in `[settlement.solana]`.
    let key_file = write_raw_key_file(60);
    let solana_key_file = write_raw_key_file(61);
    // Issue #1186: this node declares no `[[client_channels]]` -- that is the
    // whole point of the test -- and resolves its buyer's channel from chain
    // instead. That is exactly the shape whose watermarks used to be allowed
    // to live in memory, so config load now requires a `state_dir` of it.
    let state_dir_handle = tempfile::tempdir().expect("temp state dir");
    let state_dir = state_dir_handle.path().display();
    let config = write_config(&format!(
        r#"
client_edge_addr = "127.0.0.1:0"
state_dir = "{state_dir}"

[signer]
key_file = "{key_file}"

[settlement.solana]
rpc_url = "{rpc_url}"
program_id = "{program_id}"
token_address = "{token_mint}"
decimals = 6

[settlement.solana.key]
key_file = "{solana_key_file}"

[[routes]]
prefix = "g.example.app"
handler_url = "http://{app_addr}"
price = {SOLANA_ROUTE_PRICE}

# Deliberately no `[[client_channels]]`: nothing in this file names the
# buyer, their channel, or its counterparty. Everything the claim is
# checked against is read from the chain named above.
"#,
        key_file = key_file.path().display(),
        solana_key_file = solana_key_file.path().display(),
        rpc_url = validator.rpc_url,
        token_mint = mint.pubkey(),
    ));
    let connector = spawn_connector(config.path());
    let connector_identity = identity_from_key_seed(60);

    let client = reqwest::Client::new();
    let (data, _shared_secret) =
        sealed_prepare_data(b"unaffiliated solana write", &connector_identity);
    let prepare = sample_prepare("g.example.app", data);

    let channel_bytes = channel_pubkey.to_bytes();
    let genuine_nonce = 1u64;
    let genuine_message = connector_signer::solana_balance_proof_message(
        &Pubkey::from_str(LOCAL_TEST_PROGRAM_ID)
            .expect("valid local test program id")
            .to_bytes(),
        &channel_bytes,
        genuine_nonce,
        SOLANA_ROUTE_PRICE,
    );
    let genuine_signature = buyer.sign_message(&genuine_message);
    let claim = solana_claim_json(
        &channel.0,
        LOCAL_TEST_PROGRAM_ID,
        genuine_nonce,
        SOLANA_ROUTE_PRICE,
        &base64_encode(genuine_signature.as_ref()),
        &buyer.pubkey().to_string(),
    );
    let body = post_ilp_packet(
        &client,
        &connector.client_edge_addr,
        &claim,
        &prepare,
        "POST /ilp: unaffiliated solana write",
    )
    .await;
    Fulfill::decode(&body).expect(
        "a Solana buyer whose channel exists only on chain pays without the operator editing \
         config",
    );

    let after_paid = recorded.lock().expect("recorded lock").clone();
    assert_eq!(
        after_paid.len(),
        1,
        "the app recorded the unaffiliated buyer's write"
    );
    assert_eq!(after_paid[0].body.as_ref(), b"unaffiliated solana write");

    // ADR 0040's other chain, end to end: the namespace the app is told
    // comes from the claim that was verified, so the same handler behind
    // the same connector says `solana` here and `evm` above.
    assert_eq!(
        recorded_header(&after_paid[0], "x-toon-payer"),
        Some(format!("solana:{}", channel.0).as_str())
    );
    assert_eq!(
        recorded_header(&after_paid[0], "x-toon-chain"),
        Some("solana")
    );

    // And the guarantee issue #558 established is intact: a claim on the
    // same, genuinely on-chain channel, signed by somebody who is not its
    // counterparty, is still refused. Reading the record from a chain
    // changed where the counterparty comes from, not whether one is
    // required.
    let forger = SolanaSdkKeypair::new();
    let (forged_data, _forged_shared) =
        sealed_prepare_data(b"forged solana write attempt", &connector_identity);
    let forged_prepare = sample_prepare("g.example.app", forged_data);
    let forged_nonce = 2u64;
    let forged_amount = 2 * SOLANA_ROUTE_PRICE;
    // The REAL program id, deliberately: this claim must be refused because
    // the SIGNER is wrong, not because the program is. Signing it under a
    // fixture program id would have the gate reject it for the wrong reason
    // and the test would stop proving what it says it proves.
    let forged_message = connector_signer::solana_balance_proof_message(
        &Pubkey::from_str(LOCAL_TEST_PROGRAM_ID)
            .expect("valid local test program id")
            .to_bytes(),
        &channel_bytes,
        forged_nonce,
        forged_amount,
    );
    let forged_signature = forger.sign_message(&forged_message);
    let forged_claim = solana_claim_json(
        &channel.0,
        LOCAL_TEST_PROGRAM_ID,
        forged_nonce,
        forged_amount,
        &base64_encode(forged_signature.as_ref()),
        &forger.pubkey().to_string(),
    );
    let body = post_ilp_packet(
        &client,
        &connector.client_edge_addr,
        &forged_claim,
        &forged_prepare,
        "POST /ilp: forged solana write",
    )
    .await;
    let reject = Reject::decode(&body).expect("decode reject");
    assert_eq!(reject.code.as_str(), "F01");

    let after_forged = recorded.lock().expect("recorded lock").clone();
    assert_eq!(
        after_forged.len(),
        1,
        "the forged claim never reached the app -- still exactly the one genuine write"
    );
}

fn base64_encode(bytes: &[u8]) -> String {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;
    STANDARD.encode(bytes)
}
