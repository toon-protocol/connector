//! Both endpoints ride the `toon-zk-note-mina` circuit, and fail closed
//! (ADR 0080 decision 15, ADR 0073).

mod support;

use std::collections::HashMap;

use connector_chain_rpc::{Circuit, RpcTransport};
use connector_client_edge::zk_note::ZkNoteVault;
use connector_runtime::Socks5TestServer;
use connector_zk_note::Field;
use connector_zk_note_mina::MinaZkNoteVault;
use support::{recorded, StandIn};

const DAEMON_HOST: &str = "minadaemonminadaemonminadaemonminadaemonminadaemonminada.onion";
const ARCHIVE_HOST: &str = "minaarchiveminaarchiveminaarchiveminaarchiveminaarchive.onion";

fn consumed() -> Field {
    recorded()["named"]["consumedNullifier"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap()
}

fn through(url: &str, proxy: &url::Url) -> connector_chain_rpc::graphql::GraphqlRpc {
    connector_chain_rpc::graphql::GraphqlRpc::new(
        RpcTransport::through(url, proxy, Circuit::ZkNoteMina).expect("a circuit transport"),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn both_endpoints_dial_through_the_proxy_under_the_zk_note_circuit() {
    let (daemon, archive) = (StandIn::daemon().await, StandIn::archive().await);
    let proxy = Socks5TestServer::spawn(HashMap::from([
        (format!("{DAEMON_HOST}:80"), daemon.addr()),
        (format!("{ARCHIVE_HOST}:80"), archive.addr()),
    ]))
    .await;
    let vault = MinaZkNoteVault::new(
        support::VAULT.to_string(),
        through(&format!("http://{DAEMON_HOST}/"), &proxy.proxy_url()),
        through(&format!("http://{ARCHIVE_HOST}/"), &proxy.proxy_url()),
        support::verifier().await,
    );
    // `nullifier_used` reads both: the daemon's account, the archive's actions.
    assert_eq!(vault.nullifier_used(consumed()).await, Ok(true));

    let connects = proxy.connects();
    let targets: Vec<&str> = connects.iter().map(|c| c.target.as_str()).collect();
    assert!(
        targets.contains(&format!("{DAEMON_HOST}:80").as_str()),
        "{targets:?}"
    );
    assert!(
        targets.contains(&format!("{ARCHIVE_HOST}:80").as_str()),
        "{targets:?}"
    );
    assert!(
        connects
            .iter()
            .all(|c| c.username.as_deref() == Some("toon-zk-note-mina")),
        "every dial is on the zk-note circuit: {connects:?}"
    );
    assert_eq!(daemon.calls(), ["VaultState"]);
    assert_eq!(archive.calls(), ["VaultActions"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn with_the_proxy_down_nothing_reaches_either_endpoint_directly() {
    let (daemon, archive) = (StandIn::daemon().await, StandIn::archive().await);
    // A proxy that was up and is gone. The endpoints are plain loopback
    // URLs, so a client that fell back to a direct dial would reach them.
    let down = {
        let proxy = Socks5TestServer::spawn_recording_only().await;
        let url = proxy.proxy_url();
        drop(proxy);
        url
    };
    let vault = MinaZkNoteVault::new(
        support::VAULT.to_string(),
        through(&daemon.url(), &down),
        through(&archive.url(), &down),
        support::verifier().await,
    );
    assert!(vault.nullifier_used(consumed()).await.is_err());
    assert!(vault.creation_slot().await.is_err());
    assert!(vault.slot_anchor().await.is_err());
    assert_eq!(
        daemon.calls(),
        Vec::<String>::new(),
        "nothing reached the daemon"
    );
    assert_eq!(
        archive.calls(),
        Vec::<String>::new(),
        "nothing reached the archive"
    );
}

#[test]
fn the_circuit_has_a_username_of_its_own() {
    assert_eq!(Circuit::ZkNoteMina.socks_username(), "toon-zk-note-mina");
}
