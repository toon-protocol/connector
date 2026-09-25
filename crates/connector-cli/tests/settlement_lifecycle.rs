//! A channel opened, funded, redeemed, closed **and settled** against a
//! real chain, entirely through the operator surface of a connector
//! process started from a config file (issue #542's last acceptance
//! criterion; the settle half is issue #1129). Unlike
//! `connector-operator`'s own `channel_lifecycle` tests, nothing here
//! constructs a `SettlementBackend` directly and hands it to a
//! hand-built `Connector` -- the config file's `[settlement]` section is
//! the only settlement backend this test ever names, and
//! `connector_cli::run` is what turns it into a working operator surface.
//!
//! That distinction is the whole point of proving `POST /channels/:id/settle`
//! here rather than only in `connector-operator`. Both backends are built
//! by `connect()`, the constructor a real node uses -- not by the
//! test-only `deploy()` whose privately-held extra keypairs let a fixture
//! do things no deployed node can (the blind spot PR #1124 found in the
//! contract suite). A settle that works only under `deploy()` would be a
//! settle that works nowhere.

use std::io::Write;

use ed25519_dalek::Keypair;
use libsecp256k1::{Message, PublicKey, SecretKey};
use rand::rngs::OsRng;
use tower::ServiceExt;

use axum::body::Body;
use axum::http::{Request, StatusCode};

use connector_operator::test_support::sign_request;
use connector_runtime::{ChannelView, ChannelViewStatus};
use connector_settlement::SettlementBackend;
use connector_settlement_evm::test_support::{
    require_anvil, Anvil, COUNTERPARTY_PRIVATE_KEY, DEPLOYER_PRIVATE_KEY,
};
use connector_settlement_evm::EvmSettlementBackend;
use connector_signer::{derive_evm_address, evm_balance_proof_digest, to_hex, EvmBalanceProof};
use ethers::providers::{Http, Middleware, Provider};

/// `anvil`'s own default chain id (`Anvil::spawn`'s `--chain-id 31337`),
/// and so the EIP-712 domain a claim against its deployed `TokenNetwork`
/// must be signed under.
const ANVIL_CHAIN_ID: u64 = 31_337;

/// Sign `digest` exactly the way the production signing path does
/// (`connector_signer::crypto::sign_digest`): a 65-byte `r || s || v`
/// signature with `v` in libsecp256k1's raw `{0, 1}` range, not the
/// `{27, 28}` an EVM wallet would append. `EvmSettlementBackend` itself
/// normalizes that before submitting to `claimFromChannel` (issue #590) --
/// signing the wallet range here would paper over that normalization
/// never being exercised by this end-to-end test.
fn sign_evm(secret: &SecretKey, digest: &[u8; 32]) -> Vec<u8> {
    let message = Message::parse(digest);
    let (signature, recovery_id) = libsecp256k1::sign(&message, secret);
    let mut bytes = signature.serialize().to_vec();
    let recovery_byte: u8 = recovery_id.into();
    bytes.push(recovery_byte);
    bytes
}

/// The raw bytes of a `0x`-optional hex private key.
fn hex_bytes(hex: &str) -> Vec<u8> {
    let hex = hex.trim_start_matches("0x");
    (0..hex.len() / 2)
        .map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).expect("key is hex"))
        .collect()
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

/// This test binary's own base port for [`Anvil::spawn`] -- distinct from
/// other test binaries' bases (`connector-settlement-evm`'s own tests use
/// 18_600; `connector-bin`'s use 18_500; `connector-cli`'s own
/// `settlement_construction` unit tests use 18_700) so that binaries
/// running concurrently under `cargo test --workspace` don't contend for
/// the same port range.
const ANVIL_BASE_PORT: u16 = 18_800;

/// A distinct `created` per signed request. The operator surface rejects a
/// replayed signature (ADR 0008's #1067 amendment), so two writes that are
/// byte-identical -- same method, same path, same empty body, as a refused
/// settle and its retry after the window are -- must not sign identically,
/// or the retry is a 401 rather than the settle under test.
static NEXT_CREATED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1_000);

fn signed_post(keypair: &Keypair, path: &str, body: Vec<u8>) -> Request<Body> {
    let created = NEXT_CREATED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let (sig_input, sig, digest) =
        sign_request(keypair, "POST", path, &body, created, Some(9_999_999_999));
    Request::builder()
        .method("POST")
        .uri(path)
        .header("signature-input", sig_input)
        .header("signature", sig)
        .header("content-digest", digest)
        .body(Body::from(body))
        .unwrap()
}

async fn body_channel_view(response: axum::response::Response) -> ChannelView {
    let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

/// AC: "A channel is opened, funded, redeemed and closed against a real
/// chain through the operator surface of a connector process started
/// from a config file, with no backend injected by the test."
#[tokio::test]
async fn a_channel_lifecycle_reaches_a_real_chain_through_a_config_driven_node() {
    // `require_anvil`, not a bare availability check: this test now carries
    // the EVM half of issue #1129's proof, and a gate that returns early
    // and reports `passed` in CI would be worse than no gate. It panics
    // when `CI` is set and skips only on a developer machine without
    // Foundry (issue #471).
    if !require_anvil() {
        return;
    }

    let anvil = Anvil::spawn(ANVIL_BASE_PORT).await;
    let token =
        EvmSettlementBackend::deploy_mock_token(&anvil.rpc_url, DEPLOYER_PRIVATE_KEY, 1_000_000)
            .await
            .expect("deploy mock USDC");
    let backend = EvmSettlementBackend::deploy(&anvil.rpc_url, DEPLOYER_PRIVATE_KEY, token)
        .await
        .expect("deploy a TokenNetwork through a fresh registry");
    let registry_address = backend.registry_address();
    let token_network_address = backend.address();

    // The channel's counterparty must be a real address able to sign a
    // balance proof `TokenNetwork.claimFromChannel` recovers (issue #576)
    // -- and, as of issue #1118, one that can also make its own on-chain
    // deposit, since no node can make it for them. So it is anvil's second
    // genesis account: genesis-funded with ETH for its own gas, and a
    // different address from the node's, which two `EvmSettlementBackend`s
    // over one nonce sequence could not be.
    let counterparty_secret = SecretKey::parse_slice(&hex_bytes(COUNTERPARTY_PRIVATE_KEY))
        .expect("anvil's second dev key is a valid secp256k1 secret");
    let counterparty_public = PublicKey::from_secret_key(&counterparty_secret);
    let counterparty_address = derive_evm_address(&counterparty_public.serialize());

    // Mock USDC for the counterparty to deposit, minted before the node
    // starts: this write signs as the deployer, and once the node is
    // serving the deployer's nonce sequence belongs to the node alone.
    backend
        .mint_mock_tokens_to(counterparty_address.into(), 1_000_000)
        .await
        .expect("mint the counterparty something of its own to deposit");
    drop(backend);

    let mut key_file = tempfile::NamedTempFile::new().expect("temp key file");
    key_file
        .write_all(DEPLOYER_PRIVATE_KEY.as_bytes())
        .expect("write key file");

    let keypair = Keypair::generate(&mut OsRng);
    let write_key_hex = keypair
        .public
        .to_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();

    let mut config_file = tempfile::NamedTempFile::new().expect("temp config file");
    write!(
        config_file,
        r#"
client_edge_addr = "127.0.0.1:0"
# CF-39 (issue #1186): a settlement table means this node resolves channels
# from chain and takes their claims, so it must keep watermarks durably.
# A fresh directory per test, never a shared one -- these journals are real,
# and a constant path lets one test's accepted claim become the next test's
# replay.
state_dir = "{state_dir}"

[signer]
key_file = "{key_path}"

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
key_file = "{key_path}"
"#,
        state_dir = tempfile::tempdir()
            .expect("temp state dir")
            .keep()
            .display(),
        key_path = key_file.path().display(),
        rpc_url = anvil.rpc_url,
    )
    .expect("write config file");

    let command = connector_cli::run(&[
        "connector".to_string(),
        config_file.path().display().to_string(),
    ])
    .await
    .expect("run: config-driven node with settlement configured");
    // A bare config path is still `serve` (issue #784's subcommand
    // boundary): the only other `Command` a run can produce is an announce
    // that has already finished, which no path argument can select.
    let connector_cli::Command::Serve(node) = command else {
        panic!("a config path must produce a servable node");
    };
    let app = node.router;

    // Open.
    let open_body = serde_json::to_vec(&serde_json::json!({
        "counterparty_hex": to_hex(&counterparty_address),
        "settlement_timeout_seconds": 3600,
    }))
    .unwrap();
    let response = app
        .clone()
        .oneshot(signed_post(&keypair, "/channels", open_body))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let opened: ChannelView = body_channel_view(response).await;
    assert_eq!(opened.deposited, 0);

    // Fund.
    let fund_body = serde_json::to_vec(&serde_json::json!({ "amount": 1_000 })).unwrap();
    let fund_path = format!("/channels/{}/fund", opened.id);
    let response = app
        .clone()
        .oneshot(signed_post(&keypair, &fund_path, fund_body))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let funded: ChannelView = body_channel_view(response).await;
    // `POST /channels/:id/fund` is a SELF-deposit (issue #1118): it raises
    // this node's own collateral and leaves the counterparty's side alone.
    // Before #1118 it credited the counterparty instead -- a delegate
    // deposit only `TokenNetwork` permits, which made the identical
    // endpoint an unconditional error on Solana. This assertion is the
    // behaviour change, stated at the surface an operator actually calls.
    assert_eq!(funded.own_deposited, 1_000);
    assert_eq!(funded.deposited, 0);

    // The counterparty's own deposit -- the side a claim they sign is
    // redeemed out of, and the one no write on this node can make. This is
    // the production shape, not a fixture shortcut: a second connector
    // bound to the same `TokenNetwork` under the *counterparty's* own key,
    // calling the same `fund` the node just called, which credits whichever
    // side is calling. Both sides of this channel are now collateralised by
    // the same port method, each for itself.
    let counterparty_backend = EvmSettlementBackend::connect(
        &connector_settlement_evm::RpcTransport::direct(&anvil.rpc_url).expect("rpc transport"),
        COUNTERPARTY_PRIVATE_KEY,
        registry_address,
        token,
        6,
    )
    .await
    .expect("bind the counterparty's own identity to the same TokenNetwork");
    let counterparty_funded = counterparty_backend
        .fund(&connector_settlement::ChannelId(opened.id.clone()), 1_000)
        .await
        .expect("the counterparty deposits on their own side");
    // Read from the counterparty's point of view, so `own_deposited` is
    // theirs -- the mirror image of what the node saw a moment ago.
    assert_eq!(counterparty_funded.own_deposited, 1_000);
    assert_eq!(counterparty_funded.counterparty_deposited, 1_000);

    // Redeem: `TokenNetwork.claimFromChannel` verifies a real EIP-712
    // signature over the balance proof (issue #576), recovered against the
    // channel's counterparty -- a genuine claim, signed by the same key
    // that opened the channel as counterparty above, rather than an
    // arbitrary placeholder.
    let proof = EvmBalanceProof {
        channel_id: channel_id_bytes(&opened.id),
        nonce: 1,
        transferred_amount: 400,
        locked_amount: 0,
        locks_root: [0u8; 32],
        chain_id: ANVIL_CHAIN_ID,
        token_network_address: token_network_address.to_fixed_bytes(),
    };
    let signature = sign_evm(&counterparty_secret, &evm_balance_proof_digest(&proof));
    let signature_hex = format!(
        "0x{}",
        signature
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    );
    let redeem_body = serde_json::to_vec(&serde_json::json!({
        "nonce": 1,
        "cumulative_amount": 400,
        "signature_hex": signature_hex,
    }))
    .unwrap();
    let redeem_path = format!("/channels/{}/redeem", opened.id);
    let response = app
        .clone()
        .oneshot(signed_post(&keypair, &redeem_path, redeem_body))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let redeemed: ChannelView = body_channel_view(response).await;
    assert_eq!(redeemed.redeemed, 400);
    assert_eq!(redeemed.deposited, 1_000);
    assert_eq!(redeemed.own_deposited, 1_000);

    // Close.
    let close_path = format!("/channels/{}/close", opened.id);
    let response = app
        .clone()
        .oneshot(signed_post(&keypair, &close_path, Vec::new()))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let closed: ChannelView = body_channel_view(response).await;
    assert_eq!(closed.status, ChannelViewStatus::Closed);

    // Funding a closed channel is rejected, not silently accepted -- but
    // the channel is NOT finished: `close` started a challenge period
    // (issue #574) and every un-claimed deposit is still on chain.
    let fund_again_body = serde_json::to_vec(&serde_json::json!({ "amount": 1 })).unwrap();
    let response = app
        .clone()
        .oneshot(signed_post(&keypair, &fund_path, fund_again_body))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    // Settling inside the window is refused by name (issue #1129), so an
    // operator who settles too early is told to wait rather than handed a
    // generic backend failure they would have to decode.
    let settle_path = format!("/channels/{}/settle", opened.id);
    let response = app
        .clone()
        .oneshot(signed_post(&keypair, &settle_path, Vec::new()))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
    let message = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(
        message.contains("not yet due"),
        "the refusal must name the challenge window, not fail generically: {message}"
    );

    // The node's ERC-20 balance before the settle, so the payout below is
    // measured in tokens actually returned rather than inferred from a
    // status field. This node deposited 1_000 of its own and the
    // counterparty redeemed none of it (the only claim redeemed above was
    // the counterparty's, out of THEIR deposit), so all 1_000 comes back.
    let node_address = derive_evm_address(
        &PublicKey::from_secret_key(
            &SecretKey::parse_slice(&hex_bytes(DEPLOYER_PRIVATE_KEY)).expect("anvil dev key"),
        )
        .serialize(),
    );
    let balance_before = erc20_balance_of(&anvil.rpc_url, token, node_address.into()).await;

    // Past the window. `TokenNetwork` enforces a one-hour minimum
    // settlement timeout, so this moves anvil's own clock rather than
    // sleeping for an hour.
    advance_anvil_time(&anvil.rpc_url, 3_601).await;

    let response = app
        .clone()
        .oneshot(signed_post(&keypair, &settle_path, Vec::new()))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let settled: ChannelView = body_channel_view(response).await;
    assert_eq!(settled.status, ChannelViewStatus::Settled);

    // The money actually came back. This is the assertion the endpoint
    // exists for: before issue #1129 a closed channel's remainder was
    // reachable only by calling `TokenNetwork.settleChannel` out of band.
    let balance_after = erc20_balance_of(&anvil.rpc_url, token, node_address.into()).await;
    assert_eq!(
        balance_after - balance_before,
        ethers::types::U256::from(1_000u64),
        "settling must return this node's own un-claimed deposit to it"
    );

    // Settled is terminal: a second settle is refused rather than
    // double-paying or reverting anonymously.
    let response = app
        .oneshot(signed_post(&keypair, &settle_path, Vec::new()))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

/// Advance `anvil`'s own chain clock by `seconds` and mine a block, so a
/// channel's settlement timeout becomes due without this test sleeping out
/// `TokenNetwork`'s one-hour `MIN_SETTLEMENT_TIMEOUT` in real time. The
/// same helper `connector-settlement-evm`'s own contract suite uses; it
/// lives in a `tests/` file there, so it cannot be imported.
async fn advance_anvil_time(rpc_url: &str, seconds: i64) {
    let provider = Provider::<Http>::try_from(rpc_url).expect("build provider");
    let _: serde_json::Value = provider
        .request("evm_increaseTime", [seconds])
        .await
        .expect("evm_increaseTime");
    let _: serde_json::Value = provider.request("evm_mine", ()).await.expect("evm_mine");
}

/// `balanceOf(address)` against the mock ERC-20, read straight off the
/// chain rather than through any connector type -- so the payout assertion
/// above cannot be satisfied by a bug in this workspace's own bookkeeping.
async fn erc20_balance_of(
    rpc_url: &str,
    token: ethers::types::Address,
    owner: ethers::types::Address,
) -> ethers::types::U256 {
    let provider = Provider::<Http>::try_from(rpc_url).expect("build provider");
    // `balanceOf(address)` == keccak256("balanceOf(address)")[..4].
    let mut data = vec![0x70, 0xa0, 0x82, 0x31];
    data.extend_from_slice(&[0u8; 12]);
    data.extend_from_slice(owner.as_bytes());
    let call = ethers::types::transaction::eip2718::TypedTransaction::Legacy(
        ethers::types::TransactionRequest::new()
            .to(token)
            .data(data),
    );
    let bytes = provider
        .call(&call, None)
        .await
        .expect("balanceOf call succeeds");
    ethers::types::U256::from_big_endian(&bytes)
}

/// The Solana twin of the lifecycle above, and the reason
/// `POST /channels/:id/settle` had to be an endpoint rather than a runbook
/// entry (issue #1129).
///
/// On EVM an operator locked out of this write still has a way out:
/// `TokenNetwork.settleChannel(bytes32)` is a plain ABI call and
/// `cast send` can build it -- `docs/operators/peer-channel-migration.md`
/// step 10 tells an operator to do exactly that. On Solana there is no
/// such fallback. `SettleChannel` is an eight-byte discriminator over
/// eight accounts, two of them program-derived (the channel PDA and its
/// vault) and two of them associated token accounts, and no `solana` or
/// `spl-token` subcommand assembles an arbitrary instruction. That is the
/// same property that made `InitializeChannel` a node write instead of a
/// script (issue #459, ADR 0008): the only submitter is a running node.
///
/// So this test is the load-bearing one. The node is built from a config
/// file, which means `SolanaSettlementBackend::connect` -- the constructor
/// a deployed node uses, not the test-only `deploy()` whose privately-held
/// counterparty keypairs hid a Solana `fund` failure from the contract
/// suite until PR #1124. The only thing `deploy()` is used for here is to
/// be the mock mint's authority, which is a faucet, not the subject.
#[tokio::test]
async fn a_solana_channel_is_settled_through_the_operator_surface_of_a_config_driven_node() {
    use connector_settlement_solana::test_support::{
        fund as fund_sol, require_solana_test_validator, SolanaValidator, LOCAL_TEST_PROGRAM_ID,
    };
    use connector_settlement_solana::SolanaSettlementBackend;
    use solana_rpc_client::nonblocking::rpc_client::RpcClient;
    use solana_sdk::commitment_config::CommitmentConfig;
    use solana_sdk::pubkey::Pubkey;
    use solana_sdk::signer::Signer as SolanaSigner;
    use std::str::FromStr;

    if !require_solana_test_validator() {
        return;
    }

    let validator = SolanaValidator::spawn().await;
    let program_id = Pubkey::from_str(LOCAL_TEST_PROGRAM_ID).expect("valid local test program id");

    // A `deploy()`-built backend purely as the mock mint's authority: it
    // creates the mint and can hand out tokens. Nothing it can do that a
    // real node cannot is used below -- the node under test is built from
    // a config file, by `connect()`.
    let mint_authority = SolanaSettlementBackend::deploy(&validator.rpc_url, program_id)
        .await
        .expect("bind to the genesis-loaded payment-channel program");
    let token_mint = mint_authority.token_mint();

    // The node's own settlement identity: a fresh seed, SOL for fees, and
    // real mock USDC of its own to collateralise with. `fund` is a
    // self-deposit (issue #1118), so a node with an empty ATA cannot open
    // a collateralised channel at all -- exactly as in a real deployment.
    let seed = [37u8; 32];
    let node_keypair =
        solana_sdk::signer::keypair::keypair_from_seed(&seed).expect("derive node keypair");
    let node_pubkey = node_keypair.pubkey();
    let rpc =
        RpcClient::new_with_commitment(validator.rpc_url.clone(), CommitmentConfig::confirmed());
    fund_sol(&rpc, &node_pubkey).await;
    mint_authority
        .test_mint_tokens_to(&node_pubkey, 10_000)
        .await
        .expect("give the node real tokens of its own");
    drop(mint_authority);

    let node_ata =
        spl_associated_token_account::get_associated_token_address(&node_pubkey, &token_mint);
    let token_balance = |ata: Pubkey| {
        let url = validator.rpc_url.clone();
        async move {
            let rpc = RpcClient::new_with_commitment(url, CommitmentConfig::confirmed());
            rpc.get_token_account_balance(&ata)
                .await
                .expect("read the SPL balance")
                .amount
                .parse::<u64>()
                .expect("an SPL balance is an integer of base units")
        }
    };

    let mut key_file = tempfile::NamedTempFile::new().expect("temp key file");
    key_file
        .write_all(&seed)
        .expect("write raw 32-byte key file");

    let keypair = Keypair::generate(&mut OsRng);
    let write_key_hex = keypair
        .public
        .to_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();

    let mut config_file = tempfile::NamedTempFile::new().expect("temp config file");
    write!(
        config_file,
        r#"
client_edge_addr = "127.0.0.1:0"
# CF-39 (issue #1186): a settlement table means this node resolves channels
# from chain and takes their claims, so it must keep watermarks durably.
# A fresh directory per test, never a shared one -- these journals are real,
# and a constant path lets one test's accepted claim become the next test's
# replay.
state_dir = "{state_dir}"

[signer]
key_file = "{key_path}"

[operator]
bearer_token = "operator-secret"
write_keys = ["{write_key_hex}"]

[settlement.solana]
rpc_url = "{rpc_url}"
program_id = "{program_id}"
token_address = "{token_mint}"
decimals = 6

[settlement.solana.key]
key_file = "{key_path}"
"#,
        state_dir = tempfile::tempdir()
            .expect("temp state dir")
            .keep()
            .display(),
        key_path = key_file.path().display(),
        rpc_url = validator.rpc_url,
    )
    .expect("write config file");

    let command = connector_cli::run(&[
        "connector".to_string(),
        config_file.path().display().to_string(),
    ])
    .await
    .expect("run: config-driven node with Solana settlement configured");
    let connector_cli::Command::Serve(node) = command else {
        panic!("a config path must produce a servable node");
    };
    let app = node.router;

    let balance_at_rest = token_balance(node_ata).await;
    assert_eq!(balance_at_rest, 10_000, "the faucet above actually landed");

    // Open, with a zero-length challenge period so this test can settle
    // for real without waiting one out. `packages/solana-program` imposes
    // no minimum -- unlike `TokenNetwork`'s one hour, which is why the EVM
    // half above has to move anvil's clock instead.
    let counterparty = solana_sdk::signature::Keypair::new().pubkey();
    // `connector_signer::to_hex` renders a 20-byte EVM address; a Solana
    // counterparty is a 32-byte pubkey, which `POST /channels` takes as the
    // same opaque hex the port itself takes as opaque bytes.
    let counterparty_hex = counterparty
        .to_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let open_body = serde_json::to_vec(&serde_json::json!({
        "counterparty_hex": counterparty_hex,
        "settlement_timeout_seconds": 0,
    }))
    .unwrap();
    let response = app
        .clone()
        .oneshot(signed_post(&keypair, "/channels", open_body))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let opened: ChannelView = body_channel_view(response).await;
    assert_eq!(opened.own_deposited, 0);

    // Fund: a real `Deposit`, the node's own tokens into the channel vault.
    let fund_body = serde_json::to_vec(&serde_json::json!({ "amount": 1_000 })).unwrap();
    let fund_path = format!("/channels/{}/fund", opened.id);
    let response = app
        .clone()
        .oneshot(signed_post(&keypair, &fund_path, fund_body))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let funded: ChannelView = body_channel_view(response).await;
    assert_eq!(funded.own_deposited, 1_000);
    assert_eq!(
        token_balance(node_ata).await,
        9_000,
        "the deposit left the node's own account for the vault"
    );

    // Settling an *open* channel is refused: there is no deadline to have
    // passed if `close` was never called, and the port says so by name
    // rather than letting the chain revert anonymously.
    let settle_path = format!("/channels/{}/settle", opened.id);
    let response = app
        .clone()
        .oneshot(signed_post(&keypair, &settle_path, Vec::new()))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let bytes = hyper::body::to_bytes(response.into_body()).await.unwrap();
    let message = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(
        message.contains("not yet due"),
        "an unclosed channel is not settleable, and must say so: {message}"
    );

    // Close: this starts the challenge period. Before issue #1129 the run
    // ended here, with 1_000 of the node's own USDC in a vault no surface
    // could open.
    let close_path = format!("/channels/{}/close", opened.id);
    let response = app
        .clone()
        .oneshot(signed_post(&keypair, &close_path, Vec::new()))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let closed: ChannelView = body_channel_view(response).await;
    assert_eq!(closed.status, ChannelViewStatus::Closed);
    assert_eq!(
        token_balance(node_ata).await,
        9_000,
        "closing pays nothing back -- that is the whole point of this issue"
    );

    // Settle: the challenge period is zero-length, so it is already due.
    let response = app
        .clone()
        .oneshot(signed_post(&keypair, &settle_path, Vec::new()))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let settled: ChannelView = body_channel_view(response).await;
    assert_eq!(settled.status, ChannelViewStatus::Settled);

    // The collateral is back in the node's own account. Read off the chain
    // through the raw RPC, not through any connector type.
    assert_eq!(
        token_balance(node_ata).await,
        10_000,
        "settling must return the node's un-claimed deposit to its own ATA"
    );

    // Settled is terminal on this chain too: the program zeroes the channel
    // PDA, so a second settle is refused rather than replayed.
    let response = app
        .oneshot(signed_post(&keypair, &settle_path, Vec::new()))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}
