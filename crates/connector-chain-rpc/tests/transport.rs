//! What a settlement table's transport does when its endpoint misbehaves,
//! and where its dials go (ADR 0073 decisions 2 to 5), against a real
//! socket: a scripted JSON-RPC server ([`FakeRpc`]) and ADR 0070's real
//! SOCKS5 server fake.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use connector_chain_rpc::evm::{answered, EvmRpc};
use connector_chain_rpc::solana::rpc_client;
use connector_chain_rpc::{Circuit, FakeRpc, Route, RpcReply, RpcTransport, Timeouts};
use connector_runtime::{Socks5TestServer, SocksConnect};
use ethers::providers::Middleware;
use serde_json::json;
use solana_rpc_client::rpc_client::RpcClientConfig;
use solana_sdk::commitment_config::CommitmentConfig;
use url::Url;

/// Bounds short enough that a test watches them fire.
const SHORT: Timeouts = Timeouts {
    connect: Duration::from_millis(500),
    request: Duration::from_millis(500),
    pool_idle: Duration::from_millis(500),
};

fn block_number(hex: &str) -> RpcReply {
    RpcReply::Result(json!(hex))
}

#[tokio::test]
async fn a_hung_endpoint_fails_within_the_request_timeout_instead_of_forever() {
    let rpc = FakeRpc::spawn(|_| RpcReply::Hang).await;
    let transport = RpcTransport::new(&rpc.url(), Route::Direct, SHORT).expect("transport");
    let provider = EvmRpc::provider(transport);

    let started = Instant::now();
    let error = provider
        .get_block_number()
        .await
        .expect_err("a hung endpoint cannot answer");

    assert!(
        started.elapsed() < Duration::from_secs(3),
        "the request timeout bounds the wait: took {:?}",
        started.elapsed()
    );
    assert!(
        !answered(&error),
        "a timeout is not the node's answer: {error}"
    );
}

#[tokio::test]
async fn the_settlement_bounds_are_the_ones_adr_0073_sized() {
    assert_eq!(Timeouts::SETTLEMENT.connect, Duration::from_secs(20));
    assert_eq!(Timeouts::SETTLEMENT.request, Duration::from_secs(30));
    assert!(Timeouts::SETTLEMENT.pool_idle <= Duration::from_secs(30));
}

#[tokio::test]
async fn an_evm_request_refused_with_403_or_429_is_retried_until_it_is_served() {
    for status in [403, 429] {
        let rpc = FakeRpc::spawn(move |call| {
            if call.nth < 2 {
                RpcReply::Status(status)
            } else {
                block_number("0x2a")
            }
        })
        .await;
        let provider = EvmRpc::provider(RpcTransport::direct(&rpc.url()).expect("transport"));

        let number = provider
            .get_block_number()
            .await
            .unwrap_or_else(|error| panic!("HTTP {status} is a refusal, not an answer: {error}"));
        assert_eq!(number.as_u64(), 42);
    }
}

#[tokio::test]
async fn an_evm_refusal_that_outlasts_its_retries_is_reported_as_one() {
    let rpc = FakeRpc::spawn(|_| RpcReply::Status(403)).await;
    let provider = EvmRpc::provider(RpcTransport::direct(&rpc.url()).expect("transport"));

    let error = provider
        .get_block_number()
        .await
        .expect_err("a permanent 403");
    assert!(!answered(&error));
    assert!(
        error.to_string().contains("HTTP 403"),
        "the refusal is named: {error}"
    );
}

#[tokio::test]
async fn a_json_rpc_error_is_the_nodes_answer_and_a_dropped_connection_is_not() {
    let rpc = FakeRpc::spawn(|call| match call.method.as_str() {
        "eth_blockNumber" => RpcReply::Error {
            code: -32000,
            message: "nonce too low".to_string(),
        },
        _ => RpcReply::Drop,
    })
    .await;
    let provider = EvmRpc::provider(RpcTransport::direct(&rpc.url()).expect("transport"));

    let answer = provider.get_block_number().await.expect_err("an error");
    assert!(answered(&answer), "{answer}");

    let lost = provider
        .get_chainid()
        .await
        .expect_err("a dropped connection");
    assert!(!answered(&lost), "{lost}");
}

#[tokio::test]
async fn a_null_result_is_an_answer_and_a_body_with_neither_result_nor_error_is_not() {
    let rpc = FakeRpc::spawn(|call| match call.method.as_str() {
        "eth_getTransactionReceipt" => RpcReply::Result(serde_json::Value::Null),
        _ => RpcReply::Status(500),
    })
    .await;
    let provider = EvmRpc::provider(RpcTransport::direct(&rpc.url()).expect("transport"));

    let receipt = provider
        .get_transaction_receipt(ethers::types::H256::zero())
        .await
        .expect("null is the node saying it has no receipt yet");
    assert!(receipt.is_none());
    assert!(provider.get_block_number().await.is_err());
}

#[tokio::test]
async fn the_solana_client_retries_a_403_its_sdk_would_have_returned_as_final() {
    let rpc = FakeRpc::spawn(|call| {
        if call.nth < 2 {
            RpcReply::Status(403)
        } else {
            RpcReply::Result(json!(7))
        }
    })
    .await;
    let transport = RpcTransport::direct(&rpc.url()).expect("transport");
    let client = rpc_client(
        &transport,
        RpcClientConfig::with_commitment(CommitmentConfig::confirmed()),
    );

    assert_eq!(client.get_slot().await.expect("served after two 403s"), 7);
}

#[tokio::test]
async fn the_solana_client_is_bounded_by_the_transport_too() {
    let rpc = FakeRpc::spawn(|_| RpcReply::Hang).await;
    let transport = RpcTransport::new(&rpc.url(), Route::Direct, SHORT).expect("transport");
    let client = rpc_client(
        &transport,
        RpcClientConfig::with_commitment(CommitmentConfig::confirmed()),
    );

    let started = Instant::now();
    assert!(client.get_slot().await.is_err());
    assert!(started.elapsed() < Duration::from_secs(3));
}

/// ADR 0073 decisions 2 and 3, end to end over a real SOCKS5 handshake.
///
/// Both endpoints are `.onion` names. No resolver on this machine resolves
/// one, so an answer at all proves the dial went through the proxy and went
/// as a name (`socks5h`). The proxy's own record then says which circuit
/// each chain's requests rode.
#[tokio::test]
async fn each_chain_dials_through_the_proxy_as_a_name_on_its_own_pinned_circuit() {
    let evm_rpc = FakeRpc::spawn(|_| block_number("0x2a")).await;
    let solana_rpc = FakeRpc::spawn(|_| RpcReply::Result(json!(7))).await;
    let evm_host = "evmsettlementrpcevmsettlementrpcevmsettlementrpcevmsett.onion";
    let solana_host = "solanasettlementrpcsolanasettlementrpcsolanasettlementr.onion";
    let proxy = Socks5TestServer::spawn(HashMap::from([
        (format!("{evm_host}:80"), evm_rpc.addr()),
        (format!("{solana_host}:80"), solana_rpc.addr()),
    ]))
    .await;

    let evm = RpcTransport::through(
        &format!("http://{evm_host}/"),
        &proxy.proxy_url(),
        Circuit::EvmSettlement,
    )
    .expect("evm transport");
    let solana = RpcTransport::through(
        &format!("http://{solana_host}/"),
        &proxy.proxy_url(),
        Circuit::SolanaSettlement,
    )
    .expect("solana transport");

    // Two EVM clients over one transport: the backend and the syncer (or the
    // rate source) share it.
    let backend = EvmRpc::provider(evm.clone());
    let syncer = EvmRpc::provider(evm);
    assert_eq!(
        backend
            .get_block_number()
            .await
            .expect("via proxy")
            .as_u64(),
        42
    );
    assert_eq!(
        syncer.get_block_number().await.expect("via proxy").as_u64(),
        42
    );
    let solana_client = rpc_client(
        &solana,
        RpcClientConfig::with_commitment(CommitmentConfig::confirmed()),
    );
    assert_eq!(solana_client.get_slot().await.expect("via proxy"), 7);

    let connects = proxy.connects();
    assert!(!connects.is_empty());
    for connect in &connects {
        let expected = if connect.target.starts_with(evm_host) {
            Circuit::EvmSettlement.socks_username()
        } else {
            Circuit::SolanaSettlement.socks_username()
        };
        assert_eq!(
            connect,
            &SocksConnect {
                username: Some(expected.to_string()),
                target: connect.target.clone(),
            },
            "every dial is pinned to its own chain's circuit"
        );
    }
    assert!(connects
        .iter()
        .any(|c| c.target == format!("{solana_host}:80")));
}

#[tokio::test]
async fn a_proxy_that_is_down_fails_the_request_and_nothing_is_dialed_direct() {
    let rpc = FakeRpc::spawn(|_| block_number("0x2a")).await;
    // A port nothing listens on: bind one, learn it, drop it.
    let dead = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        listener.local_addr().expect("addr")
    };
    let proxy = Url::parse(&format!("socks5h://{dead}")).expect("url");

    // The endpoint is reachable direct, so a fallback would succeed.
    let transport = RpcTransport::new(
        &rpc.url(),
        Route::Circuit {
            proxy,
            circuit: Circuit::EvmSettlement,
        },
        SHORT,
    )
    .expect("transport");
    let provider = EvmRpc::provider(transport);

    let error = provider
        .get_block_number()
        .await
        .expect_err("the proxy is down, and there is no other way out");
    assert!(!answered(&error));
    assert_eq!(rpc.calls().len(), 0, "nothing reached the endpoint direct");
}
