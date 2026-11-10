//! What the adapter does when the chain's endpoints are wrong, late or gone:
//! each refuses the one read, by name, and the next read is served (ADR 0080
//! decisions 9 and 15). A stale archive in particular must not read as
//! "nullifier unused".

mod support;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use connector_client_edge::zk_note::{ZkNoteVault, ZkNoteVaultUnavailable};
use connector_zk_note::Field;
use serde_json::{json, Value};
use support::{recorded, vault_over, Reply, StandIn, SHORT};

fn field(text: &str) -> Field {
    text.parse().expect("a field element")
}

fn consumed() -> Field {
    field(recorded()["named"]["consumedNullifier"].as_str().unwrap())
}

fn reason(error: ZkNoteVaultUnavailable) -> String {
    error.0
}

async fn archive_of(actions: Value) -> StandIn {
    StandIn::spawn(move |_| Reply::Json(json!({ "data": actions.clone() }))).await
}

#[tokio::test(flavor = "multi_thread")]
async fn an_archive_behind_the_daemon_is_a_failed_read_not_an_unused_nullifier() {
    let daemon = StandIn::daemon().await;
    let mut actions = recorded()["actions"]["data"].clone();
    // The archive has not seen the escape that spent the nullifier.
    actions["actions"].as_array_mut().unwrap().pop();
    let archive = archive_of(actions).await;
    let vault = vault_over(&daemon, &archive, SHORT).await;
    let error = vault.nullifier_used(consumed()).await.unwrap_err();
    assert!(reason(error).contains("behind the daemon"));
    // The node keeps serving: reads that don't need the archive are unharmed.
    assert_eq!(
        vault.creation_slot().await,
        Ok(connector_zk_note::Slot::new(1049))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_inconsistent_archive_is_a_failed_read() {
    let daemon = StandIn::daemon().await;
    let mut actions = recorded()["actions"]["data"].clone();
    actions["actions"].as_array_mut().unwrap().remove(0);
    let archive = archive_of(actions).await;
    let vault = vault_over(&daemon, &archive, SHORT).await;
    let error = vault.nullifier_used(consumed()).await.unwrap_err();
    let why = reason(error);
    assert!(why.contains("incomplete or out of order"), "{why}");

    let mut forged = recorded()["actions"]["data"].clone();
    forged["actions"][2]["actionData"][0]["data"][2] = json!("7");
    let archive = archive_of(forged).await;
    let vault = vault_over(&daemon, &archive, SHORT).await;
    assert!(vault.nullifier_used(consumed()).await.is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_archive_ahead_of_the_daemon_only_knows_more() {
    // The daemon was read when the vault had seen two actions; the archive
    // already lists the escape.
    let mut state = recorded()["vaultState"].clone();
    let second =
        recorded()["actions"]["data"]["actions"][1]["actionState"]["actionStateOne"].clone();
    state["data"]["account"]["actionState"][0] = second;
    let daemon = StandIn::spawn(move |_| Reply::Json(state.clone())).await;
    let archive = StandIn::archive().await;
    let vault = vault_over(&daemon, &archive, SHORT).await;
    assert_eq!(vault.nullifier_used(consumed()).await, Ok(true));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_slow_read_is_refused_within_the_bound_and_the_next_one_is_served() {
    let state = recorded()["vaultState"].clone();
    let asked = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&asked);
    let daemon = StandIn::spawn(move |_| {
        if counter.fetch_add(1, Ordering::SeqCst) == 0 {
            Reply::Hang
        } else {
            Reply::Json(state.clone())
        }
    })
    .await;
    let archive = StandIn::archive().await;
    let vault = vault_over(&daemon, &archive, SHORT).await;
    let root = field(
        recorded()["vaultState"]["data"]["account"]["zkappState"][2]
            .as_str()
            .unwrap(),
    );
    let started = Instant::now();
    let error = vault.root_known(root).await.unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(5), "bounded");
    assert!(reason(error).contains("did not answer"));
    assert_eq!(
        vault.root_known(root).await,
        Ok(true),
        "the node keeps serving"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_down_endpoint_refuses_the_read_naming_it() {
    // A port nobody listens on.
    let gone = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", listener.local_addr().unwrap())
    };
    let archive = StandIn::archive().await;
    let daemon = StandIn::daemon().await;
    let down_daemon = connector_zk_note_mina::MinaZkNoteVault::new(
        support::VAULT.to_string(),
        support::direct(&gone, SHORT),
        support::direct(&archive.url(), SHORT),
        support::verifier().await,
    );
    let error = down_daemon.root_known(Field::from(1u64)).await.unwrap_err();
    assert!(reason(error).contains("127.0.0.1"));
    let down_archive = connector_zk_note_mina::MinaZkNoteVault::new(
        support::VAULT.to_string(),
        support::direct(&daemon.url(), SHORT),
        support::direct(&gone, SHORT),
        support::verifier().await,
    );
    assert!(down_archive.nullifier_used(consumed()).await.is_err());
    // The daemon's reads still answer while the archive is down.
    assert_eq!(
        down_archive.creation_slot().await,
        Ok(connector_zk_note::Slot::new(1049))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_answer_that_is_not_the_vault_is_a_failed_read() {
    for answer in [
        json!({ "data": { "account": null } }),
        json!({ "errors": [{ "message": "no such account" }] }),
        json!({ "data": { "account": { "zkappState": ["1"], "actionState": [] } } }),
        json!("not graphql"),
    ] {
        let daemon = StandIn::spawn(move |_| Reply::Json(answer.clone())).await;
        let archive = StandIn::archive().await;
        let vault = vault_over(&daemon, &archive, SHORT).await;
        assert!(vault.creation_slot().await.is_err());
        assert!(vault.root_known(Field::from(1u64)).await.is_err());
    }
    let daemon = StandIn::spawn(|_| Reply::Status(500)).await;
    let archive = StandIn::archive().await;
    let vault = vault_over(&daemon, &archive, SHORT).await;
    assert!(vault.slot_anchor().await.is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn zero_is_never_a_known_root() {
    // An empty ring slot holds zero, which is no root.
    let (daemon, archive) = (StandIn::daemon().await, StandIn::archive().await);
    let vault = vault_over(&daemon, &archive, SHORT).await;
    assert_eq!(vault.root_known(Field::from(0u64)).await, Ok(false));
}
