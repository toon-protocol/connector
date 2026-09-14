//! Reading **another** node's self-description (ADR 0050) so a peering can
//! be established from a URL (ADR 0058).
//!
//! `connector_domain::node` builds the document this node answers with;
//! this module is the other direction -- the one outbound request this
//! connector makes to a host an operator named, from inside an
//! authenticated write.
//!
//! # Why it is bounded, and how
//!
//! ADR 0058: *"That request must be bounded -- timeout, response size,
//! redirect policy -- and must never be made on the packet path."* The
//! host is chosen by whoever holds the operator's write key, and the reply
//! is whatever that host feels like sending, so every one of those three is
//! a refusal here rather than a default someone else picks:
//!
//! * **Timeout.** [`FETCH_TIMEOUT`] covers the whole exchange, not merely
//!   the connect: a host that accepts a connection and then dribbles one
//!   byte a minute is the shape a connect timeout alone does not catch.
//! * **Response size.** [`MAX_DOCUMENT_BYTES`] is enforced while the body
//!   streams, so an unbounded response is dropped as it arrives rather
//!   than after it has been buffered. A declared `Content-Length` past the
//!   cap is refused before a byte of body is read.
//! * **Redirects.** Not followed at all. The operator named a URL and this
//!   reads that URL; a `3xx` is [`SelfDescriptionError::Redirected`], which
//!   names the location so the operator can point the write at it
//!   themselves if that is what they meant. Following one would let the
//!   named host hand the peering to a different host -- and under ADR 0059
//!   that choice determines the channel address.
//!
//! # Trust-on-first-use
//!
//! Whatever the URL serves is who the peering is with. Nothing here checks
//! the document against anything the operator supplied, and nothing should:
//! ADR 0058 considered a `settlement_address` pin and rejected it. The
//! operator's vetting of the URL is the whole of the assurance, and
//! `allow_plaintext` exists only so a local topology can rehearse over
//! `http://` the way `[[peers]]` endpoints already can.

use std::time::Duration;

use async_trait::async_trait;
use thiserror::Error;
use url::Url;

use connector_config::{is_onion_endpoint, plaintext_permitted};
use connector_domain::NodeSelfDescription;

/// Why an onion URL cannot be read on a node that configured no
/// `socks_proxy` (ADR 0070 decision 3).
///
/// [`crate::NO_SOCKS_PROXY`]'s sentence is the carriages', and says
/// "endpoint" because that is what a peering has. This surface reads a
/// **URL** an operator handed to `POST /peers`, which is not an endpoint
/// until the document behind it has been read -- so the wording differs on
/// purpose, and §9's parity requirement does not reach here: it binds the
/// two carriages to each other, and this is neither of them.
///
/// Reported as [`SelfDescriptionError::Unreachable`] rather than as a
/// scheme refusal, because that is what it is: the URL is perfectly legal
/// and this node has no way to get there.
const NO_SOCKS_PROXY: &str =
    "the URL's host ends in .onion or .anyone and this node configured no socks_proxy, so there \
     is nothing that can resolve or reach it (ADR 0070 decision 3)";

/// The whole exchange's budget -- connect, headers and body together.
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(10);

/// The most body this will read. A self-description is a few hundred bytes
/// on a node with several chains and several routes; this is three orders
/// of magnitude of headroom and still a bound.
pub const MAX_DOCUMENT_BYTES: usize = 64 * 1024;

/// Why a self-description could not be read.
///
/// Every variant names something the *remote* did, and none of them
/// describes a peering's identity as pinned, verified or attested -- there
/// is no such check to fail (ADR 0058).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SelfDescriptionError {
    /// The URL is not `https://`, is not at a hidden-service host, and this node
    /// has not opted into plaintext peer endpoints. Trust-on-first-use over
    /// TLS is the whole of the assurance ADR 0058 offers, and there is none
    /// at all without either the TLS or the onion address that stands in
    /// for it (ADR 0070 decision 2).
    #[error("a peer URL must be https, or http at a '.onion' or '.anyone' host (got '{0}'); set peer_allow_plaintext_endpoints to rehearse over http elsewhere")]
    InsecureScheme(String),

    /// The host could not be reached, or the exchange ran past
    /// [`FETCH_TIMEOUT`].
    #[error("could not read the self-description at {url}: {reason}")]
    Unreachable { url: String, reason: String },

    /// The URL answered, but not with a document: any status that is not
    /// `2xx`, redirects excepted (they get their own variant).
    #[error("{url} answered {status} rather than a self-description")]
    Status { url: String, status: u16 },

    /// The URL redirected. Not followed: see this module's own header.
    #[error("{url} redirected to '{location}'; name the URL you mean in the request rather than one that redirects")]
    Redirected { url: String, location: String },

    /// The response body ran past [`MAX_DOCUMENT_BYTES`].
    #[error(
        "the self-description at {url} is larger than the {limit}-byte bound this connector reads"
    )]
    TooLarge { url: String, limit: usize },

    /// The bytes are not a self-description.
    #[error("{url} did not answer a self-description: {reason}")]
    Malformed { url: String, reason: String },
}

/// Reads the document at a URL. A port, so the bounded HTTP client below
/// is not the only thing a peering write can be exercised against.
#[async_trait]
pub trait SelfDescriptionSource: Send + Sync {
    async fn fetch(&self, url: &Url) -> Result<NodeSelfDescription, SelfDescriptionError>;
}

/// The production [`SelfDescriptionSource`]: one bounded `GET`, no
/// redirects followed, no retry.
///
/// **No retry, deliberately.** `POST /peers` is safely retryable by the
/// operator (ADR 0059 makes the channel derivation idempotent), so a
/// failed fetch is a refusal the operator sees and repeats, not a loop
/// this connector runs on a host it was pointed at.
pub struct BoundedHttpSelfDescription {
    client: reqwest::Client,
    /// The client an **onion** URL is read on, or the reason there is none
    /// (ADR 0070 decision 3). Two clients selected by host, exactly as the
    /// two peer carriages select: this fetch is the ILP wire's own
    /// bootstrap, not settlement RPC and not a `handler_url`, so decision
    /// 4 leaves it in scope rather than out.
    onion: Result<reqwest::Client, String>,
    allow_plaintext: bool,
}

impl BoundedHttpSelfDescription {
    /// `allow_plaintext` is the node's own `peer_allow_plaintext_endpoints`
    /// -- the same opt-in a `ws://`/`http://` peer endpoint already needs,
    /// and for the same reason. `socks_proxy` is the node's own
    /// `Config::socks_proxy`, `None` on a node that configured none.
    ///
    /// Without the proxy half, ADR 0058's write could not establish a
    /// peering with an onion node at all: `POST /peers` takes the
    /// counterparty's URL and *reads* it, so a node whose only published
    /// URL is a `.onion` one would be unpeerable at runtime however well
    /// the carriages could then have dialed it.
    #[must_use]
    pub fn new(allow_plaintext: bool, socks_proxy: Option<&Url>) -> BoundedHttpSelfDescription {
        let bounded = || {
            reqwest::Client::builder()
                .timeout(FETCH_TIMEOUT)
                .redirect(reqwest::redirect::Policy::none())
        };
        let client = bounded()
            .build()
            .expect("a reqwest client with a timeout and no redirect policy always builds");
        let onion = match socks_proxy {
            None => Err(NO_SOCKS_PROXY.to_string()),
            Some(proxy) => reqwest::Proxy::all(proxy.as_str())
                .and_then(|socks| bounded().proxy(socks).build())
                .map_err(|error| format!("socks_proxy '{proxy}' could not be used: {error}")),
        };
        BoundedHttpSelfDescription {
            client,
            onion,
            allow_plaintext,
        }
    }

    /// The client `url` is read on, or why there is none.
    ///
    /// [`is_onion_endpoint`] is called rather than re-derived: the suffix
    /// that decides which client reads a self-description, the suffix that
    /// decides a peering's carriage and the suffix that decides that
    /// carriage's own proxy are one implementation, so the URL a runtime
    /// peering is established from is read over the same path the peering
    /// will then be dialed over.
    fn client_for(&self, url: &Url) -> Result<&reqwest::Client, String> {
        if is_onion_endpoint(url) {
            self.onion.as_ref().map_err(Clone::clone)
        } else {
            Ok(&self.client)
        }
    }
}

#[async_trait]
impl SelfDescriptionSource for BoundedHttpSelfDescription {
    async fn fetch(&self, url: &Url) -> Result<NodeSelfDescription, SelfDescriptionError> {
        // ADR 0070 decision 2, the same exception `[[peers]]` endpoints and
        // `[[pay_channels]]` rows take: a `.onion` host permits `http://`
        // on its own, because the address *is* the key the circuit is
        // authenticated to. Trust-on-first-use over a circuit addressed by
        // its own public key is a stronger assurance than this fetch gets
        // from web PKI, not a weaker one.
        let readable = match url.scheme() {
            "https" => true,
            "http" => plaintext_permitted(url, self.allow_plaintext),
            _ => false,
        };
        if !readable {
            return Err(SelfDescriptionError::InsecureScheme(url.to_string()));
        }
        let client = self
            .client_for(url)
            .map_err(|reason| SelfDescriptionError::Unreachable {
                url: url.to_string(),
                reason,
            })?;

        let response = client.get(url.clone()).send().await.map_err(|source| {
            SelfDescriptionError::Unreachable {
                url: url.to_string(),
                reason: source.to_string(),
            }
        })?;

        let status = response.status();
        if status.is_redirection() {
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("(no Location header)")
                .to_string();
            return Err(SelfDescriptionError::Redirected {
                url: url.to_string(),
                location,
            });
        }
        if !status.is_success() {
            return Err(SelfDescriptionError::Status {
                url: url.to_string(),
                status: status.as_u16(),
            });
        }

        // A declared length past the cap is refused before any body is
        // read; an undeclared or lying one is caught by the streaming
        // bound below.
        if response
            .content_length()
            .is_some_and(|declared| declared > MAX_DOCUMENT_BYTES as u64)
        {
            return Err(SelfDescriptionError::TooLarge {
                url: url.to_string(),
                limit: MAX_DOCUMENT_BYTES,
            });
        }

        let mut response = response;
        let mut body: Vec<u8> = Vec::new();
        loop {
            let chunk =
                response
                    .chunk()
                    .await
                    .map_err(|source| SelfDescriptionError::Unreachable {
                        url: url.to_string(),
                        reason: source.to_string(),
                    })?;
            let Some(chunk) = chunk else { break };
            if body.len() + chunk.len() > MAX_DOCUMENT_BYTES {
                return Err(SelfDescriptionError::TooLarge {
                    url: url.to_string(),
                    limit: MAX_DOCUMENT_BYTES,
                });
            }
            body.extend_from_slice(&chunk);
        }

        serde_json::from_slice(&body).map_err(|source| SelfDescriptionError::Malformed {
            url: url.to_string(),
            reason: source.to_string(),
        })
    }
}

/// A [`SelfDescriptionSource`] that reaches no network at all: every URL is
/// [`SelfDescriptionError::Unreachable`].
///
/// The default a [`crate::Connector`] holds until one is configured, and
/// what a node with no outbound reachability behaves as. It is a whole
/// implementation of the port rather than a stub that records calls: "this
/// node cannot reach that host" is a real answer, and a peering write is
/// refused by name on it instead of hanging.
pub struct UnreachableSelfDescription;

#[async_trait]
impl SelfDescriptionSource for UnreachableSelfDescription {
    async fn fetch(&self, url: &Url) -> Result<NodeSelfDescription, SelfDescriptionError> {
        Err(SelfDescriptionError::Unreachable {
            url: url.to_string(),
            reason: "this connector has no self-description source configured".to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::convert::Infallible;
    use std::net::SocketAddr;

    use axum::body::Body;
    use axum::http::{header, StatusCode};
    use axum::response::{IntoResponse, Response};
    use axum::routing::get;
    use axum::Router;
    use connector_domain::{EdgeIdentity, NodeFacts};

    /// A real HTTP server on a real socket, answering whatever `router`
    /// builds. Not a mock: the bounds under test are properties of an
    /// actual exchange -- a status, a `Location` header, a body that never
    /// ends -- and none of them exists in a fake that hands back a value.
    fn serve(router: Router) -> SocketAddr {
        let server = axum::Server::bind(&"127.0.0.1:0".parse().expect("loopback"))
            .serve(router.into_make_service());
        let addr = server.local_addr();
        tokio::spawn(async move {
            let _ = server.await;
        });
        addr
    }

    fn document() -> NodeSelfDescription {
        NodeSelfDescription::describe(
            &NodeFacts {
                ilp_addresses: vec!["g.example.peer".to_string()],
                http_endpoint: Some("https://peer.example/ilp".to_string()),
                btp_endpoint: None,
                peer_carriages: vec!["http".to_string()],
                settlements: Vec::new(),
            },
            Some(EdgeIdentity {
                key_id: "key-1".to_string(),
                public_key: "0x04ab".to_string(),
            }),
            Vec::new(),
            None,
        )
    }

    fn url(addr: SocketAddr, path: &str) -> Url {
        Url::parse(&format!("http://{addr}{path}")).expect("url")
    }

    #[tokio::test]
    async fn a_served_document_reads_back_as_the_document_that_was_served() {
        let served = document();
        let answered = served.clone();
        let router = Router::new().route(
            "/ilp",
            get(move || {
                let answered = answered.clone();
                async move { axum::Json(answered) }
            }),
        );
        let addr = serve(router);

        let fetched = BoundedHttpSelfDescription::new(true, None)
            .fetch(&url(addr, "/ilp"))
            .await
            .expect("the document reads back");

        assert_eq!(fetched, served);
    }

    /// The URL an operator names is the URL that is read. Following a
    /// redirect would let the named host choose a different counterparty,
    /// and under ADR 0059 the counterparty determines the channel address
    /// -- a party this node would then fund.
    #[tokio::test]
    async fn a_redirect_is_refused_by_name_rather_than_followed() {
        let router = Router::new().route(
            "/ilp",
            get(|| async {
                Response::builder()
                    .status(StatusCode::FOUND)
                    .header(header::LOCATION, "http://elsewhere.invalid/ilp")
                    .body(Body::empty())
                    .expect("redirect response")
                    .into_response()
            }),
        );
        let addr = serve(router);

        let error = BoundedHttpSelfDescription::new(true, None)
            .fetch(&url(addr, "/ilp"))
            .await
            .expect_err("a redirect is not followed");

        match error {
            SelfDescriptionError::Redirected { location, .. } => {
                assert_eq!(location, "http://elsewhere.invalid/ilp");
            }
            other => panic!("expected a named redirect refusal, got {other:?}"),
        }
    }

    /// A body past the bound is refused **while it streams**, so nothing
    /// unbounded is ever buffered. The server here never stops sending and
    /// declares no length, which is the shape a `Content-Length` check
    /// alone does not catch.
    #[tokio::test]
    async fn an_endless_body_is_refused_at_the_bound_rather_than_buffered() {
        let router = Router::new().route(
            "/ilp",
            get(|| async {
                let stream =
                    futures_util::stream::repeat_with(|| Ok::<_, Infallible>(vec![b'x'; 8 * 1024]));
                Response::builder()
                    .status(StatusCode::OK)
                    .body(Body::wrap_stream(stream))
                    .expect("streaming response")
                    .into_response()
            }),
        );
        let addr = serve(router);

        let error = BoundedHttpSelfDescription::new(true, None)
            .fetch(&url(addr, "/ilp"))
            .await
            .expect_err("an endless body is refused");

        assert!(
            matches!(
                error,
                SelfDescriptionError::TooLarge {
                    limit: MAX_DOCUMENT_BYTES,
                    ..
                }
            ),
            "expected the size bound to fire, got {error:?}"
        );
    }

    /// A declared length past the bound costs no body read at all.
    #[tokio::test]
    async fn a_declared_length_past_the_bound_is_refused_before_the_body() {
        let router = Router::new().route(
            "/ilp",
            get(|| async {
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_LENGTH, (MAX_DOCUMENT_BYTES + 1).to_string())
                    .body(Body::from(vec![b'x'; MAX_DOCUMENT_BYTES + 1]))
                    .expect("oversized response")
                    .into_response()
            }),
        );
        let addr = serve(router);

        let error = BoundedHttpSelfDescription::new(true, None)
            .fetch(&url(addr, "/ilp"))
            .await
            .expect_err("a declared oversize is refused");

        assert!(matches!(error, SelfDescriptionError::TooLarge { .. }));
    }

    #[tokio::test]
    async fn a_non_document_answer_is_refused_by_its_status() {
        let router = Router::new().route(
            "/ilp",
            get(|| async { (StatusCode::NOT_FOUND, "no such node") }),
        );
        let addr = serve(router);

        let error = BoundedHttpSelfDescription::new(true, None)
            .fetch(&url(addr, "/ilp"))
            .await
            .expect_err("a 404 is not a document");

        assert!(matches!(
            error,
            SelfDescriptionError::Status { status: 404, .. }
        ));
    }

    #[tokio::test]
    async fn a_body_that_is_not_a_self_description_is_refused_as_malformed() {
        let router = Router::new().route("/ilp", get(|| async { "not json at all" }));
        let addr = serve(router);

        let error = BoundedHttpSelfDescription::new(true, None)
            .fetch(&url(addr, "/ilp"))
            .await
            .expect_err("prose is not a document");

        assert!(matches!(error, SelfDescriptionError::Malformed { .. }));
    }

    /// Trust-on-first-use is *over TLS*; a node that has not opted into
    /// plaintext peer endpoints refuses `http://` before any request is
    /// made.
    #[tokio::test]
    async fn plaintext_is_refused_unless_the_node_opted_in() {
        let url = Url::parse("http://peer.example/ilp").expect("url");

        let error = BoundedHttpSelfDescription::new(false, None)
            .fetch(&url)
            .await
            .expect_err("http is refused by default");

        assert!(matches!(error, SelfDescriptionError::InsecureScheme(_)));
    }

    /// ADR 0070 decision 2: a `.onion` host permits `http://` on its own,
    /// on a node that opted into nothing. The exemption is what makes a
    /// runtime peering with an onion node possible at all -- `POST /peers`
    /// reads the counterparty's URL, and an onion node's only published URL
    /// is an onion one.
    ///
    /// Refused as **unreachable** rather than as an insecure scheme when no
    /// proxy is configured, which is the distinction that matters to the
    /// operator reading it: the URL is legal and this node has no way to
    /// get there.
    ///
    /// Both spellings (issue #1284): a runtime peering is established from
    /// whatever URL the counterparty's own daemon made it publish, and this
    /// node has no say in which `anon` release that was.
    #[tokio::test]
    async fn an_onion_url_is_readable_without_the_plaintext_opt_in() {
        for host in [
            "vww6ybal4bd7szmgncyruucpgfkqahzddi37ktceo3ah7ngmcopnpyyd.onion",
            "vww6ybal4bd7szmgncyruucpgfkqahzddi37ktceo3ah7ngmcopnpyyd.anyone",
        ] {
            let url = Url::parse(&format!("http://{host}/ilp")).expect("url");

            let error = BoundedHttpSelfDescription::new(false, None)
                .fetch(&url)
                .await
                .expect_err("no proxy is configured, so it cannot be reached");

            match error {
                SelfDescriptionError::Unreachable { reason, .. } => assert!(
                    reason.contains("socks_proxy"),
                    "{host}: the refusal has to name what is missing, got: {reason}"
                ),
                other => {
                    panic!("{host} is not an insecure scheme (ADR 0070): {other}")
                }
            }
        }
    }

    /// And the suffix is a suffix. A host that merely contains `.onion` or
    /// `.anyone` without ending in it is an ordinary clearnet host, and
    /// `http://` at one is refused exactly as it was before ADR 0070 --
    /// including at the second spelling the rule gained in issue #1284,
    /// which is the case a widening is most likely to have widened too far.
    #[tokio::test]
    async fn a_host_that_only_looks_onion_is_still_refused_as_plaintext() {
        for host in [
            "onion.example",
            "notreally.onion.example",
            "anyone.example",
            "notreally.anyone.example",
        ] {
            let url = Url::parse(&format!("http://{host}/ilp")).expect("url");

            let error = BoundedHttpSelfDescription::new(false, None)
                .fetch(&url)
                .await
                .expect_err(host);

            assert!(
                matches!(error, SelfDescriptionError::InsecureScheme(_)),
                "{host} must still be plaintext, got: {error}"
            );
        }
    }

    /// A URL that is neither http nor https is refused whatever the
    /// opt-in says -- `file://` in particular, which would otherwise read
    /// the node's own disk from an operator write.
    #[tokio::test]
    async fn a_non_http_scheme_is_refused_even_with_plaintext_allowed() {
        let url = Url::parse("file:///etc/passwd").expect("url");

        let error = BoundedHttpSelfDescription::new(true, None)
            .fetch(&url)
            .await
            .expect_err("file:// is not a peer URL");

        assert!(matches!(error, SelfDescriptionError::InsecureScheme(_)));
    }

    #[tokio::test]
    async fn the_unreachable_source_refuses_by_name_rather_than_hanging() {
        let error = UnreachableSelfDescription
            .fetch(&Url::parse("https://peer.example/ilp").expect("url"))
            .await
            .expect_err("no source configured");

        assert!(matches!(error, SelfDescriptionError::Unreachable { .. }));
    }
}
