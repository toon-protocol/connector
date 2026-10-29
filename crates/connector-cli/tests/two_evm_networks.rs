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
use connector_domain::JournalEntry;
use connector_domain::{Fulfill, Network, Reject};
use connector_runtime::{FileJournal, Journal};
use connector_settlement::batch::contract::{
    assert_upholds_the_contract, BatchContractFixture, BoxFuture, ChannelTerms, OpenedChannel,
};
use connector_settlement::batch::{
    BatchChannelStatus, BatchSettlementBackend, BatchSettlementPayer, ChannelPresentation,
    EvmChannelConfig, EvmReceiverTerms, OutboundChannelRecord, ReceiverTerms, Voucher,
    VoucherSigner,
};
use connector_settlement::ChannelId;
use connector_settlement_evm::test_support::x402::X402Chain;
use connector_settlement_evm::test_support::{require_anvil, Anvil, DEPLOYER_PRIVATE_KEY};
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
        let mut signer_key = tempfile::NamedTempFile::new().expect("signer key");
        signer_key.write_all(&[0x07; 32]).expect("write");
        let mut settlement_key = tempfile::NamedTempFile::new().expect("settlement key");
        settlement_key
            .write_all(DEPLOYER_PRIVATE_KEY.as_bytes())
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

/// The outbound channel journal under `state_dir`, as the runtime names it.
fn outbound_journal(files: &Files) -> FileJournal {
    FileJournal::open(files.state_dir.path().join("outbound-channels.log")).expect("the journal")
}

/// An outbound channel the node opened on `net` under a build from before
/// ADR 0086: opened on chain through the node's own backend for that
/// network, then journaled as that build did, with a version-1 record that
/// names no network.
async fn open_as_before_adr_0086(
    config: &connector_config::Config,
    files: &Files,
    net: &Net,
) -> OutboundChannelRecord {
    net.x402
        .mint(net.token, Address::from(node_address()), 1_000_000)
        .await;
    let runtime = connector_cli::build(config).await.expect("build");
    let payer = Arc::clone(
        runtime
            .batch_settlement_evm
            .get(&network(&net.network()))
            .expect("a backend for the network"),
    );
    let record = payer
        .prepare_open(
            ReceiverTerms::Evm(EvmReceiverTerms {
                receiver: [0x55; 20],
                token: net.token.to_fixed_bytes(),
                min_withdraw_delay_secs: ONE_DAY,
            }),
            DEPOSIT,
        )
        .await
        .expect("prepare");
    payer.open_prepared(&record).await.expect("opened on chain");
    drop(payer);
    drop(runtime);
    let key = format!("evm:{}", record.channel().0);
    let journal = outbound_journal(files);
    journal
        .append(&JournalEntry::OutboundChannelOpening {
            channel_id: key.clone(),
            record: record.encode_version_1(),
        })
        .expect("journal");
    journal
        .append(&JournalEntry::OutboundChannelOpened { channel_id: key })
        .expect("journal");
    record
}

/// ADR 0086 decision 10, story 24: a node upgraded with a live outbound
/// channel keeps it. The version-1 record is rewritten as version 2 on the
/// network whose domain reproduces its id, under its unchanged key; a
/// top-up and a voucher on it go through that network; a restart writes
/// nothing more.
#[tokio::test]
async fn a_version_1_outbound_record_is_upgraded_to_the_network_that_reproduces_it() {
    if !require_anvil() {
        return;
    }
    let base = Net::spawn(31_337).await;
    let other = Net::spawn(31_338).await;
    let files = Files::new();
    let config = files.config(&[(&base, ""), (&other, "")], "127.0.0.1:9");
    let record = open_as_before_adr_0086(&config, &files, &other).await;
    let channel = record.channel().clone();
    let key = format!("evm:{}", channel.0);

    let runtime = connector_cli::build(&config).await.expect("it boots");
    let outbound = runtime.outbound_channels.clone().expect("x402");
    assert_eq!(
        outbound.network(&channel.0),
        Some(network(&other.network()))
    );

    let journal = outbound_journal(&files).read_all().expect("read");
    assert_eq!(
        journal.last(),
        Some(&JournalEntry::OutboundChannelOpening {
            channel_id: key.clone(),
            record: record.encode_on(&other.network()),
        }),
        "rewritten as version 2 on 31338"
    );
    for entry in &journal {
        let (JournalEntry::OutboundChannelOpening { channel_id, .. }
        | JournalEntry::OutboundChannelOpened { channel_id }) = entry
        else {
            panic!("an unexpected entry {entry:?}");
        };
        assert_eq!(channel_id, &key, "the journal key is unchanged");
    }

    // A top-up is sent on 31338, where the channel is; 31337 never sees it.
    outbound.top_up(&channel.0, 7).await.expect("top-up");
    assert_eq!(other.x402.channel(&channel).await, (DEPOSIT + 7, 0));
    assert_eq!(base.x402.channel(&channel).await, (0, 0));

    // A voucher is signed under 31338's domain, and not 31337's.
    let voucher = outbound
        .sign_voucher(&channel.0, 25)
        .await
        .expect("voucher");
    let id: [u8; 32] = ethers::utils::hex::decode(&channel.0[2..])
        .expect("hex")
        .try_into()
        .expect("32 bytes");
    let signature: [u8; 65] = voucher.signature.try_into().expect("65 bytes");
    let verifies_on = |chain_id| {
        connector_signer::verify_evm_voucher(
            &connector_signer::BatchSettlementDomain::x402(chain_id),
            &id,
            25,
            &signature,
            &node_address(),
        )
    };
    assert!(verifies_on(31_338), "signed on the channel's own network");
    assert!(!verifies_on(31_337));

    // Idempotent: a restart replays version 2 and writes no upgrade again.
    drop(outbound);
    drop(runtime);
    let before = outbound_journal(&files).read_all().expect("read");
    let runtime = connector_cli::build(&config).await.expect("it reboots");
    assert_eq!(
        runtime
            .outbound_channels
            .as_ref()
            .expect("x402")
            .network(&channel.0),
        Some(network(&other.network()))
    );
    assert_eq!(outbound_journal(&files).read_all().expect("read"), before);
}

/// ADR 0086 decision 10, story 25: the table of the network a version-1
/// record's channel is on was removed. One EVM table left is not proof, so
/// the node refuses to start, naming the channel it still holds value in.
#[tokio::test]
async fn a_version_1_outbound_record_no_table_reproduces_is_refused_at_boot_by_name() {
    if !require_anvil() {
        return;
    }
    let base = Net::spawn(31_337).await;
    let other = Net::spawn(31_338).await;
    let files = Files::new();
    let both = files.config(&[(&base, ""), (&other, "")], "127.0.0.1:9");
    let record = open_as_before_adr_0086(&both, &files, &other).await;
    let before = outbound_journal(&files).read_all().expect("read");

    let base_only = files.config(&[(&base, "")], "127.0.0.1:9");
    let Err(error) = connector_cli::build(&base_only).await else {
        panic!("booted holding a channel on a network it no longer settles on");
    };
    let key = format!("evm:{}", record.channel().0);
    assert!(
        matches!(&error, RuntimeError::OutboundChannelOnNoNetwork { channel_id, network: None }
            if *channel_id == key),
        "{error}"
    );
    assert!(error.to_string().contains(&record.channel().0), "{error}");
    assert_eq!(
        outbound_journal(&files).read_all().expect("read"),
        before,
        "nothing rewritten"
    );
}
