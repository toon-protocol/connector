//! The websocket underneath a dialed peering: the one place in this crate
//! that touches a socket.
//!
//! Everything else here is provable without TLS, a listener or a port
//! number, because [`crate::dial::PeerDialer`] is a port. This module is
//! its production implementation over `tokio-tungstenite` -- the same 0.20
//! the client edge's own BTP integration test already speaks.
//!
//! # `wss://`, and `ws://` only where an operator has opted in
//!
//! A peering carries signed balance proofs (ADR 0004), so the scheme that
//! selects this carriage in any deployed config is `wss://`. `ws://` is
//! [`connector_config::ConfigError::PeerEndpointScheme`] at load unless
//! the node set `peer_allow_plaintext_endpoints` (issue #678's gap 3) --
//! or unless the endpoint's host ends in `.onion`, which permits the
//! plaintext schemes on its own (ADR 0070 decision 2) because the address
//! *is* the ed25519 key the circuit is authenticated to. Either way it
//! names the same carriage without TLS and reaches here, and everything
//! above this line is identical. `local/mixed-chain`'s `a-b` peering is
//! the opt-in case, and since issue #1155 it is how the shipped image is
//! proven to carry a packet over BTP at all; a node taking the opt-in logs
//! a WARN naming every such peering at startup, `ws://` and `http://`
//! alike.
//!
//! # Which socket the bytes leave on (ADR 0070 decision 3)
//!
//! An endpoint whose host ends in `.onion` is dialed **through this node's
//! one configured SOCKS5 proxy**; every other endpoint is dialed direct.
//! The address decides and nothing else does: there is no per-peer key
//! saying which, and no mode that reroutes everything.
//!
//! This carriage cannot do that the way `connector-peer-http` does. That
//! one hands a proxy to its HTTP client and is finished; **the websocket
//! library this crate speaks has no proxy support at all**. So the onion
//! path establishes the SOCKS5 stream itself -- a `TcpStream` to the
//! proxy, then a CONNECT naming the endpoint's host *as a name* -- and
//! hands the already-established stream to the websocket client. The `h`
//! in `socks5h` is the whole of why the CONNECT carries a name: no
//! resolver on this machine can resolve a `.onion`, so resolution has to
//! happen at the proxy, and a local lookup of one is the bug this design
//! exists to avoid.
//!
//! Only the entry point differs. Both paths end in [`start_session`], so
//! the frames, the one writer, the correlation table and §2.3's symmetry
//! rule are the same code on a proxied session as on a direct one.
//!
//! # Symmetry, without inferring a role from having dialed (§2.3, §1.3)
//!
//! On BTP a session is symmetric once established: after auth **either**
//! side may originate a MESSAGE or a TRANSFER on the one session. So a
//! dialed session must be able to *serve* as well as ask, and it does --
//! inbound frames that are not answers to our own requests are handed to a
//! [`PeerSession`] over the same socket, sharing its correlation table so
//! one read loop serves both halves.
//!
//! That session starts, and stays, `client` until the far side presents its
//! own credential. **Having dialed a peer is not evidence that the peer on
//! the other end is a peer** -- §1.3 forbids inferring role from the
//! carriage, the endpoint or anything the interaction did earlier, and "we
//! are the ones who opened this socket" is exactly that kind of inference.
//! The credential decides, in the direction it is presented, or the role is
//! `client`. Having dialed a peer *through a circuit* is no more evidence
//! than having dialed one directly, so the proxied path adds nothing to
//! that decision either.

use std::sync::Arc;

use async_trait::async_trait;
use connector_btp::{decode_frame, BtpSessionHandle, OutboundRequests};
use connector_config::is_onion_endpoint;
use connector_runtime::NO_SOCKS_PROXY;
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_socks::tcp::Socks5Stream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;
use url::Url;

use crate::accept::{PeerCarriageState, PeerSession, REPLY_QUEUE_DEPTH};
use crate::dial::{DialError, PeerDialer};

/// The port a `socks_proxy` with no port in it is reached on: IANA's
/// registered SOCKS port.
///
/// `Config::load` requires the scheme and a host but not a port, so this
/// fills the gap -- and it fills it with the same number `reqwest` fills it
/// with on the HTTP carriage, so one `socks_proxy` value reaches one proxy
/// whichever carriage reads it (§9).
const DEFAULT_SOCKS_PORT: u16 = 1080;

/// Dials `wss://` peer endpoints over `tokio-tungstenite`, and onion ones
/// through a SOCKS5 proxy (ADR 0070 decision 3).
pub struct TungsteniteDialer {
    /// The pipeline an inbound, peer-originated frame on a dialed session
    /// reaches (§2.3). `None` means this connector only ever asks on the
    /// sessions it dials: answers still correlate, and anything else the
    /// far side sends is dropped rather than served.
    inbound: Option<Arc<PeerCarriageState>>,
    /// This node's one `socks_proxy`, or `None` on a node that configured
    /// none -- which is the default, and is not an error condition until an
    /// onion endpoint is actually dialed.
    socks_proxy: Option<Url>,
}

impl TungsteniteDialer {
    /// A dialer whose sessions ask and never serve.
    #[must_use]
    pub fn new() -> Self {
        TungsteniteDialer {
            inbound: None,
            socks_proxy: None,
        }
    }

    /// A dialer whose sessions are symmetric (§2.3): a MESSAGE or TRANSFER
    /// the far side originates reaches `state`'s pipeline, judged by the
    /// same rules an inbound session's frames are.
    #[must_use]
    pub fn serving(state: Arc<PeerCarriageState>) -> Self {
        TungsteniteDialer {
            inbound: Some(state),
            socks_proxy: None,
        }
    }

    /// The same dialer, dialing onion endpoints through `proxy` (ADR 0070
    /// decision 3). `None` leaves it dialing everything direct, so a call
    /// site can pass `Config::socks_proxy` straight through without a
    /// second decision about what its absence means.
    ///
    /// A combinator rather than a fourth constructor: the proxy is
    /// orthogonal to whether a dialed session serves, and spelling the
    /// product of the two axes as four constructors is how one of the four
    /// ends up forgotten at a call site. `proxy` is validated by
    /// [`Config::load`], which refuses anything that is not `socks5h://`
    /// with a host -- the `h` is not a preference: it is what makes the
    /// *proxy* resolve the name, and no resolver here can resolve a
    /// `.onion` one.
    ///
    /// [`Config::load`]: connector_config::Config::load
    #[must_use]
    pub fn through_socks_proxy(mut self, proxy: Option<&Url>) -> Self {
        self.socks_proxy = proxy.cloned();
        self
    }

    /// A SOCKS5 stream to `endpoint`, established through this node's
    /// proxy, or the reason there is none.
    ///
    /// The CONNECT names the endpoint's **host as a name** and never an
    /// address: `.onion` resolves nowhere locally, so a client that
    /// resolved first would fail before the proxy ever heard the request,
    /// and one that resolved at all would have leaked the lookup.
    async fn socks5_stream(&self, endpoint: &Url) -> Result<Socks5Stream<TcpStream>, String> {
        let Some(proxy) = self.socks_proxy.as_ref() else {
            return Err(NO_SOCKS_PROXY.to_string());
        };
        // `Config::load` has already refused a `socks_proxy` with no host,
        // so this is only reachable through a hand-built `Url`; it is an
        // ordinary dial failure rather than a panic, because a dialer is
        // public API and a bad value here must not take the process down.
        let proxy_host = proxy
            .host_str()
            .ok_or_else(|| format!("socks_proxy '{proxy}' names no host to reach it at"))?;
        let proxy_port = proxy.port().unwrap_or(DEFAULT_SOCKS_PORT);
        let socket = TcpStream::connect((proxy_host, proxy_port))
            .await
            .map_err(|error| format!("socks_proxy '{proxy}' could not be reached: {error}"))?;

        let host = endpoint
            .host_str()
            .ok_or_else(|| format!("endpoint '{endpoint}' names no host"))?;
        // `ws://` is 80 and `wss://` is 443 where the URL states neither --
        // an onion endpoint is dialed on whatever port its operator wrote
        // down, and the proxy is told that port verbatim.
        let port = endpoint
            .port_or_known_default()
            .ok_or_else(|| format!("endpoint '{endpoint}' names no port"))?;
        Socks5Stream::connect_with_socket(socket, (host, port))
            .await
            .map_err(|error| {
                format!("the SOCKS5 CONNECT to {host}:{port} through '{proxy}' failed: {error}")
            })
    }

    /// Everything a dialed session is, once there is a websocket to carry
    /// it: the one writer, the correlation table, and §2.3's read loop.
    ///
    /// Generic over the stream underneath so the proxied and direct paths
    /// are the *same* session code rather than two copies that could
    /// drift. A session over a SOCKS5 stream must behave exactly as one
    /// over a direct socket does -- same frames, same correlation, same
    /// role rules -- and the cheapest way to hold that is for there to be
    /// only one implementation of it.
    fn start_session<S>(&self, socket: WebSocketStream<S>) -> BtpSessionHandle
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let (mut sink, mut stream) = socket.split();

        // The one writer. Frames complete in whatever order their
        // downstream answers, and this channel is where those completions
        // serialize back into socket writes.
        let (replies, mut reply_rx) = mpsc::channel::<Vec<u8>>(REPLY_QUEUE_DEPTH);
        tokio::spawn(async move {
            while let Some(bytes) = reply_rx.recv().await {
                if sink.send(Message::Binary(bytes)).await.is_err() {
                    break;
                }
            }
        });

        let outbound = Arc::new(OutboundRequests::new());
        let handle = BtpSessionHandle::new(replies.clone(), Arc::clone(&outbound));

        match self.inbound.as_ref() {
            // Symmetric: one read loop, feeding the session that both
            // correlates our answers and serves the far side's requests.
            Some(state) => {
                let mut session =
                    PeerSession::with_outbound(Arc::clone(state), replies, Arc::clone(&outbound));
                tokio::spawn(async move {
                    while let Some(Ok(Message::Binary(bytes))) = stream.next().await {
                        if session.handle_frame(&bytes).await.is_err() {
                            break;
                        }
                    }
                });
            }
            // Ask-only: answers correlate, everything else is dropped --
            // byte-identical to what a session with nothing to serve would
            // do with it anyway.
            None => {
                tokio::spawn(async move {
                    while let Some(Ok(message)) = stream.next().await {
                        if let Message::Binary(bytes) = message {
                            if let Ok(frame) = decode_frame(&bytes) {
                                outbound.resolve(frame);
                            }
                        }
                    }
                });
            }
        }

        handle
    }
}

impl Default for TungsteniteDialer {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl PeerDialer for TungsteniteDialer {
    async fn dial(&self, peer_id: &str, endpoint: &Url) -> Result<BtpSessionHandle, DialError> {
        let failed = |reason: String| DialError {
            peer_id: peer_id.to_string(),
            endpoint: endpoint.to_string(),
            reason,
        };

        // [`is_onion_endpoint`] is called rather than re-derived: the
        // suffix that decides a dial and the suffix that decides a carriage
        // (`PeerCarriage::for_endpoint`) are one implementation, so a node
        // cannot load a peering it will not dial.
        if is_onion_endpoint(endpoint) {
            let stream = self.socks5_stream(endpoint).await.map_err(failed)?;
            // `client_async_tls_with_config` rather than `client_async`,
            // because `wss://` selects BTP at *any* host: an onion BTP
            // endpoint is not necessarily plaintext, and a path that only
            // handled `ws://` would be a silent hole. With a `ws://` URL
            // this is `client_async` exactly -- the mode comes off the URL
            // -- and with a `wss://` one the TLS handshake runs inside the
            // circuit, over the stream the proxy already established.
            let (socket, _) = tokio_tungstenite::client_async_tls_with_config(
                endpoint.as_str(),
                stream,
                None,
                None,
            )
            .await
            .map_err(|error| failed(error.to_string()))?;
            return Ok(self.start_session(socket));
        }

        let (socket, _) = tokio_tungstenite::connect_async(endpoint.as_str())
            .await
            .map_err(|error| failed(error.to_string()))?;
        Ok(self.start_session(socket))
    }
}
