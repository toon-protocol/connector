//! **Which socket a dial left on** (ADR 0070 decision 3), for the BTP
//! carriage: an endpoint whose host ends in `.onion` goes through the
//! configured SOCKS5 proxy, and every other endpoint is dialed direct.
//!
//! The sibling carriage tests in `peer_carriage.rs` drive a
//! [`PeerDialer`] fake and say so: the socket is not their subject. This
//! file is the exception, and it has to be -- no configuration value can
//! answer "where did that connection go", so the assertion is made against
//! a **real SOCKS5 server** ([`Socks5TestServer`]) and a **real websocket
//! server**, both on loopback. Nothing here reaches a third-party network
//! and there is no onion daemon: a `.onion` name resolves nowhere, which is
//! exactly why the proxy is the only thing that can reach one, and why
//! finding that name in [`Socks5TestServer::targets`] proves both that the
//! dial traversed the proxy and that it deferred resolution to it
//! (`socks5h`).
//!
//! # Why the session, and not only the socket
//!
//! This carriage could not do what `connector-peer-http` does. That one
//! hands a proxy to its HTTP client; the websocket library here has no
//! proxy support at all, so the onion path establishes the SOCKS5 stream
//! itself and hands the **already-established stream** to the websocket
//! client. A test that only watched the CONNECT arrive would be green for a
//! stream nothing could speak over. So the third test below routes the
//! onion name through the proxy to a real BTP-speaking websocket server and
//! runs the same exchange twice -- once direct, once proxied -- against
//! that one server: the upgrade completes either way, and the RESPONSE
//! correlates to the MESSAGE's own `requestId` either way.
//!
//! # What is not proxied
//!
//! The proxy covers the ILP wire and nothing else (ADR 0070 decision 4).
//! Settlement RPC and the app's `handler_url` hold their own clients, and
//! both still dial direct. This file makes no claim about either.

use std::collections::HashMap;
use std::io::Write as _;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use chrono::{TimeZone, Utc};
use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::Message;
use url::Url;

use connector_btp::{decode_frame, encode_response, BtpFrame, BTP_RESPONSE};
use connector_config::Config;
use connector_domain::{PacketResponse, Prepare};
use connector_peer_btp::dial::PeerDialer;
use connector_peer_btp::{BtpPeerTransport, PeerRelation, TungsteniteDialer};
use connector_runtime::{PeerForward, PeerTransport, Socks5TestServer, SystemClock};

/// A v3 onion address's shape -- 56 base32 characters -- so the host under
/// test is one an operator could actually have copied out of a
/// `HiddenServiceDir/hostname` (ADR 0070 decision 7). No such service
/// exists; nothing in this file expects one to.
const ONION_HOST: &str = "toonexampleconnectoraddress234567abcdefghijklmnopqrstuvw.onion";

/// The CONNECT target a dial to `ws://<ONION_HOST>/btp` must name: the
/// **name**, and `ws://`'s default port, because nothing resolved it here.
fn onion_target() -> String {
    format!("{ONION_HOST}:80")
}

fn onion_endpoint() -> Url {
    Url::parse(&format!("ws://{ONION_HOST}/btp")).expect("an onion endpoint is a URL")
}

/// The same address as `anon` v0.4.10.2 writes it (issue #1284). The daemon
/// renamed the TLD it publishes and routes, and neither release resolves the
/// other's spelling -- so a dialer that selected the proxy for one and not
/// the other would dial an operator's own peer direct, at a name nothing on
/// this machine can resolve.
const ANYONE_HOST: &str = "toonexampleconnectoraddress234567abcdefghijklmnopqrstuvw.anyone";

fn anyone_target() -> String {
    format!("{ANYONE_HOST}:80")
}

fn anyone_endpoint() -> Url {
    Url::parse(&format!("ws://{ANYONE_HOST}/btp")).expect("an anyone endpoint is a URL")
}

/// The ILP-packet bytes the MESSAGE below carries. Not a real OER packet:
/// what this file asserts is that the *frame* crossed intact and correlated,
/// and `encode_message`/`decode_frame` are the same codec either way (the
/// packet's own grammar is `connector-domain`'s subject, not this one's).
const PACKET: &[u8] = b"an ILP packet's bytes";

/// A **real BTP-speaking websocket server** on loopback: it completes the
/// upgrade, then answers each MESSAGE with a RESPONSE under that frame's own
/// `requestId`, echoing the ILP bytes back.
///
/// Deliberately not a [`PeerDialer`] fake (ADR 0007): the question this file
/// asks is whether a session really ran over a stream the proxy established,
/// and an in-process fake could be green with no socket involved at all. It
/// upholds enough of the dialect for a dialed session to complete a request
/// against it, and asserts no sequence of calls -- what a test reads back is
/// what the wire carried.
struct BtpTestServer {
    addr: SocketAddr,
    /// The `Host` header of each completed upgrade, in arrival order.
    hosts: Arc<Mutex<Vec<String>>>,
    /// Every frame this server was sent, decoded.
    frames: Arc<Mutex<Vec<BtpFrame>>>,
    /// Dropped with the server, which ends the accept loop.
    _shutdown: tokio::sync::oneshot::Sender<()>,
}

impl BtpTestServer {
    async fn spawn() -> BtpTestServer {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("a loopback port for the BTP test server");
        let addr = listener.local_addr().expect("the bound address");
        let hosts = Arc::new(Mutex::new(Vec::new()));
        let frames = Arc::new(Mutex::new(Vec::new()));
        let (shutdown, mut stopped) = tokio::sync::oneshot::channel();

        let seen_hosts = Arc::clone(&hosts);
        let seen_frames = Arc::clone(&frames);
        tokio::spawn(async move {
            loop {
                let accepted = tokio::select! {
                    accepted = listener.accept() => accepted,
                    _ = &mut stopped => return,
                };
                let Ok((stream, _)) = accepted else { return };
                let seen_hosts = Arc::clone(&seen_hosts);
                let seen_frames = Arc::clone(&seen_frames);
                tokio::spawn(async move {
                    // A client that hung up mid-handshake hung up: there is
                    // nothing to record and nothing a test could do with a
                    // report of it.
                    let _ = serve_one(stream, seen_hosts, seen_frames).await;
                });
            }
        });

        BtpTestServer {
            addr,
            hosts,
            frames,
            _shutdown: shutdown,
        }
    }

    fn addr(&self) -> SocketAddr {
        self.addr
    }

    fn endpoint(&self) -> Url {
        Url::parse(&format!("ws://{}/btp", self.addr)).expect("a socket address is a URL")
    }

    fn hosts(&self) -> Vec<String> {
        self.hosts.lock().expect("hosts lock").clone()
    }

    fn frames(&self) -> Vec<BtpFrame> {
        self.frames.lock().expect("frames lock").clone()
    }
}

// Both `Result` shapes below are the websocket library's own -- the
// upgrade callback's return type is fixed by its `Callback` trait, and
// `tungstenite::Error` is the error every one of its calls returns. Neither
// is this file's to box.
#[allow(clippy::result_large_err)]
async fn serve_one(
    stream: TcpStream,
    hosts: Arc<Mutex<Vec<String>>>,
    frames: Arc<Mutex<Vec<BtpFrame>>>,
) -> Result<(), tokio_tungstenite::tungstenite::Error> {
    let recorded = Arc::clone(&hosts);
    let socket = tokio_tungstenite::accept_hdr_async(
        stream,
        move |request: &Request, response: Response| -> Result<Response, ErrorResponse> {
            let host = request
                .headers()
                .get("host")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string();
            recorded.lock().expect("hosts lock").push(host);
            Ok(response)
        },
    )
    .await?;
    let (mut sink, mut stream) = socket.split();

    while let Some(message) = stream.next().await {
        let Message::Binary(bytes) = message? else {
            continue;
        };
        let Ok(frame) = decode_frame(&bytes) else {
            continue;
        };
        // Answered under the frame's OWN `requestId`, so a caller that
        // correlates its answer has proved the round trip rather than
        // observed a coincidence.
        let answer = encode_response(frame.request_id, &[], &frame.ilp_packet);
        frames.lock().expect("frames lock").push(frame);
        sink.send(Message::Binary(answer)).await?;
    }
    Ok(())
}

/// A `.onion` endpoint on a dialer **with** a proxy leaves through that
/// proxy -- asserted by the dial arriving at the SOCKS5 server, not by any
/// call being recorded.
///
/// The proxy routes nothing, so the dial fails: that is the point. What is
/// under test is where the connection went, and a proxy that refuses the
/// CONNECT still records the target it was asked for.
#[tokio::test]
async fn an_onion_endpoint_is_dialed_through_the_configured_proxy() {
    let proxy = Socks5TestServer::spawn_recording_only().await;
    let proxy_url = proxy.proxy_url();
    let dialer = TungsteniteDialer::new().through_socks_proxy(Some(&proxy_url));

    let failed = dialer.dial("onion-peer", &onion_endpoint()).await;

    let error = failed.err().expect("the proxy routes nothing to this name");
    assert_eq!(error.peer_id, "onion-peer");
    assert_eq!(error.endpoint, onion_endpoint().to_string());
    assert_eq!(
        proxy.targets(),
        vec![onion_target()],
        "the dial reached the proxy, and reached it as a name -- `socks5h` defers resolution to \
         the proxy because no resolver here can resolve a .onion"
    );
}

/// A clearnet endpoint on the **same dialer** does not traverse the proxy.
///
/// Both dials are made through one [`TungsteniteDialer`], which is the whole
/// claim: host-selection *selects*, rather than merely permitting an onion
/// dial while quietly sending everything else the same way. The clearnet
/// endpoint is a real websocket server, so its dial genuinely succeeds and
/// genuinely carries a frame -- a test where neither dial reached anything
/// could be green for the wrong reason.
#[tokio::test]
async fn a_clearnet_endpoint_on_the_same_dialer_never_traverses_the_proxy() {
    let clearnet = BtpTestServer::spawn().await;
    let proxy = Socks5TestServer::spawn_recording_only().await;
    let proxy_url = proxy.proxy_url();
    let dialer = TungsteniteDialer::new().through_socks_proxy(Some(&proxy_url));

    let session = dialer
        .dial("clearnet-peer", &clearnet.endpoint())
        .await
        .expect("the clearnet dial is direct, and the server is really there");
    let answer = session
        .send_message(&[], PACKET)
        .await
        .expect("and the session over it carries a frame");
    assert_eq!(answer.ilp_packet, PACKET);

    // The same dialer, two endpoints later -- one per spelling the daemon
    // has published (issue #1284).
    let _ = dialer.dial("onion-peer", &onion_endpoint()).await;
    let _ = dialer.dial("anyone-peer", &anyone_endpoint()).await;

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
        "the clearnet server at {clearnet_addr} must never appear as a CONNECT target"
    );
}

/// The established stream really carries a BTP session, and one
/// indistinguishable from a direct one.
///
/// The onion name is routed through the SOCKS5 server to a real websocket
/// server, which is what an onion daemon does with a circuit. The same
/// exchange is then run twice against that one server -- once direct, once
/// through the proxy -- and both complete the upgrade and correlate their
/// RESPONSE to their own MESSAGE's `requestId`. Everything above the socket
/// module is untouched, and this is what says so.
#[tokio::test]
async fn a_btp_session_over_the_proxied_stream_behaves_as_a_direct_one() {
    let upstream = BtpTestServer::spawn().await;
    let proxy = Socks5TestServer::spawn(HashMap::from([(onion_target(), upstream.addr())])).await;
    let proxy_url = proxy.proxy_url();
    let dialer = TungsteniteDialer::new().through_socks_proxy(Some(&proxy_url));

    let direct = dialer
        .dial("clearnet-peer", &upstream.endpoint())
        .await
        .expect("the direct dial reaches the server")
        .send_message(&[], PACKET)
        .await
        .expect("a direct session answers");

    let proxied = dialer
        .dial("onion-peer", &onion_endpoint())
        .await
        .expect("the proxy routes this name to a server that is really there")
        .send_message(&[], PACKET)
        .await
        .expect("a proxied session answers exactly as the direct one did");

    for (what, answer) in [("direct", &direct), ("proxied", &proxied)] {
        assert_eq!(answer.frame_type, BTP_RESPONSE, "{what}");
        assert_eq!(
            answer.ilp_packet, PACKET,
            "{what}: the ILP bytes rode through byte for byte"
        );
    }
    let sent = upstream.frames();
    assert_eq!(sent.len(), 2, "one MESSAGE per session reached the server");
    assert_eq!(
        direct.request_id, sent[0].request_id,
        "the direct session's answer correlates to its own request"
    );
    assert_eq!(
        proxied.request_id, sent[1].request_id,
        "and so does the proxied one's -- correlation is not something the proxy touched"
    );

    assert!(proxy.saw_host_ending_in(".onion"));
    let hosts = upstream.hosts();
    assert_eq!(hosts.len(), 2, "two upgrades completed at the server");
    assert!(
        hosts.iter().any(|host| host.starts_with(ONION_HOST)),
        "the proxied upgrade is addressed to the onion host, which nothing on this machine \
         resolved: {hosts:?}"
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
    let refused = TungsteniteDialer::new()
        .dial("onion-peer", &onion_endpoint())
        .await
        .err()
        .expect("no proxy, no way to reach a .onion");

    assert_eq!(refused.peer_id, "onion-peer");
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

/// A node with **no** proxy configured, routed a packet for an onion BTP
/// peer, answers `T01` naming the peer and the endpoint it attempted --
/// never `T00`, and never a silent drop (`peer-carriage-spec.md` §2.2).
///
/// It is an ordinary dial failure and nothing more exotic: there is no
/// second refusal taxonomy for onion endpoints, and no boot-time error
/// either, because whether a remote is reachable is not locally detectable.
/// The endpoint and the absent proxy are both read off a **loaded config**,
/// since whether this node holds a proxy at all is a property of its
/// configuration and not one the carriage may decide for itself.
#[tokio::test]
async fn an_onion_peer_with_no_proxy_is_answered_t01_naming_the_peer_and_the_endpoint() {
    let (config, _state, _key) = onion_peer_config();
    assert!(
        config.socks_proxy().is_none(),
        "this node wrote no socks_proxy, which is what `build_peer_transport` reads"
    );
    let peer = config.peers().first().expect("the one peering");

    // `TungsteniteDialer::new()` with no proxy is exactly what
    // `build_peer_transport` constructs for a node whose `socks_proxy` is
    // `None`, and `PeerRelation::from_config` is the config's own answer
    // that this endpoint selects BTP (§2.1).
    let transport = BtpPeerTransport::new(
        Arc::new(TungsteniteDialer::new()),
        [0u8; 20],
        Arc::new(SystemClock),
    );
    transport.add_peer(
        PeerRelation::from_config(peer, config.peer_channels())
            .expect("a `ws://` onion endpoint selects the BTP carriage"),
    );

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

/// A node with one onion BTP peering and no `socks_proxy`, loaded through
/// the real loader -- `PeerConfig` is constructible no other way, and a
/// hand-built one would be a shape no node can hold.
///
/// `ws://` at a `.onion` host loads without `peer_allow_plaintext_endpoints`
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
endpoint = "ws://{ONION_HOST}/btp"

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
