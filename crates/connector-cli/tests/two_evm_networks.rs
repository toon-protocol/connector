//! ADR 0086, built (#1601): one node settles on two EVM networks at once.
//!
//! Two disposable anvils, on chain ids 31337 and 31338, each with x402's
//! contracts placed at their canonical addresses and a FiatToken USDC
//! deployed by the same deployer -- so, as on every anvil `seed.sh` seeds and
//! as Circle does on Arc mainnet and testnet, the token sits at the **same
//! address** on both. What tells a payment on one from a payment on the
//! other is the network, and nothing else.
//!
//! The node is built from a config file with `[settlement.eip155.31337]` and
//! `[settlement.eip155.31338]`, both naming one key file (ADR 0086 decision
//! 5), through `connector_cli::build` and `router` -- the same calls the
//! binary makes. A missing `anvil` fails CI and skips locally
//! (`require_anvil`).

mod support;

use std::collections::HashMap;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use connector_cli::RuntimeError;
use connector_domain::{Fulfill, Network, Reject};
use connector_settlement::batch::contract::{
    assert_upholds_the_contract, BatchContractFixture, BoxFuture, ChannelTerms, OpenedChannel,
};
use connector_settlement::batch::{
    BatchChannelStatus, BatchSettlementBackend, ChannelPresentation, EvmChannelConfig, Voucher,
    VoucherSigner,
};
use connector_settlement::ChannelId;
use connector_settlement_evm::test_support::x402::X402Chain;
use connector_settlement_evm::test_support::{
    require_anvil, Anvil, COUNTERPARTY_PRIVATE_KEY, DEPLOYER_PRIVATE_KEY,
};
use ethers::signers::{LocalWallet, Signer};
use ethers::types::Address;
use tower::ServiceExt;

use support::{evm_voucher_on, paid_prepare, post_ilp, spawn_recording_app};

/// This binary's own base port for [`Anvil::spawn_with_chain_id`].
const ANVIL_BASE_PORT: u16 = 26_100;

const ROUTE: &str = "g.toon.two-networks";
const PRICE: u64 = 100;
const DEPOSIT: u128 = 1_000;
const ONE_DAY: u64 = 86_400;

/// One disposable EVM network: an anvil with x402 placed, USDC and a second
/// token deployed, and both minted to the client.
struct Net {
    /// Held so the chain outlives the test.
    _anvil: Anvil,
    rpc_url: String,
    x402: Arc<X402Chain>,
    token: Address,
    /// A token the node does not settle in, for the contract suite.
    other_token: Address,
    /// The table key this network is configured under.
    chain_id: u64,
    /// ERC-3009 nonce salts, distinct per deposit.
    next_salt: AtomicU64,
    configs: Mutex<HashMap<ChannelId, EvmChannelConfig>>,
}

impl Net {
    async fn spawn(chain_id: u64) -> Net {
        Self::spawn_reporting(chain_id, chain_id).await
    }

    /// A network configured as `chain_id` whose anvil reports `reported`.
    async fn spawn_reporting(chain_id: u64, reported: u64) -> Net {
        let anvil = Anvil::spawn_with_chain_id(ANVIL_BASE_PORT, reported).await;
        let mut x402 = X402Chain::place(&anvil.rpc_url).await;
        let token = x402.deploy_fiat_token().await;
        let other_token = x402.deploy_fiat_token().await;
        x402.fund_gas(payer().address()).await;
        x402.fund_gas(session().address()).await;
        for token in [token, other_token] {
            x402.mint(token, payer().address(), 1_000_000_000).await;
        }
        Net {
            rpc_url: anvil.rpc_url.clone(),
            _anvil: anvil,
            x402: Arc::new(x402),
            token,
            other_token,
            chain_id,
            next_salt: AtomicU64::new(1),
            configs: Mutex::new(HashMap::new()),
        }
    }

    fn network(&self) -> String {
        format!("eip155:{}", self.chain_id)
    }

    fn salt(&self) -> u64 {
        self.next_salt.fetch_add(1, Ordering::SeqCst)
    }

    fn table(&self, key_file: &std::path::Path, extra: &str) -> String {
        table(self.chain_id, &self.rpc_url, self.token, key_file, extra)
    }

    /// An admissible config paying `node`, as the client builds one.
    fn config(&self, node: [u8; 20]) -> EvmChannelConfig {
        let mut salt = [0u8; 32];
        salt[24..].copy_from_slice(&self.salt().to_be_bytes());
        EvmChannelConfig {
            payer: payer().address().to_fixed_bytes(),
            payer_authorizer: session().address().to_fixed_bytes(),
            receiver: node,
            receiver_authorizer: node,
            token: self.token.to_fixed_bytes(),
            withdraw_delay: ONE_DAY,
            salt,
        }
    }

    /// The client's first deposit into `config`'s channel, which opens it.
    async fn open(&self, config: EvmChannelConfig, deposit: u128) -> ChannelId {
        self.x402
            .deposit(&payer(), &config, deposit, self.salt())
            .await;
        let channel = self.x402.channel_id(&config);
        self.configs.lock().unwrap().insert(channel.clone(), config);
        channel
    }

    fn config_of(&self, channel: &ChannelId) -> EvmChannelConfig {
        self.configs.lock().unwrap()[channel].clone()
    }
}

/// A `[settlement.eip155.<chain_id>]` table.
fn table(
    chain_id: u64,
    rpc_url: &str,
    token: Address,
    key_file: &std::path::Path,
    extra: &str,
) -> String {
    format!(
        r#"
[settlement.eip155.{chain_id}]
rpc_url = "{rpc_url}"
token_address = "{token:?}"
decimals = 6
asset_eip712_name = "USDC"
asset_eip712_version = "2"
min_withdraw_delay_secs = {ONE_DAY}
{extra}

[settlement.eip155.{chain_id}.key]
key_file = "{key}"
"#,
        key = key_file.display(),
    )
}

/// The client's funding key.
fn payer() -> LocalWallet {
    LocalWallet::from_bytes(&[0x61; 32]).expect("key")
}

/// The client's session key, which signs its vouchers.
fn session() -> LocalWallet {
    LocalWallet::from_bytes(&[0x62; 32]).expect("key")
}

/// The node's settlement address: anvil's first account, on every network.
fn node_address() -> [u8; 20] {
    DEPLOYER_PRIVATE_KEY
        .parse::<LocalWallet>()
        .expect("the node's settlement key")
        .address()
        .to_fixed_bytes()
}

/// A node's files: its signer key, the ONE settlement key both EVM tables
/// name, and its state directory.
struct Files {
    signer_key: tempfile::NamedTempFile,
    settlement_key: tempfile::NamedTempFile,
    state_dir: tempfile::TempDir,
}

impl Files {
    fn new() -> Files {
        Self::with_settlement_key(DEPLOYER_PRIVATE_KEY)
    }

    /// A node settling as `settlement_key_hex` on every table.
    fn with_settlement_key(settlement_key_hex: &str) -> Files {
        Self::with_keys(settlement_key_hex, 0x07)
    }

    /// [`Self::with_settlement_key`], with its own signer seed: nodes on one
    /// path are distinct identities, so a packet sealed to one is opened by
    /// it alone.
    fn with_keys(settlement_key_hex: &str, signer_seed: u8) -> Files {
        let mut signer_key = tempfile::NamedTempFile::new().expect("signer key");
        signer_key.write_all(&[signer_seed; 32]).expect("write");
        let mut settlement_key = tempfile::NamedTempFile::new().expect("settlement key");
        settlement_key
            .write_all(settlement_key_hex.as_bytes())
            .expect("write");
        Files {
            signer_key,
            settlement_key,
            state_dir: tempfile::tempdir().expect("state dir"),
        }
    }

    /// A node settling on `tables` (each `(net, extra)`), with one paid
    /// route to `app_addr`.
    fn config(&self, tables: &[(&Net, &str)], app_addr: &str) -> connector_config::Config {
        let tables: String = tables
            .iter()
            .map(|(net, extra)| net.table(self.settlement_key.path(), extra))
            .collect();
        self.config_with(&tables, app_addr)
    }

    fn config_with(&self, tables: &str, app_addr: &str) -> connector_config::Config {
        support::load_config(&format!(
            r#"
client_edge_addr = "127.0.0.1:0"
state_dir = "{state_dir}"

[signer]
key_file = "{signer_key}"
{tables}
[[routes]]
prefix = "{ROUTE}"
handler_url = "http://{app_addr}"
price = {PRICE}
"#,
            state_dir = self.state_dir.path().display(),
            signer_key = self.signer_key.path().display(),
        ))
    }
}

/// The greeting an unpaid write to [`ROUTE`] is answered with.
async fn greeting(app: &axum::Router) -> serde_json::Value {
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

fn network(text: &str) -> Network {
    text.parse().expect("a CAIP-2 network")
}

/// ADR 0086 decision 6 and story 42: one `accepts[]` entry per EVM table,
/// each with its own network, asset, `payTo` and facilitator -- and none on
/// the table that names no facilitator (ADR 0076).
#[tokio::test]
async fn the_greeting_offers_one_entry_per_evm_network() {
    if !require_anvil() {
        return;
    }
    let base = Net::spawn(31_337).await;
    let other = Net::spawn(31_338).await;
    let (app_addr, _recorded) = spawn_recording_app().await;
    let files = Files::new();
    let config = files.config(
        &[
            (&base, ""),
            (&other, "facilitator_url = \"https://facilitator.example/\""),
        ],
        &app_addr,
    );

    let runtime = connector_cli::build(&config).await.expect("build");
    let app = connector_cli::router(&runtime, &config).expect("router");
    let terms = greeting(&app).await;

    let accepts = terms["accepts"].as_array().expect("accepts[]");
    assert_eq!(accepts.len(), 2, "{terms}");
    let node = format!("{:#x}", Address::from(node_address()));
    for (entry, net, facilitator) in [
        (&accepts[0], &base, None),
        (&accepts[1], &other, Some("https://facilitator.example/")),
    ] {
        assert_eq!(entry["network"], net.network(), "{entry}");
        assert_eq!(entry["asset"], format!("{:#x}", net.token), "{entry}");
        assert_eq!(entry["payTo"], node, "{entry}");
        assert_eq!(
            entry["extra"]["facilitator"].as_str(),
            facilitator,
            "{entry}"
        );
    }

    // The self-description lists each network separately too, with the
    // one settlement key as the voucher signer on each (ADR 0086 decision
    // 5).
    let networks: Vec<String> = runtime
        .voucher_signers
        .iter()
        .map(|fact| fact.network.clone())
        .collect();
    assert_eq!(networks, vec![base.network(), other.network()]);
    assert!(runtime
        .voucher_signers
        .iter()
        .all(|fact| fact.signer == node));
}

/// ADR 0086 decisions 1, 9 and 14: a voucher on each network is credited to
/// that network's channel, the app is told which network, a voucher naming
/// one network for a channel on the other is refused, and after a restart
/// each channel is restored to -- and lands through -- its own network's
/// backend.
#[tokio::test]
async fn a_voucher_on_each_network_is_credited_and_settled_on_that_network() {
    if !require_anvil() {
        return;
    }
    let base = Net::spawn(31_337).await;
    let other = Net::spawn(31_338).await;
    let (app_addr, recorded) = spawn_recording_app().await;
    let files = Files::new();
    let config = files.config(&[(&base, ""), (&other, "")], &app_addr);

    let runtime = connector_cli::build(&config).await.expect("build");
    let app = connector_cli::router(&runtime, &config).expect("router");
    let receiver = runtime.signer.public_key().expect("the node's wrap key");

    let mut channels = Vec::new();
    for net in [&base, &other] {
        let channel_config = net.config(node_address());
        let channel = net.open(channel_config.clone(), DEPOSIT).await;
        let signature = net
            .x402
            .sign_voucher(&session(), &channel, u128::from(PRICE));
        let response = post_ilp(
            &app,
            &evm_voucher_on(
                &net.network(),
                &channel,
                PRICE,
                &signature,
                Some(&channel_config),
            ),
            paid_prepare(ROUTE, &receiver),
        )
        .await;
        Fulfill::decode(&response).unwrap_or_else(|_| {
            panic!(
                "the voucher on {} paid for the write: {:?}",
                net.network(),
                Reject::decode(&response)
            )
        });
        channels.push((net, channel, signature));
    }

    // ADR 0086 decision 14: each write was paid on its own network, and the
    // app is told so; the payer key and the chain do not change.
    let writes = recorded.lock().unwrap().clone();
    assert_eq!(writes.len(), 2);
    for (write, (net, channel, _)) in writes.iter().zip(&channels) {
        assert_eq!(write.header("x-toon-network"), Some(net.network().as_str()));
        assert_eq!(write.header("x-toon-chain"), Some("evm"));
        assert_eq!(
            write.header("x-toon-payer"),
            Some(format!("evm:{}", channel.0).as_str())
        );
    }

    // A voucher naming 31338 on the channel opened on 31337: its id does
    // not recompute under 31338's domain, and it is refused by name.
    let (_, base_channel, _) = &channels[0];
    let signature = base
        .x402
        .sign_voucher(&session(), base_channel, u128::from(2 * PRICE));
    let refused = Reject::decode(
        &post_ilp(
            &app,
            &evm_voucher_on(&other.network(), base_channel, 2 * PRICE, &signature, None),
            paid_prepare(ROUTE, &receiver),
        )
        .await,
    )
    .expect("a voucher naming the other network is refused");
    assert!(
        refused.message.contains("eip155:31338"),
        "refused naming the network it claimed: {refused:?}"
    );

    // Restart from nothing but the config and the state_dir: each channel is
    // restored to its own network's backend, and lands there.
    drop(app);
    drop(runtime);
    let runtime = connector_cli::build(&config).await.expect("rebuild");
    for (net, channel, signature) in &channels {
        let backend = Arc::clone(
            runtime
                .batch_settlement_evm
                .get(&network(&net.network()))
                .expect("a backend per network"),
        ) as Arc<dyn BatchSettlementBackend>;
        let restored = backend
            .channel_state(channel)
            .await
            .expect("restored at boot to its own network's backend");
        assert_eq!(restored.status, BatchChannelStatus::Open);
        let landed = backend
            .land(
                channel,
                Voucher {
                    cumulative_amount: u128::from(PRICE),
                    signature: signature.clone(),
                },
            )
            .await
            .expect("the voucher lands on its own network")
            .state;
        assert_eq!(landed.landed, u128::from(PRICE));
        assert_eq!(
            net.x402.channel(channel).await,
            (DEPOSIT, u128::from(PRICE)),
            "totalClaimed on {} is the voucher's amount",
            net.network()
        );
    }
    // And neither channel exists on the other network.
    assert_eq!(other.x402.channel(&channels[0].1).await, (0, 0));
    assert_eq!(base.x402.channel(&channels[1].1).await, (0, 0));
}

/// ADR 0086 decision 4: two tables whose RPCs both report 31337 -- two
/// anvils left at the default -- are refused at boot, naming both tables.
#[tokio::test]
async fn two_tables_on_one_chain_id_are_refused_at_boot_by_name() {
    if !require_anvil() {
        return;
    }
    let base = Net::spawn(31_337).await;
    let twin = Net::spawn_reporting(31_338, 31_337).await;
    let files = Files::new();
    let config = files.config(&[(&base, ""), (&twin, "")], "127.0.0.1:9");

    let Err(error) = connector_cli::build(&config).await else {
        panic!("built a node on two tables reporting one chain id");
    };
    assert!(
        matches!(
            &error,
            RuntimeError::EvmNetworkReportedTwice { tables, reported: 31_337 }
                if *tables == vec!["eip155.31337".to_string(), "eip155.31338".to_string()]
        ),
        "{error}"
    );
    let message = error.to_string();
    assert!(
        message.contains("[settlement.eip155.31337]")
            && message.contains("[settlement.eip155.31338]")
            && message.contains("31337"),
        "{message}"
    );
}

/// ADR 0086 decisions 4 and 6 (ADR 0075 decision 1): each table is checked
/// on its own network -- one without x402's code is refused naming that
/// network, and a wrong EIP-712 name on one table is refused naming that
/// table, while the other table is fine.
#[tokio::test]
async fn each_table_is_checked_on_its_own_network_and_refused_by_its_name() {
    if !require_anvil() {
        return;
    }
    let base = Net::spawn(31_337).await;
    let files = Files::new();

    // 31338 is a bare anvil: no x402BatchSettlement at its canonical
    // address.
    let bare = Anvil::spawn_with_chain_id(ANVIL_BASE_PORT, 31_338).await;
    let tables = format!(
        "{}{}",
        base.table(files.settlement_key.path(), ""),
        table(
            31_338,
            &bare.rpc_url,
            base.token,
            files.settlement_key.path(),
            ""
        ),
    );
    let config = files.config_with(&tables, "127.0.0.1:9");
    let Err(error) = connector_cli::build(&config).await else {
        panic!("built a node on a network without x402");
    };
    assert!(
        matches!(&error, RuntimeError::X402NotDeployed { table, .. } if table == "eip155.31338"),
        "{error}"
    );
    assert!(
        error.to_string().contains("[settlement.eip155.31338]"),
        "{error}"
    );

    // A wrong EIP-712 name on 31338's table, with 31337's right.
    let other = Net::spawn(31_338).await;
    let tables = format!(
        "{}{}",
        base.table(files.settlement_key.path(), ""),
        other.table(files.settlement_key.path(), "").replace(
            "asset_eip712_name = \"USDC\"",
            "asset_eip712_name = \"USD Coin\""
        ),
    );
    let config = files.config_with(&tables, "127.0.0.1:9");
    let Err(error) = connector_cli::build(&config).await else {
        panic!("built a node whose 31338 table names the wrong EIP-712 domain");
    };
    assert!(
        matches!(
            &error,
            RuntimeError::AssetEip712DomainMismatch { table, name, .. }
                if table == "eip155.31338" && name == "USD Coin"
        ),
        "{error}"
    );
    assert!(
        error.to_string().contains("[settlement.eip155.31338]"),
        "{error}"
    );
}

/// The port's contract suite, run once per network against the backend the
/// node built for that network's table (ADR 0007, ADR 0086): the same
/// backend type on two chains, each upholding the port on its own.
#[tokio::test]
async fn each_networks_backend_upholds_the_batch_settlement_contract() {
    if !require_anvil() {
        return;
    }
    let base = Arc::new(Net::spawn(31_337).await);
    let other = Arc::new(Net::spawn(31_338).await);
    let files = Files::new();
    let config = files.config(&[(&base, ""), (&other, "")], "127.0.0.1:9");
    let runtime = connector_cli::build(&config).await.expect("build");

    for net in [&base, &other] {
        let backend = Arc::clone(
            runtime
                .batch_settlement_evm
                .get(&network(&net.network()))
                .expect("a backend per network"),
        );
        let net = Arc::clone(net);
        assert_upholds_the_contract(|| async move { fixture(net, backend) }).await;
    }
}

/// [`BatchContractFixture`] over `backend`, with `net`'s client as the
/// payer.
fn fixture(
    net: Arc<Net>,
    backend: Arc<connector_settlement_evm::EvmBatchSettlementBackend>,
) -> BatchContractFixture {
    let node = backend.own_address().to_fixed_bytes();
    let config_for = {
        let net = Arc::clone(&net);
        move |terms: ChannelTerms| {
            let base = net.config(node);
            let seat = if terms.pays_this_node {
                node
            } else {
                [0xbb; 20]
            };
            EvmChannelConfig {
                receiver: seat,
                receiver_authorizer: seat,
                token: if terms.in_settled_token {
                    net.token.to_fixed_bytes()
                } else {
                    net.other_token.to_fixed_bytes()
                },
                withdraw_delay: terms.delay_secs,
                ..base
            }
        }
    };
    let unopened = {
        let config = net.config(node);
        ChannelPresentation::Evm {
            channel: net.x402.channel_id(&config),
            config,
        }
    };
    BatchContractFixture {
        backend: backend as Arc<dyn BatchSettlementBackend>,
        minimum_delay_secs: ONE_DAY,
        open: {
            let net = Arc::clone(&net);
            Box::new(move |terms: ChannelTerms| {
                let net = Arc::clone(&net);
                let config = config_for(terms);
                let opened: BoxFuture<'static, OpenedChannel> = Box::pin(async move {
                    let channel = net.open(config.clone(), terms.deposit).await;
                    OpenedChannel {
                        presentation: ChannelPresentation::Evm { channel, config },
                        voucher_signer: VoucherSigner::Evm(session().address().into()),
                    }
                });
                opened
            })
        },
        deposit: {
            let net = Arc::clone(&net);
            Box::new(move |channel: &ChannelId, amount: u128| {
                let net = Arc::clone(&net);
                let channel = channel.clone();
                let deposited: BoxFuture<'static, ()> = Box::pin(async move {
                    let config = net.config_of(&channel);
                    net.x402
                        .deposit(&payer(), &config, amount, net.salt())
                        .await;
                });
                deposited
            })
        },
        begin_exit: {
            let net = Arc::clone(&net);
            Box::new(move |channel: &ChannelId| {
                let net = Arc::clone(&net);
                let channel = channel.clone();
                let exiting: BoxFuture<'static, ()> = Box::pin(async move {
                    let (balance, claimed) = net.x402.channel(&channel).await;
                    let config = net.config_of(&channel);
                    net.x402
                        .initiate_withdraw(&session(), &config, balance - claimed)
                        .await;
                });
                exiting
            })
        },
        sign: {
            let net = Arc::clone(&net);
            Box::new(move |channel: &ChannelId, amount: u128| {
                net.x402.sign_voucher(&session(), channel, amount)
            })
        },
        unopened,
    }
}

// ─── Peering between two two-network nodes (#1603) ──────────────────────────
//
// ADR 0086 decisions 11 and 12: a peering is on one network, chosen with
// `POST /peers`'s CAIP-2 `network` filter when the two nodes share several.
// Each node is served on a real socket, so `POST /peers` reads the other's
// self-description over HTTP exactly as it would in production.

/// What each node deposits behind its own outbound channel.
const PEERING_DEPOSIT: u128 = 10_000;
/// What a `POST /channels/:id/fund` adds.
const TOP_UP: u128 = 2_500;
/// Each node's settlement account's USDC, on each network.
const NODE_FUNDS: u128 = 1_000_000;

/// A distinct `created` per signed request: the operator surface rejects a
/// replayed signature (ADR 0008's #1067 amendment).
static NEXT_CREATED: AtomicU64 = AtomicU64::new(3_000);

/// Mint `NODE_FUNDS` of `net`'s USDC to each settlement key in `keys`.
async fn fund_nodes(net: &Net, keys: &[&str]) {
    for key in keys {
        net.x402
            .mint(net.token, wallet_of(key).address(), NODE_FUNDS)
            .await;
    }
}

fn wallet_of(key: &str) -> LocalWallet {
    key.parse::<LocalWallet>().expect("a settlement key")
}

/// One node settling on every network in `nets` as `settlement_key`, on a
/// real socket, with an operator write key and plaintext peering allowed.
struct PeerNode {
    name: &'static str,
    addr: std::net::SocketAddr,
    operator: ed25519_dalek::Keypair,
    /// Held so the node's key files and state outlive the test.
    files: Files,
    runtime: connector_cli::Runtime,
    router: axum::Router,
}

impl PeerNode {
    async fn boot(name: &'static str, settlement_key: &str, nets: &[&Net]) -> PeerNode {
        Self::boot_with(name, settlement_key, 0x07, nets, "").await
    }

    /// [`Self::boot`] with its own signer seed and `extra` TOML appended:
    /// routes, `[[tokens]]`, `[[rates]]`.
    async fn boot_with(
        name: &'static str,
        settlement_key: &str,
        signer_seed: u8,
        nets: &[&Net],
        extra: &str,
    ) -> PeerNode {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind the node");
        let addr = listener.local_addr().expect("the node's address");
        let operator = ed25519_dalek::Keypair::generate(&mut rand::rngs::OsRng);
        let files = Files::with_keys(settlement_key, signer_seed);
        let tables: String = nets
            .iter()
            .map(|net| net.table(files.settlement_key.path(), ""))
            .collect();
        let write_key: String = operator
            .public
            .to_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let config = support::load_config(&format!(
            r#"
client_edge_addr = "127.0.0.1:0"
state_dir = "{state_dir}"
peer_allow_plaintext_endpoints = true

[node]
addresses     = ["g.example.{name}"]
http_endpoint = "http://{addr}/ilp"

[signer]
key_file = "{signer_key}"

[operator]
bearer_token = "operator-secret"
write_keys = ["{write_key}"]
{tables}
{extra}
"#,
            state_dir = files.state_dir.path().display(),
            signer_key = files.signer_key.path().display(),
        ));
        let runtime = connector_cli::build(&config).await.expect("build the node");
        let router = connector_cli::router(&runtime, &config).expect("the node's router");
        let served = router.clone();
        tokio::spawn(async move {
            let _ = axum::Server::from_tcp(listener)
                .expect("serve the bound listener")
                .serve(served.into_make_service())
                .await;
        });
        PeerNode {
            name,
            addr,
            operator,
            files,
            runtime,
            router,
        }
    }

    fn url(&self) -> String {
        format!("http://{}/ilp", self.addr)
    }

    /// A signed operator write, answered with its status and body.
    async fn write(&self, path: &str, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
        let body = serde_json::to_vec(&body).expect("json");
        let created = NEXT_CREATED.fetch_add(1, Ordering::Relaxed);
        let (sig_input, sig, digest) = connector_operator::test_support::sign_request(
            &self.operator,
            "POST",
            path,
            &body,
            created,
            Some(9_999_999_999),
        );
        let request = Request::builder()
            .method("POST")
            .uri(path)
            .header("signature-input", sig_input)
            .header("signature", sig)
            .header("content-digest", digest)
            .body(Body::from(body))
            .expect("a request");
        answer(&self.router, request).await
    }

    async fn read(&self, path: &str) -> serde_json::Value {
        let request = Request::builder()
            .method("GET")
            .uri(path)
            .header("authorization", "Bearer operator-secret")
            .body(Body::empty())
            .expect("a request");
        let (status, body) = answer(&self.router, request).await;
        assert_eq!(status, StatusCode::OK, "{} GET {path}: {body}", self.name);
        body
    }

    /// `POST /peers` naming `other`, with `filter` merged into the body.
    async fn peer_with(
        &self,
        other: &PeerNode,
        filter: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        let mut body = serde_json::json!({
            "id": other.name,
            "url": other.url(),
            "fee": 0,
            "deposit": PEERING_DEPOSIT,
        });
        for (key, value) in filter.as_object().expect("an object") {
            body[key] = value.clone();
        }
        self.write("/peers", body).await
    }

    /// This node's outbound channels, as `GET /channels` reports them.
    async fn outbound(&self) -> Vec<serde_json::Value> {
        self.read("/channels")
            .await
            .as_array()
            .expect("a list")
            .iter()
            .filter(|view| view["direction"] == "outbound")
            .cloned()
            .collect()
    }

    /// The key a payload for this node is sealed to (ADR 0018).
    fn identity(&self) -> connector_signer::PublicKeyBytes {
        self.runtime
            .signer
            .public_key()
            .expect("a local signer has a public key")
    }

    /// Route everything under `g.example.<to>` to the peering with `via`.
    async fn route(&self, to: &PeerNode, via: &PeerNode) {
        let (status, body) = self
            .write(
                "/routes/peers",
                serde_json::json!({
                    "prefix": format!("g.example.{}", to.name),
                    "peer_id": via.name,
                    "price": APP_PRICE,
                }),
            )
            .await;
        assert!(
            status.is_success(),
            "{} routes to {}: {body}",
            self.name,
            to.name
        );
    }

    /// Originate one packet of `amount` over `POST /packets`, sealed to
    /// `payee`'s app route: `Ok` once the app answered, or the reject.
    async fn originate(&self, payee: &PeerNode, amount: u64) -> Result<(), Reject> {
        let plaintext = connector_domain::EnvelopeRequest {
            method: "POST".to_string(),
            target: "/".to_string(),
            headers: vec![],
            body: b"across networks".to_vec(),
        }
        .encode();
        let (data, shared_secret) =
            connector_signer::giftwrap::seal_request(&plaintext, &payee.identity()).expect("seal");
        let prepare = connector_domain::Prepare {
            amount,
            expires_at: chrono::Utc::now() + chrono::Duration::minutes(5),
            greeting: false,
            destination: format!("g.example.{}.app", payee.name),
            data,
        }
        .encode();
        let created = NEXT_CREATED.fetch_add(1, Ordering::Relaxed);
        let (sig_input, sig, digest) = connector_operator::test_support::sign_request(
            &self.operator,
            "POST",
            "/packets",
            &prepare,
            created,
            Some(9_999_999_999),
        );
        let request = Request::builder()
            .method("POST")
            .uri("/packets")
            .header("signature-input", sig_input)
            .header("signature", sig)
            .header("content-digest", digest)
            .body(Body::from(prepare))
            .expect("a request");
        let response = self
            .router
            .clone()
            .oneshot(request)
            .await
            .expect("an answer");
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = hyper::body::to_bytes(response.into_body())
            .await
            .expect("the body");
        match Fulfill::decode(&bytes) {
            Ok(fulfill) => {
                let opened =
                    connector_signer::giftwrap::open_response(&shared_secret, &fulfill.data)
                        .expect("open the response");
                let envelope =
                    connector_domain::EnvelopeResponse::decode(&opened).expect("an envelope");
                assert_eq!(envelope.status, 200);
                Ok(())
            }
            Err(_) => Err(Reject::decode(&bytes).expect("a FULFILL or a REJECT")),
        }
    }

    /// Every voucher amount this node's client-edge journal accepted on
    /// inbound channel `id`, in order -- what the payee was paid.
    fn journaled(&self, id: &str) -> Vec<u64> {
        let journal = std::fs::read_to_string(
            self.files
                .state_dir
                .path()
                .join(support::CLIENT_EDGE_JOURNAL),
        )
        .unwrap_or_default();
        journal
            .lines()
            .filter(|line| line.starts_with("inbound_claim_accepted\t"))
            .filter_map(|line| {
                let fields: Vec<&str> = line.split('\t').collect();
                (fields[1] == format!("evm:{id}")).then(|| fields[3].parse().expect("an amount"))
            })
            .collect()
    }
}

async fn answer(app: &axum::Router, request: Request<Body>) -> (StatusCode, serde_json::Value) {
    let response = app.clone().oneshot(request).await.expect("an answer");
    let status = response.status();
    let bytes = hyper::body::to_bytes(response.into_body())
        .await
        .expect("the body");
    let body = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| serde_json::Value::String(String::from_utf8_lossy(&bytes).into()));
    (status, body)
}

/// ADR 0086 decision 12 and story 31: two nodes that share two networks
/// have no honest default. `POST /peers` with no `network` is refused as
/// `AmbiguousNetwork`, naming both, and with the retired `chain` filter by
/// name -- and neither opens anything.
#[tokio::test]
async fn post_peers_between_two_network_nodes_needs_a_network() {
    if !require_anvil() {
        return;
    }
    let base = Net::spawn(31_337).await;
    let other = Net::spawn(31_338).await;
    let keys = [DEPLOYER_PRIVATE_KEY, COUNTERPARTY_PRIVATE_KEY];
    fund_nodes(&base, &keys).await;
    fund_nodes(&other, &keys).await;
    let a = PeerNode::boot("a", DEPLOYER_PRIVATE_KEY, &[&base, &other]).await;
    let b = PeerNode::boot("b", COUNTERPARTY_PRIVATE_KEY, &[&base, &other]).await;

    let (status, body) = a.peer_with(&b, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let message = body.as_str().expect("a message");
    assert!(message.contains("eip155:31337"), "{message}");
    assert!(message.contains("eip155:31338"), "{message}");
    assert!(message.contains("`network`"), "{message}");

    let (status, body) = a.peer_with(&b, serde_json::json!({"chain": "evm"})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body.as_str().expect("a message").contains("`chain`"),
        "{body}"
    );

    assert!(a.outbound().await.is_empty(), "nothing was opened");
    assert_eq!(a.read("/peers").await, serde_json::json!([]));
}

/// ADR 0086 decisions 11 and 12, stories 30, 33, 34 and 35: `POST /peers`
/// with `network` lands the peering there -- each node's own half on that
/// network's chain and nowhere else -- channel reads report it, a fund lands
/// on the channel's own network, and a repeat that would move the peering
/// to the other network is refused before anything is opened.
#[tokio::test]
async fn post_peers_with_a_network_lands_both_halves_on_it() {
    if !require_anvil() {
        return;
    }
    let base = Net::spawn(31_337).await;
    let other = Net::spawn(31_338).await;
    let keys = [DEPLOYER_PRIVATE_KEY, COUNTERPARTY_PRIVATE_KEY];
    fund_nodes(&base, &keys).await;
    fund_nodes(&other, &keys).await;
    let a = PeerNode::boot("a", DEPLOYER_PRIVATE_KEY, &[&base, &other]).await;
    let b = PeerNode::boot("b", COUNTERPARTY_PRIVATE_KEY, &[&base, &other]).await;
    let on = serde_json::json!({ "network": other.network() });

    // The payee first, then the payer, as `local/keys.sh` does: each opens
    // its own half, on the network the write names.
    let mut halves = Vec::new();
    for (from, to) in [(&b, &a), (&a, &b)] {
        let (status, established) = from.peer_with(to, on.clone()).await;
        assert_eq!(status, StatusCode::OK, "{}: {established}", from.name);
        assert_eq!(established["channel"]["status"], "created");
        assert_eq!(established["channel"]["chain"], "evm");
        assert_eq!(established["channel"]["network"], other.network());
        halves.push(ChannelId(
            established["channel"]["id"]
                .as_str()
                .expect("a channel id")
                .to_string(),
        ));
    }

    // Both halves are on 31338's chain, and neither is on 31337's.
    for channel in &halves {
        assert_eq!(other.x402.channel(channel).await.0, PEERING_DEPOSIT);
        assert_eq!(base.x402.channel(channel).await, (0, 0));
    }

    // Each node reads its own half back with its network.
    for (node, channel) in [(&b, &halves[0]), (&a, &halves[1])] {
        let views = node.outbound().await;
        assert_eq!(views.len(), 1, "{views:?}");
        assert_eq!(views[0]["id"], channel.0);
        assert_eq!(views[0]["network"], other.network());
        assert_eq!(views[0]["collateral"], PEERING_DEPOSIT as u64);
    }

    // A fund lands on the channel's own network: 31338's chain moves, and
    // the node's USDC on 31337 does not.
    let a_address = wallet_of(DEPLOYER_PRIVATE_KEY).address();
    let (status, funded) = a
        .write(
            &format!("/channels/{}/fund", halves[1].0),
            serde_json::json!({ "amount": TOP_UP }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{funded}");
    assert_eq!(funded["network"], other.network());
    assert_eq!(
        other.x402.channel(&halves[1]).await.0,
        PEERING_DEPOSIT + TOP_UP
    );
    assert_eq!(
        base.x402.balance_of(base.token, a_address).await,
        NODE_FUNDS
    );
    assert_eq!(
        other.x402.balance_of(other.token, a_address).await,
        NODE_FUNDS - PEERING_DEPOSIT - TOP_UP
    );

    // Moving the peering to 31337 by writing it again is refused, and no
    // channel is opened there.
    let (status, refused) = a
        .peer_with(&b, serde_json::json!({ "network": base.network() }))
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    let message = refused.as_str().expect("a message");
    assert!(message.contains(&other.network()), "{message}");
    assert!(message.contains(&base.network()), "{message}");
    assert_eq!(a.outbound().await.len(), 1);
    assert_eq!(
        base.x402.balance_of(base.token, a_address).await,
        NODE_FUNDS
    );

    // The same write on the peering's own network is a repeat: found, not
    // a second channel.
    let (status, repeated) = a.peer_with(&b, on).await;
    assert_eq!(status, StatusCode::OK, "{repeated}");
    assert_eq!(repeated["channel"]["status"], "found");
    assert_eq!(repeated["channel"]["id"], halves[1].0);
}

// ─────────────────────────────────────────────────────────────────────────
// A forward between networks (ADR 0086 decision 13, issue #1604): A pays B
// on 31337, B pays C on 31338, and B -- holding both tables -- forwards
// between them. USDC on the two networks sits at one address and is two
// assets, so it is a forward between assets: passed through when B
// declares no `[[tokens]]`, converted at B's declared rate when it does,
// and refused when B declares no rate for the pair.
// ─────────────────────────────────────────────────────────────────────────

/// What C's app route charges, and so the least a packet to it carries.
const APP_PRICE: u64 = 1_000;
/// What A originates toward C's app: an operator's own packet pays its own
/// node no fee (ADR 0061), and every peering here charges none, so this is
/// what B receives -- and, unconverted, what C receives.
const AMOUNT: u64 = 1_000;
/// Anvil's third well-known dev account: C's settlement key, so C's
/// voucher signer is not A's. Not genesis-funded; [`Crossing::build`] gives it
/// gas.
const THIRD_PRIVATE_KEY: &str = "5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a";

/// A three-node path across two networks: A -> B -> C.
struct Crossing {
    base: Net,
    other: Net,
    a: PeerNode,
    b: PeerNode,
    c: PeerNode,
    /// A's channel to B, on 31337.
    a_to_b: String,
    /// B's channel to C, on 31338.
    b_to_c: String,
    app: Arc<Mutex<Vec<support::RecordedWrite>>>,
}

impl Crossing {
    /// A–B peered on 31337, B–C on 31338, each payee first, and a route
    /// from A through B to C's app. `dealing` is B's `[[tokens]]`/`[[rates]]`
    /// declaration, given the two networks.
    async fn build(dealing: impl Fn(&Net, &Net) -> String) -> Crossing {
        let base = Net::spawn(31_337).await;
        let other = Net::spawn(31_338).await;
        other
            .x402
            .fund_gas(wallet_of(THIRD_PRIVATE_KEY).address())
            .await;
        fund_nodes(&base, &[COUNTERPARTY_PRIVATE_KEY, DEPLOYER_PRIVATE_KEY]).await;
        fund_nodes(&other, &[DEPLOYER_PRIVATE_KEY, THIRD_PRIVATE_KEY]).await;
        let (app_addr, app) = spawn_recording_app().await;

        let a = PeerNode::boot_with("a", COUNTERPARTY_PRIVATE_KEY, 0x0a, &[&base], "").await;
        let b = PeerNode::boot_with(
            "b",
            DEPLOYER_PRIVATE_KEY,
            0x0b,
            &[&base, &other],
            &dealing(&base, &other),
        )
        .await;
        let c = PeerNode::boot_with(
            "c",
            THIRD_PRIVATE_KEY,
            0x0c,
            &[&other],
            &format!(
                "[[routes]]\nprefix = \"g.example.c.app\"\nhandler_url = \"http://{app_addr}\"\n\
                 price = {APP_PRICE}\n"
            ),
        )
        .await;

        let mut payers = Vec::new();
        for (payee, payer, net) in [(&b, &a, &base), (&c, &b, &other)] {
            let on = serde_json::json!({ "network": net.network() });
            let (status, body) = payee.peer_with(payer, on.clone()).await;
            assert_eq!(status, StatusCode::OK, "{}: {body}", payee.name);
            let (status, body) = payer.peer_with(payee, on).await;
            assert_eq!(status, StatusCode::OK, "{}: {body}", payer.name);
            assert_eq!(body["channel"]["network"], net.network());
            payers.push(body["channel"]["id"].as_str().expect("an id").to_string());
        }
        a.route(&c, &b).await;
        b.route(&c, &c).await;

        let b_to_c = payers.pop().expect("B's channel to C");
        let a_to_b = payers.pop().expect("A's channel to B");
        Crossing {
            base,
            other,
            a,
            b,
            c,
            a_to_b,
            b_to_c,
            app,
        }
    }
}

/// B's dealing declaration: 31337's USDC its numeraire, 31338's USDC a
/// token it deals, a third token on 31337 (that anvil's second FiatToken)
/// it also deals, and `rates` as written -- `{base}`, `{other}` and
/// `{third}` stand for the three assets.
fn dealing_with(rates: &str) -> impl Fn(&Net, &Net) -> String + '_ {
    move |base_net, other_net| {
        let base = format!("{}/erc20:{:?}", base_net.network(), base_net.token);
        let other = format!("{}/erc20:{:?}", other_net.network(), other_net.token);
        let third = format!("{}/erc20:{:?}", base_net.network(), base_net.other_token);
        format!(
            r#"
[[tokens]]
asset = "{base}"
numeraire = true

[[tokens]]
asset = "{other}"

[[tokens]]
asset = "{third}"
{rates}
[rate_guards]
spread = {{ numerator = 1, denominator = 100 }}
ttl_secs = 300
max_move = {{ numerator = 5, denominator = 100 }}
"#,
            rates = rates
                .replace("{base}", &base)
                .replace("{other}", &other)
                .replace("{third}", &third),
        )
    }
}

/// Story 36: a node with no `[[tokens]]` forwards a packet that arrived on
/// its 31337 peering out over its 31338 peering unconverted, as
/// `mixed-chain` forwards Base to Solana.
#[tokio::test]
async fn a_pass_through_node_forwards_between_networks_unconverted() {
    if !require_anvil() {
        return;
    }
    let path = Crossing::build(|_, _| String::new()).await;

    path.a
        .originate(&path.c, AMOUNT)
        .await
        .unwrap_or_else(|reject| panic!("A→B→C: {reject:?}"));

    assert_eq!(
        path.b.journaled(&path.a_to_b),
        vec![AMOUNT],
        "B was paid on 31337"
    );
    assert_eq!(
        path.c.journaled(&path.b_to_c),
        vec![AMOUNT],
        "C was paid on 31338 exactly what B was paid on 31337"
    );
    // Each channel is on its own network's chain and not on the other's.
    assert_eq!(
        path.other
            .x402
            .channel(&ChannelId(path.a_to_b.clone()))
            .await,
        (0, 0)
    );
    assert_eq!(
        path.base
            .x402
            .channel(&ChannelId(path.b_to_c.clone()))
            .await,
        (0, 0)
    );
    let writes = path.app.lock().unwrap().clone();
    assert_eq!(writes.len(), 1);
    assert_eq!(
        writes[0].header("x-toon-network"),
        Some(path.other.network().as_str()),
        "C's app is told the payment it was reached by is on 31338"
    );
}

/// Story 37: a dealing node converts the same forward at its declared
/// `[[rates]]` figure, and the payee's journal shows the converted amount.
#[tokio::test]
async fn a_dealing_node_converts_a_forward_between_networks_at_its_declared_rate() {
    if !require_anvil() {
        return;
    }
    let path = Crossing::build(dealing_with(
        "[[rates]]\nfrom = \"{base}\"\nto = \"{other}\"\nrate = { numerator = 2, denominator = 1 }\n",
    ))
    .await;

    path.a
        .originate(&path.c, AMOUNT)
        .await
        .unwrap_or_else(|reject| panic!("A→B→C: {reject:?}"));

    assert_eq!(path.b.journaled(&path.a_to_b), vec![AMOUNT]);
    assert_eq!(
        path.c.journaled(&path.b_to_c),
        // 1000 at the declared 2/1 mid, less B's 1/100 spread: 1980.
        vec![1_980],
        "C was paid the converted amount, not the amount B received"
    );
}

/// Story 38: with no declared rate for the pair, the forward is refused,
/// never dealt at an implicit 1:1, and C is paid nothing.
///
/// B prices 31338's USDC only against a third token, so the load rule (a
/// token off the numeraire's network needs a static rate) is met while
/// 31337's USDC to 31338's has no row and composes through nothing. (A row
/// from 31338's USDC to the numeraire would compose this pair as its
/// inverse.)
#[tokio::test]
async fn a_forward_between_networks_with_no_declared_rate_is_refused() {
    if !require_anvil() {
        return;
    }
    let path = Crossing::build(dealing_with(
        "[[rates]]\nfrom = \"{other}\"\nto = \"{third}\"\nrate = { numerator = 1, denominator = 1 }\n",
    ))
    .await;

    let reject = path
        .a
        .originate(&path.c, AMOUNT)
        .await
        .expect_err("no rate is declared for 31337's USDC to 31338's");

    assert_eq!(reject.code.as_str(), "F02", "{reject:?}");
    assert!(
        reject.message.contains("eip155:31337/") && reject.message.contains("eip155:31338/"),
        "the refusal names the pair: {reject:?}"
    );
    assert!(
        path.c.journaled(&path.b_to_c).is_empty(),
        "C was paid nothing"
    );
    assert!(path.app.lock().unwrap().is_empty());
}
