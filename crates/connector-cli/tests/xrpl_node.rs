//! ADR 0087, built (#1614): a node boots on XRPL and publishes it.
//!
//! Two kinds of test, because they answer different questions:
//!
//! * **Against a scripted ledger** ([`FakeRpc`]). Runs on every machine and
//!   holds what the node does with what a ledger *says*: the boot refusals
//!   (a table key that is not the ledger's `network_id`, an account that is
//!   missing or below the base reserve, two tables on one account), the
//!   greeting and the self-description it publishes, and the vouchers its
//!   claim gate admits, journals and restores.
//! * **Against a spawned `xrpld`** (`Xrpld::spawn`, ADR 0084): the real
//!   runtime booted on a real stand-alone ledger. A missing Docker fails CI
//!   and skips locally (`require_xrpld`).
//!
//! Every node is built through `connector_cli::build` and `router`, the
//! calls the binary makes.

mod support;

use std::collections::HashMap;
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use connector_chain_rpc::{FakeRpc, RpcReply};
use connector_cli::RuntimeError;
use connector_domain::x402::{X402BatchSettlementTerms, X402BatchSettlementXrplTerms};
use connector_domain::{Fulfill, Reject};
use connector_settlement_xrpl::test_support::{
    require_xrpld, LedgerCloser, Xrpld, STANDALONE_NETWORK_ID,
};
use connector_signer::{xrpl_family_seed, XrplAlgorithm, XrplSigner};
use serde_json::{json, Value};
use tower::ServiceExt;

use support::{paid_prepare, post_ilp, spawn_recording_app};

const ROUTE: &str = "g.toon.xrpl";
/// An operator write key (the public half): the surface needs one to mount.
/// The tests only read, with the bearer token.
const OPERATOR_WRITE_KEY: &str = "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a";
const PRICE: u64 = 100;
const ONE_DAY: u64 = 86_400;
/// The base reserve every scripted ledger below reports, in XRP.
const RESERVE_XRP: u64 = 1;
const RESERVE_DROPS: u64 = RESERVE_XRP * 1_000_000;

fn seed(algorithm: XrplAlgorithm, entropy: u8) -> String {
    xrpl_family_seed(algorithm, &[entropy; 16])
}

fn signer(seed: &str) -> XrplSigner {
    XrplSigner::from_family_seed(seed).expect("a family seed")
}

/// A key file holding `seed`, the shape `[settlement.xrpl.<id>.key]` names.
fn key_file(seed: &str) -> tempfile::NamedTempFile {
    let mut file = tempfile::NamedTempFile::new().expect("key file");
    file.write_all(seed.as_bytes()).expect("write");
    file
}

/// What a scripted ledger holds. Cloned into the script, so a test can
/// change what the node's next read sees.
#[derive(Clone)]
struct Ledger {
    /// `server_info.network_id`; `None` is a node that reports none.
    network_id: Option<u32>,
    /// The accounts the ledger holds, by classic address, with their drops.
    accounts: Arc<Mutex<HashMap<String, u64>>>,
    /// `PayChannel` entries by lowercase channel id.
    channels: Arc<Mutex<HashMap<String, Value>>>,
    /// Whether `server_info` reports a validated ledger; `false` is an
    /// endpoint still syncing, with no base reserve to read.
    validated: bool,
}

impl Ledger {
    fn on(network_id: u32) -> Ledger {
        Ledger {
            network_id: Some(network_id),
            accounts: Arc::default(),
            channels: Arc::default(),
            validated: true,
        }
    }

    fn syncing(self) -> Ledger {
        Ledger {
            validated: false,
            ..self
        }
    }

    fn with_account(self, address: &str, drops: u64) -> Ledger {
        self.accounts
            .lock()
            .unwrap()
            .insert(address.to_string(), drops);
        self
    }

    /// A `PayChannel` entry from `payer` to `node`, with `extra` fields laid
    /// over a plain admissible one.
    fn open_channel(&self, id: &str, node: &str, payer: &XrplSigner, drops: u64, extra: Value) {
        let mut entry = json!({
            "LedgerEntryType": "PayChannel",
            "Account": payer.classic_address(),
            "Destination": node,
            "Amount": drops.to_string(),
            "Balance": "0",
            "SettleDelay": ONE_DAY,
            "PublicKey": hex::encode_upper(payer.public_key()),
        });
        for (name, value) in extra.as_object().expect("an object") {
            entry[name] = value.clone();
        }
        self.channels.lock().unwrap().insert(id.to_string(), entry);
    }

    async fn serve(&self) -> FakeRpc {
        let ledger = self.clone();
        FakeRpc::spawn(move |call| ledger.answer(&call.method, &call.params)).await
    }

    fn answer(&self, method: &str, params: &Value) -> RpcReply {
        let success = |mut body: Value| {
            body["status"] = json!("success");
            RpcReply::Result(body)
        };
        let error = |code: &str| {
            RpcReply::Result(json!({ "status": "error", "error": code, "error_message": code }))
        };
        // rippled takes its one params object in a list.
        let params = &params[0];
        match method {
            "server_info" => {
                let mut info = if self.validated {
                    json!({
                        "server_state": "full",
                        "validated_ledger": { "seq": 5, "reserve_base_xrp": RESERVE_XRP },
                    })
                } else {
                    json!({ "server_state": "syncing" })
                };
                if let Some(id) = self.network_id {
                    info["network_id"] = json!(id);
                }
                success(json!({ "info": info }))
            }
            "account_info" => {
                let address = params["account"].as_str().unwrap_or_default();
                match self.accounts.lock().unwrap().get(address) {
                    Some(drops) => success(json!({ "account_data": {
                        "Balance": drops.to_string(), "Sequence": 3, "OwnerCount": 0,
                    }})),
                    None => error("actNotFound"),
                }
            }
            "ledger_entry" => {
                let index = params["index"].as_str().unwrap_or_default();
                match self
                    .channels
                    .lock()
                    .unwrap()
                    .get(&index.to_ascii_lowercase())
                {
                    Some(node) => success(json!({ "node": node })),
                    None => error("entryNotFound"),
                }
            }
            other => panic!("the scripted ledger does not serve {other}"),
        }
    }
}

/// A node's files: its identity key, the XRPL key file(s) its tables name and
/// its state directory.
struct Files {
    signer_key: tempfile::NamedTempFile,
    state_dir: tempfile::TempDir,
}

impl Files {
    fn new() -> Files {
        let mut signer_key = tempfile::NamedTempFile::new().expect("signer key");
        signer_key.write_all(&[0x07; 32]).expect("write");
        Files {
            signer_key,
            state_dir: tempfile::tempdir().expect("state dir"),
        }
    }

    /// The config text for `tables`, one paid route to `app_addr`, and an
    /// operator surface reading with a bearer token.
    fn text(&self, tables: &str, app_addr: &str) -> String {
        format!(
            r#"
client_edge_addr = "127.0.0.1:0"
state_dir = "{state_dir}"

[signer]
key_file = "{signer_key}"

[operator]
bearer_token = "operator-secret"
write_keys = ["{write_key}"]
{tables}
[[routes]]
prefix = "{ROUTE}"
handler_url = "http://{app_addr}"
price = {PRICE}
"#,
            state_dir = self.state_dir.path().display(),
            signer_key = self.signer_key.path().display(),
            write_key = OPERATOR_WRITE_KEY,
        )
    }

    fn config(&self, tables: &str, app_addr: &str) -> connector_config::Config {
        support::load_config(&self.text(tables, app_addr))
    }
}

/// A `[settlement.xrpl.<network>]` table.
fn table(network: u32, rpc_url: &str, key_file: &std::path::Path, extra: &str) -> String {
    format!(
        r#"
[settlement.xrpl.{network}]
rpc_url = "{rpc_url}"
min_settle_delay_secs = {ONE_DAY}
redemption_safety_secs = 90
{extra}

[settlement.xrpl.{network}.key]
key_file = "{key}"
"#,
        key = key_file.display(),
    )
}

/// The greeting an unpaid write to [`ROUTE`] is answered with.
async fn greeting(app: &axum::Router) -> Value {
    let prepare = connector_domain::Prepare {
        amount: 0,
        expires_at: chrono::Utc::now() + chrono::Duration::seconds(30),
        greeting: true,
        destination: ROUTE.to_string(),
        data: Vec::new(),
    };
    let request = Request::builder()
        .method("POST")
        .uri("/ilp")
        .body(Body::from(prepare.encode()))
        .expect("a request");
    let response = app.clone().oneshot(request).await.expect("an answer");
    assert_eq!(response.status(), StatusCode::PAYMENT_REQUIRED);
    let bytes = hyper::body::to_bytes(response.into_body())
        .await
        .expect("the body");
    serde_json::from_slice(&bytes).expect("JSON terms")
}

/// An operator read, with the bearer token.
async fn read(app: &axum::Router, path: &str) -> Value {
    let request = Request::builder()
        .method("GET")
        .uri(path)
        .header("authorization", "Bearer operator-secret")
        .body(Body::empty())
        .expect("a request");
    let response = app.clone().oneshot(request).await.expect("an answer");
    let status = response.status();
    let bytes = hyper::body::to_bytes(response.into_body())
        .await
        .expect("the body");
    assert_eq!(
        status,
        StatusCode::OK,
        "GET {path}: {}",
        String::from_utf8_lossy(&bytes)
    );
    serde_json::from_slice(&bytes).expect("JSON")
}

/// An XRPL voucher on the wire (ADR 0087 decision 2): the channel id bare
/// hex, the signature hex, and the network the claim itself cannot bind.
fn voucher(network: &str, channel: &str, drops: u64, signature: &[u8]) -> String {
    json!({
        "version": "1.0",
        "blockchain": "xrpl",
        "network": network,
        "scheme": "batch-settlement",
        "messageId": format!("voucher-{drops}"),
        "timestamp": "2026-10-30T12:00:00Z",
        "senderId": "an-xrpl-client",
        "channelId": channel,
        "maxClaimableAmount": drops.to_string(),
        "signature": hex::encode(signature),
    })
    .to_string()
}

fn channel_bytes(id: &str) -> [u8; 32] {
    hex::decode(id).expect("hex").try_into().expect("32 bytes")
}

/// `build` refused, by the variant `matches` names.
async fn refused(config: &connector_config::Config) -> RuntimeError {
    match connector_cli::build(config).await {
        Ok(_) => panic!("the node booted, but its config should have been refused"),
        Err(error) => error,
    }
}

const CHANNEL: &str = "5db01b7ffed6b67e6b0414ded11e051d2ee2b7619ce0eaa6286d67a3a4d5bdb3";

// ---------------------------------------------------------------------------
// What the node publishes.
// ---------------------------------------------------------------------------

/// ADR 0087 decision 9: one `batch-settlement` entry per XRPL table, with the
/// network, the asset, `payTo` and the binding in `extra`, and no facilitator.
/// The self-description's `voucherSigners` carries the channel key.
#[tokio::test]
async fn a_node_boots_on_xrpl_and_publishes_it() {
    for algorithm in [XrplAlgorithm::Secp256k1, XrplAlgorithm::Ed25519] {
        let seed = seed(algorithm, 9);
        let node = signer(&seed);
        let ledger = Ledger::on(1000).with_account(&node.classic_address(), 50 * RESERVE_DROPS);
        let rpc = ledger.serve().await;
        let (app_addr, _recorded) = spawn_recording_app().await;
        let files = Files::new();
        let key = key_file(&seed);
        let config = files.config(&table(1000, &rpc.url(), key.path(), ""), &app_addr);

        let runtime = connector_cli::build(&config).await.expect("build");
        let app = connector_cli::router(&runtime, &config).expect("router");
        let terms = greeting(&app).await;

        let accepts = terms["accepts"].as_array().expect("accepts[]");
        assert_eq!(accepts.len(), 1, "{terms}");
        assert_eq!(
            accepts[0],
            json!({
                "scheme": "batch-settlement",
                "network": "xrpl:1000",
                "amount": PRICE.to_string(),
                "asset": "xrpl:1000/slip44:144",
                "payTo": node.classic_address(),
                "maxTimeoutSeconds": 60,
                "extra": {
                    "binding": "xrpl-paychan-v1",
                    "minSettleDelay": ONE_DAY,
                    "redemptionSafetySeconds": 90,
                },
            }),
            "{algorithm:?}"
        );
        assert!(
            accepts[0]["extra"].get("facilitator").is_none(),
            "ADR 0076 stays EVM-only"
        );

        assert_eq!(
            runtime.batch_settlements,
            vec![X402BatchSettlementTerms::Xrpl(
                X402BatchSettlementXrplTerms {
                    network: "xrpl:1000".to_string(),
                    asset: "xrpl:1000/slip44:144".to_string(),
                    pay_to: node.classic_address(),
                    binding: "xrpl-paychan-v1".to_string(),
                    min_settle_delay_secs: ONE_DAY,
                    redemption_safety_secs: 90,
                }
            )]
        );
        // `voucherSigners`: the 33-byte channel key, in hex (decision 2).
        assert_eq!(runtime.voucher_signers.len(), 1);
        assert_eq!(runtime.voucher_signers[0].network, "xrpl:1000");
        assert_eq!(
            runtime.voucher_signers[0].signer,
            hex::encode(node.public_key()),
            "{algorithm:?}"
        );
    }
}

/// The self-description a node answers `GET /ilp` with lists the entry and the
/// signer, parsed back as the typed facts.
#[tokio::test]
async fn the_self_description_carries_the_xrpl_entry_and_signer() {
    let seed = seed(XrplAlgorithm::Secp256k1, 9);
    let node = signer(&seed);
    let ledger = Ledger::on(1).with_account(&node.classic_address(), 50 * RESERVE_DROPS);
    let rpc = ledger.serve().await;
    let (app_addr, _recorded) = spawn_recording_app().await;
    let files = Files::new();
    let key = key_file(&seed);
    let config = files.config(&table(1, &rpc.url(), key.path(), ""), &app_addr);

    let runtime = connector_cli::build(&config).await.expect("build");
    let app = connector_cli::router(&runtime, &config).expect("router");
    let request = Request::builder()
        .method("GET")
        .uri("/ilp")
        .body(Body::empty())
        .expect("a request");
    let response = app.oneshot(request).await.expect("an answer");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = hyper::body::to_bytes(response.into_body())
        .await
        .expect("the body");
    let document: Value = serde_json::from_slice(&bytes).expect("JSON");

    assert_eq!(
        document["voucherSigners"],
        json!([{ "network": "xrpl:1", "signer": hex::encode(node.public_key()) }]),
        "{document}"
    );
    let entry = &document["batchSettlements"][0];
    assert_eq!(entry["network"], "xrpl:1", "{document}");
    assert_eq!(entry["asset"], "xrpl:1/slip44:144", "{document}");
    assert_eq!(entry["payTo"], node.classic_address(), "{document}");
    assert_eq!(entry["binding"], "xrpl-paychan-v1", "{document}");
    assert_eq!(entry["minSettleDelay"], ONE_DAY, "{document}");
    assert_eq!(entry["redemptionSafetySeconds"], 90, "{document}");
}

// ---------------------------------------------------------------------------
// What the node refuses to boot on (ADR 0087 decisions 4 and 5).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn boot_refuses_a_table_whose_key_is_not_the_ledgers_network_id() {
    let seed = seed(XrplAlgorithm::Secp256k1, 9);
    let node = signer(&seed);
    // The endpoint serves network 1; the table says 0, then 1000.
    let ledger = Ledger::on(1).with_account(&node.classic_address(), 50 * RESERVE_DROPS);
    let rpc = ledger.serve().await;
    let (app_addr, _recorded) = spawn_recording_app().await;
    let key = key_file(&seed);
    for claimed in [0, 1000] {
        let files = Files::new();
        let config = files.config(&table(claimed, &rpc.url(), key.path(), ""), &app_addr);
        match refused(&config).await {
            error @ RuntimeError::XrplNetworkMismatch { .. } => {
                let RuntimeError::XrplNetworkMismatch {
                    ref table,
                    expected,
                    reported,
                } = error
                else {
                    unreachable!()
                };
                assert_eq!(table, &format!("xrpl.{claimed}"));
                assert_eq!((expected, reported), (claimed, Some(1)));
                let message = error.to_string();
                assert!(
                    message.contains(&format!("network {claimed}"))
                        && message.contains("network 1"),
                    "{message}"
                );
            }
            other => panic!("expected a network mismatch, got {other}"),
        }
    }

    // A node that reports no network_id is mainnet's, and is refused by a
    // table for any other network.
    let mut silent = Ledger::on(0).with_account(&node.classic_address(), 50 * RESERVE_DROPS);
    silent.network_id = None;
    let rpc = silent.serve().await;
    let files = Files::new();
    let config = files.config(&table(1, &rpc.url(), key.path(), ""), &app_addr);
    assert!(matches!(
        refused(&config).await,
        RuntimeError::XrplNetworkMismatch {
            expected: 1,
            reported: None,
            ..
        }
    ));
}

/// An unknown network is refused before anything is dialed: at config load,
/// by name (ADR 0087 decision 5, ADR 0085's table).
#[test]
fn config_load_refuses_a_network_the_binary_has_no_row_for() {
    let files = Files::new();
    let key = key_file(&seed(XrplAlgorithm::Secp256k1, 9));
    let text = files.text(
        &table(7, "http://127.0.0.1:5005", key.path(), ""),
        "127.0.0.1:1",
    );
    let error = load_error(&text);
    assert!(
        error.contains("[settlement.xrpl.7] is not a network this build knows"),
        "{error}"
    );
}

#[tokio::test]
async fn boot_refuses_an_account_the_ledger_does_not_hold() {
    let seed = seed(XrplAlgorithm::Secp256k1, 9);
    let node = signer(&seed);
    // The ledger holds an account, just not this node's.
    let ledger = Ledger::on(1000).with_account("rSomeoneElse", 50 * RESERVE_DROPS);
    let rpc = ledger.serve().await;
    let (app_addr, _recorded) = spawn_recording_app().await;
    let files = Files::new();
    let key = key_file(&seed);
    let config = files.config(&table(1000, &rpc.url(), key.path(), ""), &app_addr);

    match refused(&config).await {
        RuntimeError::XrplAccountMissing { table, address } => {
            assert_eq!(table, "xrpl.1000");
            assert_eq!(address, node.classic_address());
        }
        other => panic!("expected a missing account, got {other}"),
    }
}

#[tokio::test]
async fn boot_refuses_an_account_below_the_base_reserve() {
    let seed = seed(XrplAlgorithm::Secp256k1, 9);
    let node = signer(&seed);
    let ledger = Ledger::on(1000).with_account(&node.classic_address(), RESERVE_DROPS - 1);
    let rpc = ledger.serve().await;
    let (app_addr, _recorded) = spawn_recording_app().await;
    let files = Files::new();
    let key = key_file(&seed);
    let config = files.config(&table(1000, &rpc.url(), key.path(), ""), &app_addr);

    match refused(&config).await {
        RuntimeError::XrplAccountUnderfunded {
            table,
            address,
            balance_drops,
            reserve_drops,
        } => {
            assert_eq!(table, "xrpl.1000");
            assert_eq!(address, node.classic_address());
            assert_eq!(
                (balance_drops, reserve_drops),
                (RESERVE_DROPS - 1, RESERVE_DROPS)
            );
        }
        other => panic!("expected an underfunded account, got {other}"),
    }

    // At the reserve, exactly, the account is funded.
    ledger
        .accounts
        .lock()
        .unwrap()
        .insert(node.classic_address(), RESERVE_DROPS);
    connector_cli::build(&config).await.expect("at the reserve");
}

/// An endpoint with no validated ledger reports no base reserve, so the
/// account's funding cannot be checked: refused, never assumed funded.
#[tokio::test]
async fn boot_refuses_an_endpoint_that_reports_no_base_reserve() {
    let seed = seed(XrplAlgorithm::Secp256k1, 9);
    let node = signer(&seed);
    let ledger = Ledger::on(1000)
        .with_account(&node.classic_address(), RESERVE_DROPS * 10)
        .syncing();
    let rpc = ledger.serve().await;
    let (app_addr, _recorded) = spawn_recording_app().await;
    let files = Files::new();
    let key = key_file(&seed);
    let config = files.config(&table(1000, &rpc.url(), key.path(), ""), &app_addr);

    match refused(&config).await {
        RuntimeError::XrplReserveUnreported { table } => assert_eq!(table, "xrpl.1000"),
        other => panic!("expected an unreported reserve, got {other}"),
    }
}

/// ADR 0087 decision 4: two tables on one account are refused by name, before
/// either ledger is dialed -- so neither endpoint here need exist.
#[tokio::test]
async fn boot_refuses_two_tables_on_one_account() {
    let seed = seed(XrplAlgorithm::Ed25519, 4);
    let (app_addr, _recorded) = spawn_recording_app().await;
    let files = Files::new();
    let first = key_file(&seed);
    // A different file, the same key: the account is what is compared.
    let second = key_file(&format!("  {seed}\n"));
    let tables = format!(
        "{}{}",
        table(1, "http://127.0.0.1:9", first.path(), ""),
        table(1000, "http://127.0.0.1:9", second.path(), ""),
    );
    let config = files.config(&tables, &app_addr);

    match refused(&config).await {
        error @ RuntimeError::XrplAccountOnTwoNetworks { .. } => {
            let RuntimeError::XrplAccountOnTwoNetworks { tables, address } = &error else {
                unreachable!()
            };
            assert_eq!(tables, &["xrpl.1".to_string(), "xrpl.1000".to_string()]);
            assert_eq!(address, &signer(&seed).classic_address());
            assert!(
                error
                    .to_string()
                    .contains("[settlement.xrpl.1] and [settlement.xrpl.1000]"),
                "{error}"
            );
        }
        other => panic!("expected one account on two networks, got {other}"),
    }

    // Two tables, two accounts, two ledgers: no refusal.
    let other_seed = self::seed(XrplAlgorithm::Secp256k1, 5);
    let one = Ledger::on(1)
        .with_account(&signer(&seed).classic_address(), 2 * RESERVE_DROPS)
        .serve()
        .await;
    let two = Ledger::on(1000)
        .with_account(&signer(&other_seed).classic_address(), 2 * RESERVE_DROPS)
        .serve()
        .await;
    let second = key_file(&other_seed);
    let tables = format!(
        "{}{}",
        table(1, &one.url(), first.path(), ""),
        table(1000, &two.url(), second.path(), ""),
    );
    let runtime = connector_cli::build(&files.config(&tables, &app_addr))
        .await
        .expect("two accounts on two networks");
    assert_eq!(
        runtime
            .voucher_signers
            .iter()
            .map(|fact| fact.network.as_str())
            .collect::<Vec<_>>(),
        ["xrpl:1", "xrpl:1000"]
    );
}

#[tokio::test]
async fn boot_refuses_a_key_file_that_is_no_family_seed() {
    let (app_addr, _recorded) = spawn_recording_app().await;
    let files = Files::new();
    let key = key_file("not a seed");
    let config = files.config(
        &table(1000, "http://127.0.0.1:9", key.path(), ""),
        &app_addr,
    );
    match refused(&config).await {
        RuntimeError::XrplKeyUnusable { table, message } => {
            assert_eq!(table, "xrpl.1000");
            assert!(message.contains("family seed"), "{message}");
        }
        other => panic!("expected an unusable key, got {other}"),
    }
}

// ---------------------------------------------------------------------------
// What config load refuses (ADR 0087 decisions 5 and 7), each by name.
// ---------------------------------------------------------------------------

fn load_error(text: &str) -> String {
    let mut file = tempfile::NamedTempFile::new().expect("temp config file");
    file.write_all(text.as_bytes()).expect("write config");
    match connector_config::Config::load(file.path()) {
        Ok(_) => panic!("the config loaded, but should have been refused"),
        Err(error) => error.to_string(),
    }
}

#[test]
fn config_load_refuses_a_bare_table_an_unknown_key_and_an_inline_key() {
    let files = Files::new();
    let key = key_file(&seed(XrplAlgorithm::Secp256k1, 9));
    let node = |settlement: &str| files.text(settlement, "127.0.0.1:1");

    let bare = load_error(&node(&format!(
        r#"
[settlement.xrpl]
rpc_url = "http://127.0.0.1:5005"

[settlement.xrpl.key]
key_file = "{}"
"#,
        key.path().display()
    )));
    assert!(
        bare.contains("[settlement.xrpl.<id>]") && bare.contains("one table per network"),
        "{bare}"
    );

    let unknown = load_error(&node(&table(
        1000,
        "http://127.0.0.1:5005",
        key.path(),
        "network_id = 1000",
    )));
    assert!(unknown.contains("network_id"), "{unknown}");

    let inline = load_error(&node(
        r#"
[settlement.xrpl.1000]
rpc_url = "http://127.0.0.1:5005"
key = "snoPBrXtMeMyMHUVTgbuqAfg1SUTb"
"#,
    ));
    assert!(
        inline.contains("writes its key inline") && inline.contains("key_file"),
        "{inline}"
    );
}

/// ADR 0087 decision 7: XRP is `xrpl:<id>/slip44:144` everywhere, and every
/// `<chain>:native` spelling is refused by name -- in `[[tokens]]` and in
/// `[[rates]]` alike.
#[test]
fn config_load_refuses_every_native_spelling() {
    let files = Files::new();
    let key = key_file(&seed(XrplAlgorithm::Secp256k1, 9));
    let tables = table(1000, "http://127.0.0.1:5005", key.path(), "");
    for native in [
        "xrpl:native",
        "eip155:1:native",
        "solana:native",
        "mina:native",
    ] {
        let text = format!(
            r#"{}
[[tokens]]
asset = "{native}"
numeraire = true
"#,
            files.text(&tables, "127.0.0.1:1")
        );
        let error = load_error(&text);
        assert!(
            error.contains(native) && error.contains("CAIP-19"),
            "{native}: {error}"
        );

        let text = format!(
            "{}\n[rate_guards]\nspread = {{ numerator = 1, denominator = 100 }}\nttl_secs = 300\n\
             max_move = {{ numerator = 5, denominator = 100 }}\n\n[[tokens]]\n\
             asset = \"xrpl:1000/slip44:144\"\nnumeraire = true\n\n[[rates]]\n\
             from = \"{native}\"\nto = \"xrpl:1000/slip44:144\"\n\
             rate = {{ numerator = 1, denominator = 1 }}\n",
            files.text(&tables, "127.0.0.1:1")
        );
        let error = load_error(&text);
        assert!(
            error.contains(native) && error.contains("CAIP-19"),
            "[[rates]] {native}: {error}"
        );
    }
}

/// And the one spelling that is read: a node that deals XRP declares
/// `xrpl:<id>/slip44:144`, which needs the table for its network.
#[test]
fn a_node_may_deal_xrp_only_on_a_network_it_settles_on() {
    let files = Files::new();
    let key = key_file(&seed(XrplAlgorithm::Secp256k1, 9));
    let tokens = |asset: &str| {
        format!(
            r#"
[[tokens]]
asset = "{asset}"
numeraire = true
"#
        )
    };
    let with_table = format!(
        "{}{}",
        files.text(
            &table(1000, "http://127.0.0.1:5005", key.path(), ""),
            "127.0.0.1:1"
        ),
        tokens("xrpl:1000/slip44:144")
    );
    let mut file = tempfile::NamedTempFile::new().expect("temp config file");
    file.write_all(with_table.as_bytes()).expect("write");
    let config = connector_config::Config::load(file.path()).expect("XRP on a settled network");
    assert_eq!(config.denomination().tokens().len(), 1);

    let elsewhere = format!(
        "{}{}",
        files.text(
            &table(1000, "http://127.0.0.1:5005", key.path(), ""),
            "127.0.0.1:1"
        ),
        tokens("xrpl:1/slip44:144")
    );
    let error = load_error(&elsewhere);
    assert!(
        error.contains("xrpl:1/slip44:144") && error.contains("[settlement.xrpl.1]"),
        "{error}"
    );
}

/// ADR 0087 decision 6: every XRPL leg is a dealing boundary, so a node that
/// deals at all must say which XRP each of its tables holds -- a channel on a
/// table whose XRP is undeclared has no denomination to convert from.
#[test]
fn a_dealing_node_must_declare_the_xrp_each_xrpl_table_holds() {
    let files = Files::new();
    let first = key_file(&seed(XrplAlgorithm::Secp256k1, 1));
    let second = key_file(&seed(XrplAlgorithm::Secp256k1, 2));
    let tables = format!(
        "{}{}",
        table(1, "http://127.0.0.1:5005", first.path(), ""),
        table(1000, "http://127.0.0.1:5006", second.path(), "")
    );
    let tokens = |declared: &[&str]| {
        declared
            .iter()
            .enumerate()
            .map(|(i, asset)| {
                format!(
                    "\n[[tokens]]\nasset = \"{asset}\"\nnumeraire = {}\n",
                    i == 0
                )
            })
            .collect::<String>()
    };
    let text =
        |declared: &[&str]| format!("{}{}", files.text(&tables, "127.0.0.1:1"), tokens(declared));

    let error = load_error(&text(&["xrpl:1/slip44:144"]));
    assert!(
        error.contains("xrpl:1000/slip44:144") && error.contains("[settlement.xrpl.<id>]"),
        "{error}"
    );

    // Both declared, and the second given a static rate (XRPL has no rate
    // source, ADR 0087 decision 6): the node loads.
    let both = format!(
        "{}\n[rate_guards]\nspread = {{ numerator = 1, denominator = 100 }}\nttl_secs = 300\n\
         max_move = {{ numerator = 5, denominator = 100 }}\n\n[[rates]]\n\
         from = \"xrpl:1000/slip44:144\"\nto = \"xrpl:1/slip44:144\"\n\
         rate = {{ numerator = 1, denominator = 1 }}\n",
        text(&["xrpl:1/slip44:144", "xrpl:1000/slip44:144"])
    );
    let mut file = tempfile::NamedTempFile::new().expect("temp config file");
    file.write_all(both.as_bytes()).expect("write");
    let loaded = connector_config::Config::load(file.path());
    assert!(loaded.is_ok(), "{:?}", loaded.err().map(|e| e.to_string()));
}

// ---------------------------------------------------------------------------
// Vouchers: admitted, journaled, listed, and restored (ADR 0087 decision 2).
// ---------------------------------------------------------------------------

/// An admitted XRPL channel appears in `GET /channels` with its network,
/// direction, collateral and landed amount, and in `GET /claims`; and it
/// survives a restart at its watermark.
#[tokio::test]
async fn an_admitted_xrpl_channel_is_listed_and_survives_a_restart_at_its_watermark() {
    let seed = seed(XrplAlgorithm::Secp256k1, 9);
    let node = signer(&seed);
    let payer = signer(&self::seed(XrplAlgorithm::Ed25519, 3));
    let ledger = Ledger::on(1000).with_account(&node.classic_address(), 50 * RESERVE_DROPS);
    ledger.open_channel(CHANNEL, &node.classic_address(), &payer, 10_000, json!({}));
    let rpc = ledger.serve().await;
    let (app_addr, recorded) = spawn_recording_app().await;
    let files = Files::new();
    let key = key_file(&seed);
    let config = files.config(&table(1000, &rpc.url(), key.path(), ""), &app_addr);

    let runtime = connector_cli::build(&config).await.expect("build");
    let app = connector_cli::router(&runtime, &config).expect("router");
    let receiver = runtime.signer.public_key().expect("the node's wrap key");
    let id = channel_bytes(CHANNEL);

    // Paid with 300 drops on the channel: it is admitted on its first voucher.
    let claim = voucher(
        "xrpl:1000",
        CHANNEL,
        300,
        &payer.sign_claim(&id, 300).expect("a claim"),
    );
    let answer = post_ilp(&app, &claim, paid_prepare(ROUTE, &receiver)).await;
    Fulfill::decode(&answer).expect("the paid write is fulfilled");
    assert_eq!(recorded.lock().unwrap().len(), 1);

    let channels = read(&app, "/channels").await;
    let views = channels.as_array().expect("a list");
    let xrpl: Vec<&Value> = views
        .iter()
        .filter(|view| view["chain"] == "xrpl")
        .collect();
    assert_eq!(xrpl.len(), 1, "{channels}");
    let view = xrpl[0];
    assert_eq!(view["id"], CHANNEL, "{view}");
    assert_eq!(view["network"], "xrpl:1000", "{view}");
    assert_eq!(view["direction"], "inbound", "{view}");
    assert_eq!(view["collateral"], 10_000, "{view}");
    assert_eq!(view["landed"], 0, "{view}");
    assert_eq!(view["watermark"], 300, "{view}");
    assert_eq!(view["scheme"], "batch-settlement", "{view}");
    assert_eq!(
        view["counterparty"],
        hex::encode(payer.public_key()),
        "{view}"
    );

    let claims = read(&app, "/claims").await;
    let claim = claims
        .as_array()
        .expect("a list")
        .iter()
        .find(|claim| claim["channel_id"] == format!("xrpl:{CHANNEL}"))
        .unwrap_or_else(|| panic!("the journal keys the channel xrpl:<channel id>: {claims}"));
    assert_eq!(claim["direction"], "inbound", "{claim}");
    assert_eq!(claim["cumulative_amount"], 300, "{claim}");
    assert_eq!(claim["scheme"], "batch-settlement", "{claim}");

    // A voucher that does not advance is refused; one signed by someone else
    // is refused as a bad signature.
    let stranger = signer(&self::seed(XrplAlgorithm::Secp256k1, 77));
    let forged = voucher(
        "xrpl:1000",
        CHANNEL,
        500,
        &stranger.sign_claim(&id, 500).expect("a claim"),
    );
    let answer = post_ilp(&app, &forged, paid_prepare(ROUTE, &receiver)).await;
    Reject::decode(&answer).expect("a forged voucher is rejected");
    let answer = post_ilp(
        &app,
        &voucher(
            "xrpl:1000",
            CHANNEL,
            200,
            &payer.sign_claim(&id, 200).expect("a claim"),
        ),
        paid_prepare(ROUTE, &receiver),
    )
    .await;
    Reject::decode(&answer).expect("a voucher below the watermark is rejected");

    // A voucher naming a network the node holds no table for names no channel.
    let elsewhere = voucher(
        "xrpl:1",
        CHANNEL,
        500,
        &payer.sign_claim(&id, 500).expect("a claim"),
    );
    let answer = post_ilp(&app, &elsewhere, paid_prepare(ROUTE, &receiver)).await;
    Reject::decode(&answer).expect("a voucher on another network is rejected");

    // Restart: the same files, a new process's worth of state.
    drop(app);
    drop(runtime);
    let runtime = connector_cli::build(&config).await.expect("rebuild");
    let app = connector_cli::router(&runtime, &config).expect("router");
    let channels = read(&app, "/channels").await;
    let view = channels
        .as_array()
        .expect("a list")
        .iter()
        .find(|view| view["chain"] == "xrpl")
        .unwrap_or_else(|| panic!("the channel survived the restart: {channels}"));
    assert_eq!(view["network"], "xrpl:1000", "{view}");
    assert_eq!(view["watermark"], 300, "{view}");
    assert_eq!(view["collateral"], 10_000, "{view}");

    // The restored watermark still holds: 300 is spent, and 300 again is not
    // an advance.
    let answer = post_ilp(
        &app,
        &voucher(
            "xrpl:1000",
            CHANNEL,
            300,
            &payer.sign_claim(&id, 300).expect("a claim"),
        ),
        paid_prepare(ROUTE, &runtime.signer.public_key().expect("key")),
    )
    .await;
    Reject::decode(&answer).expect("a replay of the restored watermark is rejected");
    let answer = post_ilp(
        &app,
        &voucher(
            "xrpl:1000",
            CHANNEL,
            450,
            &payer.sign_claim(&id, 450).expect("a claim"),
        ),
        paid_prepare(ROUTE, &runtime.signer.public_key().expect("key")),
    )
    .await;
    Fulfill::decode(&answer).expect("a later voucher advances from the restored watermark");
    let channels = read(&app, "/channels").await;
    assert_eq!(
        channels
            .as_array()
            .expect("a list")
            .iter()
            .find(|view| view["chain"] == "xrpl")
            .expect("the channel")["watermark"],
        450
    );
}

/// #1615, on a scripted ledger so it runs everywhere: the app is told the
/// chain and the network, a claim with no `network` or an unconfigured one is
/// refused by name, and a client that lost its store recovers the node's
/// watermark from `POST /ilp/claim-state` with an XRPL challenge.
#[tokio::test]
async fn a_client_pays_over_xrpl_and_recovers_its_watermark_from_the_node() {
    let seed = seed(XrplAlgorithm::Secp256k1, 9);
    let node = signer(&seed);
    let payer = signer(&self::seed(XrplAlgorithm::Ed25519, 3));
    let ledger = Ledger::on(1000).with_account(&node.classic_address(), 50 * RESERVE_DROPS);
    ledger.open_channel(CHANNEL, &node.classic_address(), &payer, 10_000, json!({}));
    let rpc = ledger.serve().await;
    let (app_addr, recorded) = spawn_recording_app().await;
    let files = Files::new();
    let key = key_file(&seed);
    let config = files.config(&table(1000, &rpc.url(), key.path(), ""), &app_addr);
    let runtime = connector_cli::build(&config).await.expect("build");
    let app = connector_cli::router(&runtime, &config).expect("router");
    let receiver = runtime.signer.public_key().expect("the node's wrap key");
    let id = channel_bytes(CHANNEL);
    let claim = |network: Option<&str>, drops: u64| {
        let mut json: Value = serde_json::from_str(&voucher(
            "xrpl:1000",
            CHANNEL,
            drops,
            &payer.sign_claim(&id, u128::from(drops)).expect("a claim"),
        ))
        .expect("JSON");
        match network {
            Some(network) => json["network"] = json!(network),
            None => {
                json.as_object_mut().expect("an object").remove("network");
            }
        }
        json.to_string()
    };
    let refusal = |bytes: axum::body::Bytes| Reject::decode(&bytes).expect("refused").message;

    let answer = post_ilp(
        &app,
        &claim(Some("xrpl:1000"), 300),
        paid_prepare(ROUTE, &receiver),
    )
    .await;
    Fulfill::decode(&answer).expect("the paid write is fulfilled");
    {
        let recorded = recorded.lock().unwrap();
        let write = &recorded[0];
        assert_eq!(write.header("x-toon-chain"), Some("xrpl"));
        assert_eq!(write.header("x-toon-network"), Some("xrpl:1000"));
        assert_eq!(
            write.header("x-toon-payer"),
            Some(format!("xrpl:{CHANNEL}").as_str())
        );
    }

    let message = refusal(post_ilp(&app, &claim(None, 400), paid_prepare(ROUTE, &receiver)).await);
    assert!(message.contains("'network'"), "{message}");
    let message = refusal(
        post_ilp(
            &app,
            &claim(Some("xrpl:1"), 400),
            paid_prepare(ROUTE, &receiver),
        )
        .await,
    );
    assert!(
        message.contains("xrpl:1") && message.contains("settles"),
        "{message}"
    );

    let at = refusal(
        post_ilp(
            &app,
            &claim(Some("xrpl:1000"), 300),
            paid_prepare(ROUTE, &receiver),
        )
        .await,
    );
    let over = refusal(
        post_ilp(
            &app,
            &claim(Some("xrpl:1000"), 10_001),
            paid_prepare(ROUTE, &receiver),
        )
        .await,
    );
    assert!(at.contains("advances value by 0"), "{at}");
    assert!(over.contains("could never be landed"), "{over}");

    // The client lost its store: the node answers with its watermark.
    let state = |entry: Value| {
        let app = app.clone();
        async move {
            let request = Request::builder()
                .method("POST")
                .uri("/ilp/claim-state")
                .header("content-type", "application/json")
                .body(Body::from(json!({ "channels": [entry] }).to_string()))
                .expect("a request");
            let response = app.oneshot(request).await.expect("an answer");
            assert_eq!(response.status(), StatusCode::OK);
            let bytes = hyper::body::to_bytes(response.into_body())
                .await
                .expect("the body");
            serde_json::from_slice::<Value>(&bytes).expect("JSON")["channels"][0].clone()
        }
    };
    let expires = chrono::Utc::now().timestamp() as u64 + 300;
    let entry = |who: &XrplSigner, network: &str| {
        json!({
            "blockchain": "xrpl",
            "scheme": "batch-settlement",
            "network": network,
            "channelId": CHANNEL,
            "expires": expires,
            "signature": hex::encode(
                who.sign_claim_state_challenge(network, &id, expires).expect("a challenge"),
            ),
        })
    };
    let restored = state(entry(&payer, "xrpl:1000")).await;
    assert_eq!(restored["ok"], true, "{restored}");
    assert_eq!(restored["network"], "xrpl:1000", "{restored}");
    assert_eq!(restored["cumulativeClaimed"], "300", "{restored}");
    assert_eq!(restored["maxCumulative"], "10000", "{restored}");
    assert_eq!(restored["available"], "9700", "{restored}");
    let stranger = signer(&self::seed(XrplAlgorithm::Secp256k1, 77));
    let refused = state(entry(&stranger, "xrpl:1000")).await;
    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(refused["error"], "unverified", "{refused}");
    // A challenge for one network is no proof on another.
    let mut wrong = entry(&payer, "xrpl:1");
    wrong["network"] = json!("xrpl:1000");
    let wrong = state(wrong).await;
    assert_eq!(wrong["ok"], false, "{wrong}");
    assert_eq!(wrong["error"], "unverified", "{wrong}");
    let elsewhere = state(entry(&payer, "xrpl:1")).await;
    assert_eq!(elsewhere["error"], "network-not-settled", "{elsewhere}");
    let mut unnamed = entry(&payer, "xrpl:1000");
    unnamed
        .as_object_mut()
        .expect("an object")
        .remove("network");
    assert_eq!(state(unnamed).await["error"], "network-missing");
}

/// A channel the node would not admit is not accepted a voucher on (ADR 0087
/// decision 9): here, one paying someone else and one with a `CancelAfter`.
#[tokio::test]
async fn a_channel_breaking_an_admission_rule_takes_no_voucher() {
    let seed = seed(XrplAlgorithm::Secp256k1, 9);
    let node = signer(&seed);
    let payer = signer(&self::seed(XrplAlgorithm::Ed25519, 3));
    let ledger = Ledger::on(1000).with_account(&node.classic_address(), 50 * RESERVE_DROPS);
    let to_someone_else = "11".repeat(32);
    let cancels = "22".repeat(32);
    ledger.open_channel(&to_someone_else, "rSomeoneElse", &payer, 10_000, json!({}));
    ledger.open_channel(
        &cancels,
        &node.classic_address(),
        &payer,
        10_000,
        json!({ "CancelAfter": 999_999_999u64 }),
    );
    let rpc = ledger.serve().await;
    let (app_addr, recorded) = spawn_recording_app().await;
    let files = Files::new();
    let key = key_file(&seed);
    let config = files.config(&table(1000, &rpc.url(), key.path(), ""), &app_addr);
    let runtime = connector_cli::build(&config).await.expect("build");
    let app = connector_cli::router(&runtime, &config).expect("router");
    let receiver = runtime.signer.public_key().expect("the node's wrap key");

    for channel in [&to_someone_else, &cancels] {
        let claim = voucher(
            "xrpl:1000",
            channel,
            300,
            &payer
                .sign_claim(&channel_bytes(channel), 300)
                .expect("a claim"),
        );
        let answer = post_ilp(&app, &claim, paid_prepare(ROUTE, &receiver)).await;
        Reject::decode(&answer).expect("rejected");
    }
    assert!(recorded.lock().unwrap().is_empty(), "no write was carried");
}

// ---------------------------------------------------------------------------
// ADR 0073: the RPC rides its own pinned circuit.
// ---------------------------------------------------------------------------

/// With `rpc_via_socks_proxy`, every XRPL read goes through the node's one
/// `socks_proxy`, as a name, on a circuit of this network's own -- and a
/// second XRPL network is on another. The ledgers' hosts are onion names
/// nothing here resolves, so a read that arrived at all was proxied.
#[tokio::test]
async fn with_rpc_via_socks_proxy_each_xrpl_rpc_rides_its_own_pinned_circuit() {
    use connector_runtime::Socks5TestServer;

    const HOST_ONE: &str = "xrplnetworkonexrplnetworkonexrplnetworkonexrplnetworkone.onion";
    const HOST_TWO: &str = "xrplnetworktwoxrplnetworktwoxrplnetworktwoxrplnetworktwo.onion";
    let (seed_one, seed_two) = (
        seed(XrplAlgorithm::Secp256k1, 1),
        seed(XrplAlgorithm::Ed25519, 2),
    );
    let one = Ledger::on(1)
        .with_account(&signer(&seed_one).classic_address(), 2 * RESERVE_DROPS)
        .serve()
        .await;
    let two = Ledger::on(1000)
        .with_account(&signer(&seed_two).classic_address(), 2 * RESERVE_DROPS)
        .serve()
        .await;
    let proxy = Socks5TestServer::spawn(HashMap::from([
        (format!("{HOST_ONE}:80"), one.addr()),
        (format!("{HOST_TWO}:80"), two.addr()),
    ]))
    .await;
    let (app_addr, _recorded) = spawn_recording_app().await;
    let files = Files::new();
    let (key_one, key_two) = (key_file(&seed_one), key_file(&seed_two));
    let tables = format!(
        "{}{}",
        table(
            1,
            &format!("http://{HOST_ONE}/"),
            key_one.path(),
            "rpc_via_socks_proxy = true"
        ),
        table(
            1000,
            &format!("http://{HOST_TWO}/"),
            key_two.path(),
            "rpc_via_socks_proxy = true"
        ),
    );
    let text = format!(
        "socks_proxy = \"{}\"\n{}",
        proxy.proxy_url(),
        files.text(&tables, &app_addr)
    );
    let config = support::load_config(&text);

    connector_cli::build(&config)
        .await
        .expect("both ledgers read through the proxy");

    let connects = proxy.connects();
    for (host, circuit) in [
        (HOST_ONE, "toon-settlement-xrpl-1"),
        (HOST_TWO, "toon-settlement-xrpl-1000"),
    ] {
        let dials: Vec<_> = connects
            .iter()
            .filter(|c| c.target == format!("{host}:80"))
            .collect();
        assert!(!dials.is_empty(), "{host} was dialed: {connects:?}");
        assert!(
            dials.iter().all(|c| c.username.as_deref() == Some(circuit)),
            "every dial to {host} is on circuit {circuit}: {connects:?}"
        );
    }
    assert!(
        connects.iter().all(|c| c.username.is_some()),
        "no XRPL dial went through the proxy unpinned: {connects:?}"
    );
    // Fail closed: with the proxy gone, boot is refused and nothing reaches
    // the ledger.
    let dead = std::net::TcpListener::bind("127.0.0.1:0").expect("a free port");
    let dead_url = format!("socks5h://{}", dead.local_addr().expect("addr"));
    drop(dead);
    let direct = format!(
        "socks_proxy = \"{dead_url}\"\n{}",
        files.text(
            &table(
                1,
                &format!("http://{HOST_ONE}/"),
                key_one.path(),
                "rpc_via_socks_proxy = true"
            ),
            &app_addr
        )
    );
    let calls_before = one.calls().len();
    connector_cli::build(&support::load_config(&direct))
        .await
        .err()
        .expect("a proxied table with no proxy to ride is refused");
    assert_eq!(
        one.calls().len(),
        calls_before,
        "nothing reached the ledger without the proxy"
    );

    // Config load already refuses the key without a proxy to ride.
    let without = text.replace(&format!("socks_proxy = \"{}\"\n", proxy.proxy_url()), "");
    assert!(
        load_error(&without).contains("rpc_via_socks_proxy"),
        "a proxied table needs a socks_proxy"
    );
}

// ---------------------------------------------------------------------------
// Against a spawned `xrpld` (ADR 0084).
// ---------------------------------------------------------------------------

/// The real runtime on a real stand-alone ledger: boot, the greeting, the
/// refusals that need a ledger to be wrong in, and a voucher on a real
/// `PayChannel`, listed, restarted and landed.
#[tokio::test]
async fn a_node_boots_on_a_spawned_xrpld_and_serves_a_real_channel() {
    if !require_xrpld() {
        return;
    }
    let chain = Xrpld::spawn().await;
    let rpc = chain.rpc();
    let _closer: LedgerCloser = chain.close_ledgers_every(Duration::from_millis(200));
    let node = chain.funded_wallet(&rpc, 100_000_000).await;
    let payer = chain.funded_wallet(&rpc, 100_000_000_000).await;
    let payer_signer = signer(&payer.seed);
    let (app_addr, _recorded) = spawn_recording_app().await;
    let files = Files::new();
    let key = key_file(&node.seed);

    // The wrong network id, and an account the ledger does not hold.
    let wrong = files.config(&table(1, &chain.rpc_url, key.path(), ""), &app_addr);
    assert!(matches!(
        refused(&wrong).await,
        RuntimeError::XrplNetworkMismatch {
            expected: 1,
            reported: Some(1000),
            ..
        }
    ));
    let (_, unfunded_seed) = rpc.wallet_propose().await.expect("wallet_propose");
    let unfunded = key_file(&unfunded_seed);
    let missing = files.config(
        &table(STANDALONE_NETWORK_ID, &chain.rpc_url, unfunded.path(), ""),
        &app_addr,
    );
    assert!(matches!(
        refused(&missing).await,
        RuntimeError::XrplAccountMissing { .. }
    ));

    // The node boots, and its greeting carries the XRPL entry.
    let config = files.config(
        &table(STANDALONE_NETWORK_ID, &chain.rpc_url, key.path(), ""),
        &app_addr,
    );
    let runtime = connector_cli::build(&config).await.expect("build");
    let app = connector_cli::router(&runtime, &config).expect("router");
    let terms = greeting(&app).await;
    let entry = &terms["accepts"][0];
    assert_eq!(entry["network"], "xrpl:1000", "{terms}");
    assert_eq!(entry["asset"], "xrpl:1000/slip44:144", "{terms}");
    assert_eq!(entry["payTo"], node.address, "{terms}");
    assert_eq!(entry["extra"]["binding"], "xrpl-paychan-v1", "{terms}");
    assert_eq!(
        runtime.voucher_signers[0].signer,
        hex::encode(signer(&node.seed).public_key())
    );

    // A real channel from the payer, admitted on its first voucher.
    let create = json!({
        "TransactionType": "PaymentChannelCreate",
        "Account": payer.address,
        "Destination": node.address,
        "Amount": "5000000",
        "SettleDelay": ONE_DAY,
        "PublicKey": hex::encode_upper(payer_signer.public_key()),
    });
    let last = rpc.fee().await.expect("fee").ledger_current_index + 40;
    let mut create = create;
    create["LastLedgerSequence"] = json!(last);
    let submitted = rpc
        .sign_and_submit(create, &payer.seed)
        .await
        .expect("submit");
    assert_eq!(submitted.engine_result, "tesSUCCESS", "{submitted:?}");
    let meta = match rpc
        .wait_for_tx(
            &submitted.hash,
            last,
            connector_chain_rpc::xrpl::TxWait {
                poll: Duration::from_millis(50),
                give_up_after: Duration::from_secs(30),
            },
        )
        .await
        .expect("wait")
    {
        connector_chain_rpc::xrpl::TxFate::Landed { meta, .. } => meta,
        other => panic!("the channel was not created: {other:?}"),
    };
    let channel = meta["AffectedNodes"]
        .as_array()
        .expect("affected nodes")
        .iter()
        .find_map(|node| {
            let created = &node["CreatedNode"];
            (created["LedgerEntryType"] == "PayChannel").then(|| {
                created["LedgerIndex"]
                    .as_str()
                    .expect("index")
                    .to_lowercase()
            })
        })
        .expect("a channel was created");
    let receiver = runtime.signer.public_key().expect("the node's wrap key");
    let claim = voucher(
        "xrpl:1000",
        &channel,
        300,
        &payer_signer
            .sign_claim(&channel_bytes(&channel), 300)
            .expect("a claim"),
    );
    let answer = post_ilp(&app, &claim, paid_prepare(ROUTE, &receiver)).await;
    Fulfill::decode(&answer).expect("the paid write is fulfilled");

    let channels = read(&app, "/channels").await;
    let view = channels
        .as_array()
        .expect("a list")
        .iter()
        .find(|view| view["chain"] == "xrpl")
        .unwrap_or_else(|| panic!("the channel is listed: {channels}"));
    assert_eq!(view["id"], channel.as_str(), "{view}");
    assert_eq!(view["network"], "xrpl:1000", "{view}");
    assert_eq!(view["direction"], "inbound", "{view}");
    assert_eq!(view["collateral"], 5_000_000, "{view}");
    assert_eq!(view["landed"], 0, "{view}");
    assert_eq!(view["watermark"], 300, "{view}");

    // Restart: the channel is back at its watermark.
    drop(app);
    drop(runtime);
    let runtime = connector_cli::build(&config).await.expect("rebuild");
    let app = connector_cli::router(&runtime, &config).expect("router");
    let channels = read(&app, "/channels").await;
    let view = channels
        .as_array()
        .expect("a list")
        .iter()
        .find(|view| view["chain"] == "xrpl")
        .unwrap_or_else(|| panic!("the channel survived the restart: {channels}"));
    assert_eq!(view["watermark"], 300, "{view}");
    assert_eq!(view["network"], "xrpl:1000", "{view}");
}
