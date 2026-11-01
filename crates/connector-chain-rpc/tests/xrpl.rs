//! The XRPL client's reading of a node's answers, against a scripted
//! endpoint ([`FakeRpc`]). The real node is `connector-settlement-xrpl`'s
//! `Xrpld` harness; these run without Docker, so the bounded-wait rule is
//! checked on every machine.

use std::time::Duration;

use connector_chain_rpc::xrpl::{TxFate, TxWait, XrplRpc, XrplRpcError};
use connector_chain_rpc::{FakeRpc, RpcReply, RpcTransport};
use serde_json::{json, Value};

fn success(mut body: Value) -> RpcReply {
    body["status"] = json!("success");
    RpcReply::Result(body)
}

fn error(code: &str) -> RpcReply {
    RpcReply::Result(json!({ "status": "error", "error": code, "error_message": code }))
}

fn client(fake: &FakeRpc) -> XrplRpc {
    XrplRpc::new(RpcTransport::direct(&fake.url()).expect("transport"))
}

const WAIT: TxWait = TxWait {
    poll: Duration::from_millis(1),
    give_up_after: Duration::from_secs(5),
};

fn server_info(validated: u32) -> RpcReply {
    success(json!({ "info": {
        "network_id": 1000, "server_state": "full", "validated_ledger": { "seq": validated }
    }}))
}

#[tokio::test]
async fn an_absent_account_and_an_absent_entry_read_as_none_not_as_failures() {
    let fake = FakeRpc::spawn(|call| match call.method.as_str() {
        "account_info" => error("actNotFound"),
        "ledger_entry" => error("entryNotFound"),
        "tx" => error("txnNotFound"),
        other => panic!("unexpected {other}"),
    })
    .await;
    let rpc = client(&fake);
    assert_eq!(rpc.account_info("rNobody").await.unwrap(), None);
    assert_eq!(rpc.ledger_entry(&"0".repeat(64)).await.unwrap(), None);
    assert_eq!(rpc.tx(&"0".repeat(64)).await.unwrap(), None);
}

#[tokio::test]
async fn any_other_node_error_is_the_nodes_answer_and_is_not_swallowed() {
    let fake = FakeRpc::spawn(|_| error("invalidParams")).await;
    let failed = client(&fake).account_info("rX").await.unwrap_err();
    assert_eq!(failed.node_error(), Some("invalidParams"));
}

#[tokio::test]
async fn server_info_reports_the_network_id_and_the_validated_ledger() {
    let fake = FakeRpc::spawn(|_| server_info(7)).await;
    let info = client(&fake).server_info().await.unwrap();
    assert_eq!(info.network_id, Some(1000));
    assert_eq!(info.validated_ledger, Some(7));
    assert_eq!(info.server_state, "full");
}

#[tokio::test]
async fn a_wait_ends_known_not_landed_once_a_validated_ledger_is_past_the_bound() {
    // The ledger advances one per `server_info`; the transaction is never found.
    let fake = FakeRpc::spawn(|call| match call.method.as_str() {
        "server_info" => server_info(10 + call.nth as u32),
        "tx" => error("txnNotFound"),
        other => panic!("unexpected {other}"),
    })
    .await;
    let fate = client(&fake).wait_for_tx("AB", 12, WAIT).await.unwrap();
    assert_eq!(
        fate,
        TxFate::KnownNotLanded {
            last_ledger_sequence: 12
        }
    );
    // Validated 10, 11, 12 are not past 12; 13 is.
    assert_eq!(fake.count("server_info"), 4);
}

#[tokio::test]
async fn a_transaction_validated_just_before_the_bound_passes_is_landed_not_lost() {
    // Validated is already past the bound on the first read, but the
    // transaction is in a validated ledger: it landed.
    let fake = FakeRpc::spawn(|call| match call.method.as_str() {
        "server_info" => server_info(50),
        "tx" => success(json!({
            "validated": true, "ledger_index": 12,
            "meta": { "TransactionResult": "tecNO_DST" }
        })),
        other => panic!("unexpected {other}"),
    })
    .await;
    let fate = client(&fake).wait_for_tx("AB", 12, WAIT).await.unwrap();
    assert!(matches!(
        fate,
        TxFate::Landed { ledger_index: 12, ref transaction_result, .. }
            if transaction_result == "tecNO_DST"
    ));
}

#[tokio::test]
async fn a_known_but_unvalidated_transaction_is_waited_on_until_the_bound() {
    let fake = FakeRpc::spawn(|call| match call.method.as_str() {
        "server_info" => server_info(5 + call.nth as u32),
        "tx" => success(json!({ "validated": false, "meta": {} })),
        other => panic!("unexpected {other}"),
    })
    .await;
    let fate = client(&fake).wait_for_tx("AB", 6, WAIT).await.unwrap();
    assert!(matches!(fate, TxFate::KnownNotLanded { .. }));
}

#[tokio::test]
async fn a_failed_poll_does_not_end_the_wait_for_a_transaction_that_may_land() {
    // The first two polls lose their answers; the transaction landed anyway.
    let fake = FakeRpc::spawn(|call| match call.method.as_str() {
        "server_info" if call.nth < 2 => RpcReply::Drop,
        "server_info" => server_info(12),
        "tx" => success(json!({
            "validated": true, "ledger_index": 11,
            "meta": { "TransactionResult": "tesSUCCESS" }
        })),
        other => panic!("unexpected {other}"),
    })
    .await;
    let fate = client(&fake).wait_for_tx("AB", 12, WAIT).await.unwrap();
    assert!(matches!(
        fate,
        TxFate::Landed {
            ledger_index: 11,
            ..
        }
    ));
}

#[tokio::test]
async fn an_endpoint_that_answers_while_its_ledger_stands_still_ends_the_wait() {
    // Every read answers, and the validated ledger never reaches the bound.
    let fake = FakeRpc::spawn(|call| match call.method.as_str() {
        "server_info" => server_info(7),
        "tx" => error("txnNotFound"),
        other => panic!("unexpected {other}"),
    })
    .await;
    let wait = TxWait {
        poll: Duration::from_millis(1),
        give_up_after: Duration::from_millis(50),
    };
    let ended = tokio::time::timeout(
        Duration::from_secs(10),
        client(&fake).wait_for_tx("AB", 12, wait),
    )
    .await
    .expect("the wait gives up rather than hanging");
    assert!(
        matches!(
            ended,
            Err(XrplRpcError::LedgerStalled {
                validated: Some(7),
                ..
            })
        ),
        "{ended:?}"
    );
}

#[tokio::test]
async fn an_endpoint_that_never_answers_ends_the_wait_with_its_error_not_a_hang() {
    let fake = FakeRpc::spawn(|_| RpcReply::Drop).await;
    let wait = TxWait {
        poll: Duration::from_millis(1),
        give_up_after: Duration::from_millis(50),
    };
    let ended = tokio::time::timeout(
        Duration::from_secs(10),
        client(&fake).wait_for_tx("AB", 12, wait),
    )
    .await
    .expect("the wait gives up rather than hanging");
    assert!(
        matches!(ended, Err(XrplRpcError::Transport { .. })),
        "{ended:?}"
    );
}
