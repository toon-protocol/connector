//! **Which socket a dial left on** (ADR 0070 decision 3): an endpoint whose
//! host ends in `.onion` goes through the configured SOCKS5 proxy, and every
//! other endpoint is dialed direct.
//!
//! The sibling carriage tests assert against a loaded `Config` and say in
//! their own headers that the socket is not exercised. This file is the
//! exception, and it has to be: no configuration value can answer "where did
//! that connection go", so the assertion is made against a **real SOCKS5
//! server** ([`Socks5TestServer`]) and a **real HTTP listener**, both on
//! loopback. Nothing here reaches a third-party network, and there is no
//! onion daemon: a `.onion` name resolves nowhere, which is exactly why the
//! proxy is the only thing that can reach one and why finding that name in
//! [`Socks5TestServer::targets`] proves both that the dial traversed the
//! proxy and that it deferred resolution to it (`socks5h`).
//!
//! Where a property *is* a property of configuration it is still asserted
//! against `Config::load` -- the last test below takes its onion endpoint off
//! a loaded config rather than inventing one, because an endpoint no node can
//! load is not an endpoint worth proving a reject for.
//!
//! # What is not proxied
//!
//! The proxy covers the ILP wire and nothing else (ADR 0070 decision 4).
//! Settlement RPC and the app's `handler_url` hold their own clients --
//! `connector-runtime`'s `outbound_client` and `HttpAppClient`, neither of
//! which this crate touches -- and both still dial direct. Routing settlement
//! through a circuit is a separate decision with its own evidence to gather,
//! and this file makes no claim about it.

use std::collections::HashMap;
use std::io::Write as _;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{TimeZone, Utc};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use url::Url;

use connector_config::Config;
use connector_domain::{PacketResponse, Prepare};
use connector_peer_http::{
    Headers, HttpPeerTransport, PeerHttpClient, PeerRelation, PeerRequest, ReqwestPeerClient,
};
use connector_runtime::{PeerForward, PeerTransport, Socks5TestServer, SystemClock};

/// A v3 onion address's shape -- 56 base32 characters -- so the host under
/// test is one an operator could actually have copied out of a
/// `HiddenServiceDir/hostname` (ADR 0070 decision 7). No such service exists;
/// nothing in this file expects one to.
const ONION_HOST: &str = "toonexampleconnectoraddress234567abcdefghijklmnopqrstuvw.onion";

/// The CONNECT target a dial to `http://<ONION_HOST>/ilp` must name: the
/// **name**, and the default HTTP port, because nothing resolved it here.
fn onion_target() -> String {
    format!("{ONION_HOST}:80")
}

fn onion_endpoint() -> Url {
    Url::parse(&format!("http://{ONION_HOST}/ilp")).expect("an onion endpoint is a URL")
}

/// The same address as `anon` v0.4.10.2 writes it (issue #1284). The daemon
/// renamed the TLD it publishes and routes, and neither release resolves the
/// other's spelling -- so a client that selected the proxy for one and not
/// the other would dial an operator's own peer direct, at a name nothing on
/// this machine can resolve.
const ANYONE_HOST: &str = "toonexampleconnectoraddress234567abcdefghijklmnopqrstuvw.anyone";

fn anyone_target() -> String {
    format!("{ANYONE_HOST}:80")
}

fn anyone_endpoint() -> Url {
    Url::parse(&format!("http://{ANYONE_HOST}/ilp")).expect("an anyone endpoint is a URL")
}

/// A PREPARE-shaped request body plus one §3 header, so what arrives at the
/// far end is recognisable as this carriage's POST rather than as any byte
/// this test happened to write.
fn request() -> PeerRequest {
    let mut headers = Headers::new();
    headers.push("toon-peer-id", "onion-peer");
    PeerRequest {
        headers,
        body: b"an ILP packet's bytes".to_vec(),
    }
}

/// What the listener below answers every request with.
const PONG: &[u8] = b"pong";

/// One request as the listener saw it on the wire.
struct SeenRequest {
    /// The request head, lowercased -- header names are case-insensitive and
    /// nothing here asserts on a value whose case matters.
    head: String,
    body: Vec<u8>,
}

/// A **real HTTP/1.1 listener** on loopback: it reads one request per
/// connection, records it, and answers `200` with [`PONG`].
///
/// Deliberately not a stub of `PeerHttpClient` (ADR 0007): the question this
/// file asks is whether bytes crossed a socket, and a fake that answered in
/// process could be green with no socket involved at all. It upholds enough
/// of HTTP/1.1 for `reqwest` to complete a request against it and asserts no
/// sequence of calls -- what a test reads back is the request the wire
/// carried.
struct HttpTestListener {
    addr: SocketAddr,
    seen: Arc<Mutex<Vec<SeenRequest>>>,
    /// Dropped with the listener, which ends the accept loop.
    _shutdown: tokio::sync::oneshot::Sender<()>,
}

impl HttpTestListener {
    async fn spawn() -> HttpTestListener {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("a loopback port for the HTTP test listener");
        let addr = listener.local_addr().expect("the bound address");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (shutdown, mut stopped) = tokio::sync::oneshot::channel();

        let recorded = Arc::clone(&seen);
        tokio::spawn(async move {
            loop {
                let accepted = tokio::select! {
                    accepted = listener.accept() => accepted,
                    _ = &mut stopped => return,
                };
                let Ok((stream, _)) = accepted else { return };
                let recorded = Arc::clone(&recorded);
                tokio::spawn(async move {
                    // A client that hung up mid-request hung up: there is
                    // nothing to record and nothing a test could do with a
                    // report of it.
                    let _ = answer_one(stream, recorded).await;
                });
            }
        });

        HttpTestListener {
            addr,
            seen,
            _shutdown: shutdown,
        }
    }

    fn addr(&self) -> SocketAddr {
        self.addr
    }

    fn endpoint(&self) -> Url {
        Url::parse(&format!("http://{}/ilp", self.addr)).expect("a socket address is a URL")
    }
}

async fn answer_one(
    mut stream: TcpStream,
    seen: Arc<Mutex<Vec<SeenRequest>>>,
) -> std::io::Result<()> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 1024];
    // A request head ends at the blank line; the body's length is whatever
    // `content-length` said, which `reqwest` always sends for a sized body.
    let head_end = loop {
        if let Some(at) = buffer
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .map(|at| at + 4)
        {
            break at;
        }
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Ok(());
        }
        buffer.extend_from_slice(&chunk[..read]);
    };
    let head = String::from_utf8_lossy(&buffer[..head_end]).to_lowercase();
    let length = head
        .split("content-length:")
        .nth(1)
        .and_then(|rest| rest.split("\r\n").next())
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    while buffer.len() < head_end + length {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    seen.lock().expect("seen lock").push(SeenRequest {
        head,
        body: buffer[head_end..].to_vec(),
    });

    stream
        .write_all(
            format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/octet-stream\r\ncontent-length: \
                 {}\r\nconnection: close\r\n\r\n",
                PONG.len()
            )
            .as_bytes(),
        )
        .await?;
    stream.write_all(PONG).await
}

/// A `.onion` endpoint on a node **with** a proxy configured leaves through
/// that proxy -- asserted by the dial arriving at the SOCKS5 server, not by
/// any call being recorded.
///
/// The proxy routes nothing, so the dial fails: that is the point. What is
/// under test is where the connection went, and a proxy that refuses the
/// CONNECT still records the target it was asked for.
#[tokio::test]
async fn an_onion_endpoint_is_dialed_through_the_configured_proxy() {
    let proxy = Socks5TestServer::spawn_recording_only().await;
    let client = ReqwestPeerClient::through_socks_proxy(&proxy.proxy_url());

    let failed = client.post(&onion_endpoint(), request()).await;

    assert!(
        failed.is_err(),
        "the proxy routes nothing, so the dial cannot succeed: {failed:?}"
    );
    assert_eq!(
        proxy.targets(),
        vec![onion_target()],
        "the dial reached the proxy, and reached it as a name -- `socks5h` defers resolution to \
         the proxy because no resolver here can resolve a .onion"
    );
}

/// A clearnet endpoint on the **same client** does not traverse the proxy.
///
/// Both dials are made through one `ReqwestPeerClient`, which is the whole
/// claim: host-selection *selects*, rather than merely permitting an onion
/// dial while quietly sending everything else the same way. The clearnet
/// endpoint is a real listener, so its dial genuinely succeeds -- a test
/// where neither dial reached anything could be green for the wrong reason.
#[tokio::test]
async fn a_clearnet_endpoint_on_the_same_node_never_traverses_the_proxy() {
    let clearnet = HttpTestListener::spawn().await;
    let proxy = Socks5TestServer::spawn_recording_only().await;
    let client = ReqwestPeerClient::through_socks_proxy(&proxy.proxy_url());

    let answered = client
        .post(&clearnet.endpoint(), request())
        .await
        .expect("the clearnet dial is direct, and the listener is really there");
    assert_eq!(answered.status, 200);
    assert_eq!(answered.body, PONG);

    // The same client, two endpoints later -- one per spelling the daemon
    // has published (issue #1284).
    let _ = client.post(&onion_endpoint(), request()).await;
    let _ = client.post(&anyone_endpoint(), request()).await;

    assert_eq!(
        proxy.targets(),
        vec![onion_target(), anyone_target()],
        "both hidden-service endpoints traversed the proxy, in the order they were dialed, and \
         the clearnet one was never offered to it"
    );
    let clearnet_addr = clearnet.addr().to_string();
    assert!(
        !proxy
            .targets()
            .iter()
            .any(|target| target.contains(&clearnet_addr)),
        "the clearnet listener at {clearnet_addr} must never appear as a CONNECT target"
    );
}

/// The proxy really carried the bytes, end to end.
///
/// The onion name is routed through the SOCKS5 server to a real HTTP
/// listener, which is what an onion daemon does with a circuit. The POST
/// arrives there -- addressed to the onion host it was never able to resolve
/// -- and the listener's answer comes back to the client.
#[tokio::test]
async fn the_proxy_carries_the_whole_post_to_the_onion_target() {
    let upstream = HttpTestListener::spawn().await;
    let proxy = Socks5TestServer::spawn(HashMap::from([(onion_target(), upstream.addr())])).await;
    let client = ReqwestPeerClient::through_socks_proxy(&proxy.proxy_url());

    let answered = client
        .post(&onion_endpoint(), request())
        .await
        .expect("the proxy routes this name to a listener that is really there");

    assert_eq!(answered.status, 200);
    assert_eq!(
        answered.body, PONG,
        "the answer came back through the proxy"
    );
    assert!(proxy.saw_host_ending_in(".onion"));

    let seen = upstream.seen.lock().expect("seen lock");
    assert_eq!(seen.len(), 1, "exactly one POST arrived");
    assert_eq!(
        seen[0].body,
        request().body,
        "the ILP body rode through byte for byte"
    );
    assert!(
        seen[0].head.contains(&format!("host: {ONION_HOST}")),
        "the request is addressed to the onion host, which nothing on this machine resolved: {}",
        seen[0].head
    );
    assert!(
        seen[0].head.contains("toon-peer-id: onion-peer"),
        "and the §3 headers rode with it: {}",
        seen[0].head
    );
}

/// With no proxy configured the onion dial is **refused before any socket**,
/// and says why.
///
/// This is what keeps the reject below honest: without it the same `T01`
/// would arrive by way of a local DNS lookup for a `.onion` name, which is
/// both a resolver error an operator cannot act on and a lookup this node
/// should never have made. The reason names the config key, because that is
/// the one thing the operator has to change.
#[tokio::test]
async fn with_no_proxy_an_onion_dial_is_refused_before_any_lookup() {
    let refused = ReqwestPeerClient::default()
        .post(&onion_endpoint(), request())
        .await
        .expect_err("no proxy, no way to reach a .onion");

    assert_eq!(refused.endpoint, onion_endpoint().to_string());
    assert!(
        refused.reason.contains("socks_proxy"),
        "the refusal names the key that would fix it: {}",
        refused.reason
    );
    assert!(
        !refused.reason.contains("dns"),
        "and it is this node's own refusal, not a resolver's answer about a name no resolver \
         should have been asked for: {}",
        refused.reason
    );
}

/// A node with **no** proxy configured, routed a packet for an onion peer,
/// answers `T01` naming the peer and the endpoint it attempted -- never
/// `T00`, and never a silent drop (`peer-carriage-spec.md` §2.2).
///
/// It is an ordinary dial failure and nothing more exotic: there is no
/// second refusal taxonomy for onion endpoints, and no boot-time error
/// either, because whether a remote is reachable is not locally detectable.
/// The endpoint and the absent proxy are both read off a **loaded config**,
/// since which of the two clients this node holds is a property of its
/// configuration and not one the carriage may decide for itself.
#[tokio::test]
async fn an_onion_peer_with_no_proxy_is_answered_t01_naming_the_peer_and_the_endpoint() {
    let (config, _state, _key) = onion_peer_config();
    assert!(
        config.socks_proxy().is_none(),
        "this node wrote no socks_proxy, which is what `build_peer_transport` reads"
    );
    let peer = config.peers().first().expect("the one peering");
    let endpoint = peer.endpoint().expect("a dialable onion endpoint").clone();

    // `ReqwestPeerClient::default()` is exactly what `build_peer_transport`
    // constructs for a node whose `socks_proxy` is `None`.
    let transport = HttpPeerTransport::new(
        Arc::new(ReqwestPeerClient::default()),
        [0u8; 20],
        Arc::new(SystemClock),
    );
    transport.add_peer(PeerRelation::new(
        peer.id(),
        endpoint.clone(),
        HashMap::new(),
        HashMap::new(),
        Duration::from_secs(2),
        Duration::from_secs(2),
    ));

    let PeerForward {
        response,
        reached_peer,
        ..
    } = transport
        .forward(
            peer.id(),
            Prepare {
                amount: 100,
                expires_at: Utc.with_ymd_and_hms(2031, 1, 1, 0, 0, 0).unwrap(),
                greeting: false,
                destination: "g.example.app".to_string(),
                data: Vec::new(),
            },
            None,
        )
        .await;

    match response {
        PacketResponse::Reject(reject) => {
            assert_eq!(reject.code.as_str(), "T01");
            assert!(
                reject.message.contains("onion-peer"),
                "the reject names the peer: {}",
                reject.message
            );
            assert!(
                reject.message.contains(ONION_HOST),
                "and the endpoint it attempted: {}",
                reject.message
            );
        }
        other => panic!("expected a T01 reject, got {other:?}"),
    }
    assert!(!reached_peer, "nothing was reached, which is the point");
}

/// A node with one onion peering and no `socks_proxy`, loaded through the
/// real loader -- `PeerConfig` is constructible no other way, and a
/// hand-built one would be a shape no node can hold.
///
/// `http://` at a `.onion` host loads without `peer_allow_plaintext_endpoints`
/// (ADR 0070 decision 2): the address *is* an ed25519 public key, so the
/// circuit is authenticated to it, and that is a narrower and stronger
/// binding than the global plaintext switch this config never sets.
fn onion_peer_config() -> (Config, tempfile::TempDir, tempfile::NamedTempFile) {
    let mut key_file = tempfile::NamedTempFile::new().expect("temp key file");
    key_file.write_all(&[7u8; 32]).expect("write key file");
    let state_dir = tempfile::tempdir().expect("temp state dir");
    let mut config_file = tempfile::NamedTempFile::new().expect("temp config file");
    write!(
        config_file,
        r#"
client_edge_addr = "127.0.0.1:0"
state_dir = "{state_dir}"

[signer]
key_file = "{key_file}"

# An EVM `[[peer_channels]]` row needs `[settlement.evm]`: that table is
# where this node's EVM address comes from, and a peer claim is redeemed by
# the channel's on-chain participant.
[settlement.evm]
rpc_url = "http://127.0.0.1:8545"
contract_address = "0x1234567890123456789012345678901234567890"
token_address = "0x49beE1Bca5d15Fb0963117923403F9498119a9Ce"
decimals = 6

[settlement.evm.key]
key_file = "{key_file}"

[[peers]]
id = "onion-peer"
endpoint = "http://{ONION_HOST}/ilp"

[[peer_channels]]
peer_id = "onion-peer"
channel_id = "0x1111111111111111111111111111111111111111111111111111111111111111"
counterparty_key = "0x2222222222222222222222222222222222222222"
chain_id = 31337
token_network = "0x3333333333333333333333333333333333333333"
"#,
        state_dir = state_dir.path().display(),
        key_file = key_file.path().display(),
    )
    .expect("write config file");
    let config = Config::load(config_file.path()).expect("an onion peering loads");
    (config, state_dir, key_file)
}
