//! The one place this crate touches a socket: a [`PeerHttpClient`] over
//! `reqwest`.
//!
//! Everything the carriage *decides* -- which headers ride, what an absent
//! ack means, when a claim may be retransmitted, §7.2's in-flight rule --
//! is above this file and provable without it. What is here is TLS, a
//! connection pool, a byte copy, and -- since ADR 0070 -- **which of two
//! sockets the bytes leave on**.

use async_trait::async_trait;
use connector_config::is_onion_endpoint;
use connector_runtime::NO_SOCKS_PROXY;
use url::Url;

use crate::dial::{HttpDialError, PeerHttpClient};
use crate::headers::{Headers, PeerRequest, PeerResponse};

/// The content type an OER ILP packet rides under, the same one the client
/// edge's `POST /ilp` uses (`client-edge-spec.md` §1.1). A FLUSH's body is
/// empty and carries it too: the shape is "a POST with an empty ILP body",
/// not a differently typed request.
const OCTET_STREAM: &str = "application/octet-stream";

/// A [`PeerHttpClient`] backed by `reqwest`, rustls only.
///
/// **Two clients, selected per endpoint by host** (ADR 0070 decision 3):
/// an endpoint whose host ends in `.onion` leaves through the configured
/// SOCKS5 proxy, every other endpoint is dialed direct, and nothing else
/// participates in the choice. There is no per-peer key saying which, and
/// no mode that reroutes everything -- the address already carries the
/// answer, so selection lives here, below the peering relation, where a
/// peering registered while the process serves (ADR 0058) gets it for free
/// rather than through a second decision that could disagree with this one.
///
/// The proxy covers **this** wire and no other (ADR 0070 decision 4):
/// settlement RPC and the app's `handler_url` hold their own clients and
/// dial direct.
pub struct ReqwestPeerClient {
    /// Every endpoint that is not an onion endpoint.
    direct: reqwest::Client,
    /// Onion endpoints -- or the reason this node has no way to dial one.
    ///
    /// `Err` on a node that configured no proxy, which is the default and
    /// is not an error condition until an onion endpoint is actually
    /// dialed; and `Err` too if `reqwest` refused the configured URL,
    /// which [`Config::load`] has already checked the scheme of. Both fail
    /// the same way -- as an ordinary dial failure carrying the reason --
    /// because both mean the same thing to the packet: this endpoint
    /// cannot be reached from this node.
    ///
    /// [`Config::load`]: connector_config::Config::load
    onion: Result<reqwest::Client, String>,
}

impl ReqwestPeerClient {
    /// A client over `client`, so a caller that already tunes timeouts,
    /// pools or roots keeps them. The carriage's own deadlines
    /// (`peerAnswerTimeoutMs`, `claimAckTimeoutMs` -- §6.3) are applied
    /// above this, per peering relation, and are not this client's.
    ///
    /// No proxy: a node that configured none dials every endpoint direct,
    /// and an onion one fails at the dial saying so.
    #[must_use]
    pub fn new(client: reqwest::Client) -> Self {
        ReqwestPeerClient {
            direct: client,
            onion: Err(NO_SOCKS_PROXY.to_string()),
        }
    }

    /// A client that dials onion endpoints through `proxy`, and everything
    /// else direct (ADR 0070 decision 3).
    ///
    /// `proxy` is `Config::socks_proxy`, which [`Config::load`] has
    /// already refused unless it is `socks5h://` -- the `h` is not a
    /// preference: it is what makes the *proxy* resolve the name, and no
    /// resolver on this machine can resolve a `.onion` one.
    ///
    /// Infallible on purpose. The only way the proxied client fails to
    /// build is a `reqwest` refusal of a URL config has already validated
    /// the scheme of, and the honest answer to that is the same one a
    /// missing proxy gets: onion dials fail, naming the reason, while
    /// every clearnet peering on this node keeps working.
    ///
    /// Takes no direct client, unlike [`ReqwestPeerClient::new`], so it does
    /// not compose with a caller that tuned one. Nothing needs that today --
    /// `build_peer_transport` picks one constructor or the other and tunes
    /// neither -- and a parameter added for a caller that does not exist is
    /// a shape somebody has to maintain in the meantime. A caller who wants
    /// both should add it then, when there is a real client to thread.
    ///
    /// [`Config::load`]: connector_config::Config::load
    #[must_use]
    pub fn through_socks_proxy(proxy: &Url) -> Self {
        let onion = reqwest::Proxy::all(proxy.as_str())
            .and_then(|socks| reqwest::Client::builder().proxy(socks).build())
            .map_err(|error| format!("socks_proxy '{proxy}' could not be used: {error}"));
        ReqwestPeerClient {
            direct: reqwest::Client::new(),
            onion,
        }
    }

    /// The client `endpoint` leaves on, or why there is none.
    ///
    /// [`is_onion_endpoint`] is called rather than re-derived: the suffix
    /// that decides a dial and the suffix that decides a carriage
    /// (`PeerCarriage::for_endpoint`) are one implementation, so a node
    /// cannot load a peering it will not dial.
    fn client_for(&self, endpoint: &Url) -> Result<&reqwest::Client, String> {
        if is_onion_endpoint(endpoint) {
            self.onion.as_ref().map_err(Clone::clone)
        } else {
            Ok(&self.direct)
        }
    }
}

impl Default for ReqwestPeerClient {
    fn default() -> Self {
        ReqwestPeerClient::new(reqwest::Client::new())
    }
}

#[async_trait]
impl PeerHttpClient for ReqwestPeerClient {
    async fn post(
        &self,
        endpoint: &Url,
        request: PeerRequest,
    ) -> Result<PeerResponse, HttpDialError> {
        let failure = |reason: String| HttpDialError {
            peer_id: String::new(),
            endpoint: endpoint.to_string(),
            reason,
        };

        let mut outbound = self
            .client_for(endpoint)
            .map_err(failure)?
            .post(endpoint.clone())
            .header("content-type", OCTET_STREAM)
            .body(request.body);
        for (name, value) in request.headers.iter() {
            outbound = outbound.header(name, value);
        }

        let response = outbound
            .send()
            .await
            .map_err(|error| failure(error.to_string()))?;
        let status = response.status().as_u16();
        let mut headers = Headers::new();
        for (name, value) in response.headers() {
            // A header whose bytes are not text is not one §3 names, and §3
            // requires anything this document does not name be ignored on
            // receipt rather than refused -- so the carriage stays additively
            // extensible.
            if let Ok(value) = value.to_str() {
                headers.push(name.as_str(), value);
            }
        }
        let body = response
            .bytes()
            .await
            .map_err(|error| failure(error.to_string()))?
            .to_vec();

        Ok(PeerResponse {
            status,
            headers,
            body,
        })
    }
}
