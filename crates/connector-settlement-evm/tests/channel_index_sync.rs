//! `EvmChannelIndexSyncer` against a real, disposable `anvil` chain (ADR
//! 0007, issue #661): a channel opened and funded on chain shows up in the
//! index once it is deep enough behind head, a channel inside the
//! confirmation window does not, and a settled channel is reported
//! [`ChannelIndexLookup::Terminal`]. See `tests/support/mod.rs` for how the
//! chain is stood up.
//!
//! One case here needs no chain at all: `anvil` answers a wide
//! `eth_getLogs` happily, which is exactly why it never caught the syncer
//! asking for one. That case drives a stub RPC that behaves like the public
//! endpoint the devnet boxes actually point at -- see
//! [`AddressRestrictedRpc`].

mod support;

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use chrono::Duration;
use connector_settlement::SettlementBackend;
use connector_settlement_evm::EvmSettlementBackend;
use connector_settlement_evm::{
    ChannelIndexLookup, EvmChannelIndex, EvmChannelIndexSyncer, IndexedContract, RejectedSnapshot,
};
use ethers::core::rand::thread_rng;
use ethers::providers::{Http, Provider};
use ethers::signers::{LocalWallet, Signer as EvmSigner};
use ethers::types::Address;

use support::{require_anvil, Anvil, DEPLOYER_PRIVATE_KEY};

/// `MIN_SETTLEMENT_TIMEOUT` plus margin -- see `contract_suite.rs`'s own
/// constant of the same shape.
const INSTANT_SETTLEMENT_TIMEOUT_SECONDS: i64 = 3_601;

async fn advance_anvil_time(rpc_url: &str, seconds: i64) {
    let provider = Provider::<Http>::try_from(rpc_url).expect("build provider");
    let _: serde_json::Value = provider
        .request("evm_increaseTime", [seconds])
        .await
        .expect("evm_increaseTime");
    let _: serde_json::Value = provider.request("evm_mine", ()).await.expect("evm_mine");
}

/// Mine `count` empty blocks -- how this test pushes a channel-open past a
/// configured confirmation depth without waiting on real wall-clock time.
async fn mine_blocks(rpc_url: &str, count: u64) {
    let provider = Provider::<Http>::try_from(rpc_url).expect("build provider");
    for _ in 0..count {
        let _: serde_json::Value = provider.request("evm_mine", ()).await.expect("evm_mine");
    }
}

/// What an index driven off `backend`'s chain and `TokenNetwork` is about
/// (issue #1282). Taken from the backend rather than written down here, so
/// it is the chain id anvil actually reports and the `TokenNetwork` the
/// registry actually resolved.
fn indexed_contract_of(backend: &EvmSettlementBackend) -> IndexedContract {
    IndexedContract {
        chain_id: backend.chain_id(),
        token_network: backend.address(),
    }
}

/// The same, for the cases below that drive a stub RPC rather than a real
/// chain: Base Sepolia's chain id, since `DEVNET_TOKEN_NETWORK` is the
/// contract the devnet boxes point at there.
fn devnet_indexed_contract(token_network: Address) -> IndexedContract {
    IndexedContract {
        chain_id: 84_532,
        token_network,
    }
}

/// Run `syncer.sync_once` until it reports no more progress -- the same
/// "drain the backlog" loop `EvmChannelIndexSyncer::run` performs, without
/// its indefinite poll sleep, so a test can assert on a caught-up index.
async fn sync_to_caught_up(syncer: &EvmChannelIndexSyncer, index: &EvmChannelIndex) {
    for _ in 0..10_000 {
        let progressed = syncer.sync_once(index).await.expect("sync_once");
        if progressed == 0 {
            return;
        }
    }
    panic!("channel index sync did not converge after 10,000 bounded ranges");
}

#[tokio::test]
async fn a_channel_opened_and_funded_on_chain_is_indexed_once_confirmed() {
    if !require_anvil() {
        return;
    }
    let anvil = Anvil::spawn().await;
    let rpc_url = anvil.rpc_url.clone();

    let token = EvmSettlementBackend::deploy_mock_token(&rpc_url, DEPLOYER_PRIVATE_KEY, 1_000_000)
        .await
        .expect("deploy mock USDC");
    let backend = EvmSettlementBackend::deploy(&rpc_url, DEPLOYER_PRIVATE_KEY, token)
        .await
        .expect("deploy a TokenNetwork through a fresh registry");

    let counterparty = LocalWallet::new(&mut thread_rng())
        .address()
        .as_bytes()
        .to_vec();
    let channel = backend
        .open(
            counterparty.clone(),
            Duration::seconds(INSTANT_SETTLEMENT_TIMEOUT_SECONDS),
        )
        .await
        .expect("open a channel");
    // The index reports the *counterparty's* deposit for a lookup made
    // under this backend's own address, so that is the side funded here
    // (issue #1118).
    backend
        .fund_counterparty(&channel, 750)
        .await
        .expect("fund the counterparty's side of the channel");

    // One confirmation, and mine a couple more blocks so the open/fund logs
    // are comfortably behind head.
    mine_blocks(&rpc_url, 3).await;

    let index = EvmChannelIndex::open(None, indexed_contract_of(&backend), 0)
        .expect("open in-memory index");
    let syncer =
        EvmChannelIndexSyncer::new(&rpc_url, backend.address(), 1, 0).expect("build syncer");
    sync_to_caught_up(&syncer, &index).await;

    let channel_id = support::channel_id_bytes(&channel.0);
    match index.lookup(&channel_id, backend.own_address()) {
        ChannelIndexLookup::Active {
            counterparty: found,
            deposit,
        } => {
            assert_eq!(found.as_bytes(), counterparty.as_slice());
            assert_eq!(deposit, ethers::types::U256::from(750u64));
        }
        other => panic!("expected an active, funded channel, got {other:?}"),
    }
}

#[tokio::test]
async fn a_channel_inside_the_confirmation_window_is_not_yet_indexed() {
    if !require_anvil() {
        return;
    }
    let anvil = Anvil::spawn().await;
    let rpc_url = anvil.rpc_url.clone();

    let token = EvmSettlementBackend::deploy_mock_token(&rpc_url, DEPLOYER_PRIVATE_KEY, 1_000_000)
        .await
        .expect("deploy mock USDC");
    let backend = EvmSettlementBackend::deploy(&rpc_url, DEPLOYER_PRIVATE_KEY, token)
        .await
        .expect("deploy a TokenNetwork through a fresh registry");

    let counterparty = LocalWallet::new(&mut thread_rng())
        .address()
        .as_bytes()
        .to_vec();
    let channel = backend
        .open(
            counterparty,
            Duration::seconds(INSTANT_SETTLEMENT_TIMEOUT_SECONDS),
        )
        .await
        .expect("open a channel");

    // A confirmation depth deeper than this chain has ever reached: the
    // open is real, but this index must not have caught up to it yet.
    let index = EvmChannelIndex::open(None, indexed_contract_of(&backend), 0)
        .expect("open in-memory index");
    let syncer =
        EvmChannelIndexSyncer::new(&rpc_url, backend.address(), 1_000, 0).expect("build syncer");
    sync_to_caught_up(&syncer, &index).await;

    let channel_id = support::channel_id_bytes(&channel.0);
    assert_eq!(
        index.lookup(&channel_id, backend.own_address()),
        ChannelIndexLookup::Miss,
        "a channel inside the confirmation window must fall through to a direct chain read, \
         not be answered from an index that has not caught up to it"
    );
}

#[tokio::test]
async fn a_settled_channel_is_indexed_as_terminal_without_a_further_chain_read() {
    if !require_anvil() {
        return;
    }
    let anvil = Anvil::spawn().await;
    let rpc_url = anvil.rpc_url.clone();

    let token = EvmSettlementBackend::deploy_mock_token(&rpc_url, DEPLOYER_PRIVATE_KEY, 1_000_000)
        .await
        .expect("deploy mock USDC");
    let backend = EvmSettlementBackend::deploy(&rpc_url, DEPLOYER_PRIVATE_KEY, token)
        .await
        .expect("deploy a TokenNetwork through a fresh registry");

    let counterparty = LocalWallet::new(&mut thread_rng())
        .address()
        .as_bytes()
        .to_vec();
    let channel = backend
        .open(
            counterparty,
            Duration::seconds(INSTANT_SETTLEMENT_TIMEOUT_SECONDS),
        )
        .await
        .expect("open a channel");
    backend.close(&channel).await.expect("close the channel");
    advance_anvil_time(&rpc_url, INSTANT_SETTLEMENT_TIMEOUT_SECONDS).await;
    backend.settle(&channel).await.expect("settle the channel");
    mine_blocks(&rpc_url, 3).await;

    let index = EvmChannelIndex::open(None, indexed_contract_of(&backend), 0)
        .expect("open in-memory index");
    let syncer =
        EvmChannelIndexSyncer::new(&rpc_url, backend.address(), 1, 0).expect("build syncer");
    sync_to_caught_up(&syncer, &index).await;

    let channel_id = support::channel_id_bytes(&channel.0);
    assert_eq!(
        index.lookup(&channel_id, backend.own_address()),
        ChannelIndexLookup::Terminal
    );
}

/// Issue #1282, against a real chain and a real state volume: the snapshot
/// a caught-up node leaves behind is not resumed once `[settlement.evm]`
/// names a different chain.
///
/// A settled channel is the case that makes this a wrong answer rather
/// than a slow one -- [`ChannelIndexLookup::Terminal`] is served from this
/// table with **no chain read at all**, so a snapshot carried across a
/// cutover can refuse a channel that is genuinely open on the new chain.
/// The chain is here rather than faked because the rows being discarded
/// have to be rows a real `TokenNetwork`'s own logs produced, not rows a
/// test wrote.
#[tokio::test]
async fn a_snapshot_from_another_chain_is_not_resumed_against_this_one() {
    if !require_anvil() {
        return;
    }
    let anvil = Anvil::spawn().await;
    let rpc_url = anvil.rpc_url.clone();

    let token = EvmSettlementBackend::deploy_mock_token(&rpc_url, DEPLOYER_PRIVATE_KEY, 1_000_000)
        .await
        .expect("deploy mock USDC");
    let backend = EvmSettlementBackend::deploy(&rpc_url, DEPLOYER_PRIVATE_KEY, token)
        .await
        .expect("deploy a TokenNetwork through a fresh registry");

    let counterparty = LocalWallet::new(&mut thread_rng())
        .address()
        .as_bytes()
        .to_vec();
    let channel = backend
        .open(
            counterparty,
            Duration::seconds(INSTANT_SETTLEMENT_TIMEOUT_SECONDS),
        )
        .await
        .expect("open a channel");
    backend.close(&channel).await.expect("close the channel");
    advance_anvil_time(&rpc_url, INSTANT_SETTLEMENT_TIMEOUT_SECONDS).await;
    backend.settle(&channel).await.expect("settle the channel");
    mine_blocks(&rpc_url, 3).await;

    let state_dir = tempfile::tempdir().expect("temp state dir");
    let snapshot = state_dir.path().join("evm-channel-index.json");
    let channel_id = support::channel_id_bytes(&channel.0);
    let syncer =
        EvmChannelIndexSyncer::new(&rpc_url, backend.address(), 1, 0).expect("build syncer");

    // A node that has caught up on this chain, and stopped.
    let checkpoint = {
        let index = EvmChannelIndex::open(Some(&snapshot), indexed_contract_of(&backend), 0)
            .expect("open the durable index");
        sync_to_caught_up(&syncer, &index).await;
        assert_eq!(
            index.lookup(&channel_id, backend.own_address()),
            ChannelIndexLookup::Terminal
        );
        index
            .last_indexed_block()
            .expect("a caught-up index has a checkpoint")
    };

    // The same state volume, after the operator moved `[settlement.evm]`
    // to another chain. Nothing about the file changed; what it indexes
    // did.
    let elsewhere = EvmChannelIndex::open(
        Some(&snapshot),
        IndexedContract {
            chain_id: backend.chain_id() + 1,
            token_network: backend.address(),
        },
        0,
    )
    .expect("a snapshot from another chain opens rather than erroring");

    assert_eq!(
        elsewhere.last_indexed_block(),
        None,
        "a checkpoint at block {checkpoint} on another chain must not be resumed as this \
         chain's progress"
    );
    assert_eq!(
        elsewhere.lookup(&channel_id, backend.own_address()),
        ChannelIndexLookup::Miss,
        "the other chain's settled channel must fall through to a direct chain read, not be \
         refused Terminal out of an index belonging to a different deployment"
    );
    assert!(
        matches!(
            elsewhere.rejected_snapshot(),
            Some(RejectedSnapshot::ChainId { .. })
        ),
        "the operator must be able to find out why this node re-backfilled: {:?}",
        elsewhere.rejected_snapshot()
    );

    // And the file was ignored, not repaired or deleted: pointed back at
    // the chain it describes, it is a checkpoint again. The rule is "do
    // not trust it", not "rewrite it".
    let home_again = EvmChannelIndex::open(Some(&snapshot), indexed_contract_of(&backend), 0)
        .expect("re-open against the chain the snapshot does describe");
    assert_eq!(home_again.last_indexed_block(), Some(checkpoint));
    assert_eq!(home_again.rejected_snapshot(), None);
    assert_eq!(
        home_again.lookup(&channel_id, backend.own_address()),
        ChannelIndexLookup::Terminal
    );
}

// ── the address filter (verified live 2026-08-14) ────────────────────────────

/// The `TokenNetwork` the devnet relay and store boxes both point at
/// (`infra/linode-*/connector-rust.toml`), used verbatim so the recorded
/// filter can be compared against the address an operator would read off
/// the committed config.
const DEVNET_TOKEN_NETWORK: &str = "0xe9E05dfecfe165266C88d73e61D483612651952a";

/// A JSON-RPC endpoint that refuses an `eth_getLogs` naming no contract,
/// the way `https://base-sepolia-rpc.publicnode.com` -- the `rpc_url` both
/// devnet boxes are configured with -- does:
///
/// ```text
/// (code: -32701, message: Please specify an address in your request, or, to
///  remove restrictions, order a dedicated full node here: ...)
/// ```
///
/// This is not a hypothetical provider. Both boxes logged that refusal every
/// five seconds for the life of every process: the syncer never took a
/// checkpoint, so every channel lookup fell back to a direct chain read
/// forever. `anvil` serves the wide query without complaint, which is
/// precisely why the anvil-backed cases above passed throughout.
///
/// Hand-rolled over a `TcpListener` rather than pulled in as another test
/// dependency, following `crates/connector-bin/tests/announce_subcommand.rs`'s
/// `RecordingIngress`: it serves exactly one request shape (a `POST` with a
/// `content-length` carrying one JSON-RPC call), which is the only shape
/// `Provider<Http>` ever sends.
struct AddressRestrictedRpc {
    url: String,
    filters: Arc<Mutex<Vec<serde_json::Value>>>,
}

impl AddressRestrictedRpc {
    fn start(head_block: u64) -> AddressRestrictedRpc {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind stub rpc");
        let url = format!("http://{}", listener.local_addr().expect("stub rpc addr"));
        let filters = Arc::new(Mutex::new(Vec::new()));
        let recorded = filters.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let Some(body) = read_request_body(&mut stream) else {
                    continue;
                };
                let request: serde_json::Value = match serde_json::from_slice(&body) {
                    Ok(request) => request,
                    Err(_) => continue,
                };
                let id = request["id"].clone();
                let response = match request["method"].as_str() {
                    Some("eth_blockNumber") => {
                        serde_json::json!({"jsonrpc":"2.0","id":id,"result":format!("{head_block:#x}")})
                    }
                    Some("eth_getLogs") => {
                        let filter = request["params"][0].clone();
                        recorded.lock().expect("filter lock").push(filter.clone());
                        if filter.get("address").is_none() {
                            serde_json::json!({"jsonrpc":"2.0","id":id,"error":{
                                "code": -32701,
                                "message": "Please specify an address in your request or, to \
                                            remove restrictions, order a dedicated full node \
                                            here: https://www.allnodes.com/base/host",
                            }})
                        } else {
                            serde_json::json!({"jsonrpc":"2.0","id":id,"result":[]})
                        }
                    }
                    // Anything else this provider asks on the way (a chain
                    // id probe, say) is answered rather than hung on: the
                    // subject here is the log query, not the handshake.
                    _ => serde_json::json!({"jsonrpc":"2.0","id":id,"result":"0x1"}),
                };
                let payload = response.to_string();
                let _ = stream.write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\
                         content-length: {}\r\nconnection: close\r\n\r\n{payload}",
                        payload.len()
                    )
                    .as_bytes(),
                );
            }
        });
        AddressRestrictedRpc { url, filters }
    }

    fn filters(&self) -> Vec<serde_json::Value> {
        self.filters.lock().expect("filter lock").clone()
    }
}

/// Read one `POST` with a `content-length` body off `stream` -- the only
/// request shape this stub is ever sent.
fn read_request_body(stream: &mut std::net::TcpStream) -> Option<Vec<u8>> {
    let mut raw = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        let read = match stream.read(&mut buffer) {
            Ok(0) | Err(_) => return None,
            Ok(read) => read,
        };
        raw.extend_from_slice(&buffer[..read]);
        let Some(split) = raw
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .map(|at| at + 4)
        else {
            continue;
        };
        let headers = String::from_utf8_lossy(&raw[..split]).to_lowercase();
        let length: usize = headers
            .split("content-length:")
            .nth(1)
            .and_then(|rest| rest.split("\r\n").next())
            .and_then(|value| value.trim().parse().ok())
            .unwrap_or(0);
        if raw.len() - split >= length {
            return Some(raw[split..split + length].to_vec());
        }
    }
}

/// Every `eth_getLogs` this syncer sends names the `TokenNetwork` it was
/// built for, so an address-restricted provider serves it.
///
/// `ethers-contract`'s `Contract::event::<D>()` builds `D::new(Filter::new(),
/// ..)` -- a filter with no address -- where its siblings `event_with_filter`
/// and `event_for_name` both set one. That difference is invisible against
/// `anvil` and fatal against the endpoint the fleet runs on, which is why
/// this case asserts on the FILTER rather than only on the sync succeeding.
#[tokio::test]
async fn every_log_query_names_the_token_network_so_a_restricted_rpc_serves_it() {
    let rpc = AddressRestrictedRpc::start(100);
    let token_network: Address = DEVNET_TOKEN_NETWORK.parse().expect("parse address");

    let index = EvmChannelIndex::open(None, devnet_indexed_contract(token_network), 0)
        .expect("open in-memory index");
    let syncer = EvmChannelIndexSyncer::new(&rpc.url, token_network, 1, 0).expect("build syncer");

    let progressed = syncer
        .sync_once(&index)
        .await
        .expect("a scoped query is served rather than refused -32701");
    assert_eq!(
        progressed, 100,
        "blocks 0..=99 (head 100 less 1 confirmation)"
    );

    let filters = rpc.filters();
    assert_eq!(
        filters.len(),
        3,
        "one query per indexed event type: {filters:?}"
    );
    for filter in &filters {
        assert_eq!(
            filter["address"],
            serde_json::json!(DEVNET_TOKEN_NETWORK.to_lowercase()),
            "every query must be scoped to the TokenNetwork this syncer indexes: {filter}"
        );
    }
}

/// And the index really did take a checkpoint -- the thing that never
/// happened on either devnet box. Without it, a restart rescans from the
/// configured `from_block` and every lookup keeps paying for a direct chain
/// read.
#[tokio::test]
async fn a_served_query_leaves_a_checkpoint_behind() {
    let rpc = AddressRestrictedRpc::start(100);
    let token_network: Address = DEVNET_TOKEN_NETWORK.parse().expect("parse address");

    let index = EvmChannelIndex::open(None, devnet_indexed_contract(token_network), 0)
        .expect("open in-memory index");
    assert_eq!(index.last_indexed_block(), None);

    let syncer = EvmChannelIndexSyncer::new(&rpc.url, token_network, 1, 0).expect("build syncer");
    syncer.sync_once(&index).await.expect("sync_once");

    assert_eq!(index.last_indexed_block(), Some(99));
}
