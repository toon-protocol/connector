//! The one place this crate touches a socket: a [`PeerHttpClient`] over
//! `reqwest`.
//!
//! Everything the carriage *decides* -- which headers ride, what an absent
//! ack means, §7.2's in-flight rule --
//! is above this file and provable without it. What is here is TLS, a
//! connection pool, a byte copy, and -- since ADR 0070 -- **which of two
//! sockets the bytes leave on**.

use async_trait::async_trait;
use connector_config::is_onion_endpoint;
use connector_proxied_client::ProxiedClient;
use connector_runtime::NO_SOCKS_PROXY;
use url::Url;

use crate::dial::{HttpDialError, PeerHttpClient};
use crate::headers::{Headers, PeerRequest, PeerResponse};

/// The content type an OER ILP packet rides under, the same one the client
/// edge's `POST /ilp` uses (`client-edge-spec.md` §1.1).
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
    /// Onion endpoints. `None` on a node that configured no proxy, which is
    /// the default and is not an error condition until an onion endpoint is
    /// actually dialed. A proxy whose host does not resolve fails the dial
    /// the same way -- as an ordinary dial failure carrying the reason --
    /// and is built again at the next one.
    onion: Option<ProxiedClient>,
}

impl ReqwestPeerClient {
    /// A client over `client`, so a caller that already tunes timeouts,
    /// pools or roots keeps them. The carriage's own deadlines
    /// (`peerAnswerTimeoutMs` -- §6.3) are applied
    /// above this, per peering relation, and are not this client's.
    ///
    /// No proxy: a node that configured none dials every endpoint direct,
    /// and an onion one fails at the dial saying so.
    #[must_use]
    pub fn new(client: reqwest::Client) -> Self {
        ReqwestPeerClient {
            direct: client,
            onion: None,
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
    /// Infallible on purpose. The proxied client fails to build when the
    /// proxy's host does not resolve (its daemon is not up yet), and the
    /// honest answer to that is the same one a missing proxy gets: onion
    /// dials fail, naming the reason, while every clearnet peering on this
    /// node keeps working. [`ProxiedClient`] builds it again at the next
    /// onion dial, and again when a request finds the proxy unreachable.
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
        ReqwestPeerClient {
            direct: reqwest::Client::new(),
            onion: Some(ProxiedClient::new(proxy, |builder| builder)),
        }
    }

    /// The client `endpoint` leaves on, or why there is none.
    ///
    /// [`is_onion_endpoint`] is called rather than re-derived: the suffix
    /// that decides a dial and the suffix that decides a carriage
    /// (`PeerCarriage::for_endpoint`) are one implementation, so a node
    /// cannot load a peering it will not dial.
    fn client_for(&self, endpoint: &Url) -> Result<reqwest::Client, String> {
        if is_onion_endpoint(endpoint) {
            match &self.onion {
                Some(proxied) => proxied.client(),
                None => Err(NO_SOCKS_PROXY.to_string()),
            }
        } else {
            Ok(self.direct.clone())
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

        let response = outbound.send().await.map_err(|error| {
            if let (Some(proxied), true) = (&self.onion, is_onion_endpoint(endpoint)) {
                proxied.observe(&error);
            }
            failure(error.to_string())
        })?;
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
