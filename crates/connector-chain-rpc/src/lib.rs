//! The HTTP transport every settlement table's RPC clients dial through
//! (ADR 0073).
//!
//! Before this crate there were four clients of a settlement `rpc_url` and
//! four ways of building one. The EVM backend, the channel-index syncer and
//! the rate source each called `Provider::<Http>::try_from`, which is
//! `reqwest::Client::new()`: no connect timeout, no request timeout, and no
//! retry. The Solana backend used the SDK's `HttpSender`, which has a 30s
//! timeout and retries a 429 but not a 403. A circuit that stopped answering
//! therefore hung the EVM side **forever**, and nothing could route any of
//! them through a proxy without editing four constructors in three crates and
//! hoping none was missed. One of them left direct is the whole leak ADR 0073
//! exists to close.
//!
//! So a settlement table's endpoint is now a value, [`RpcTransport`], built
//! once per table and cloned into every client of that `rpc_url`. It carries
//! three things, and nothing else:
//!
//! - **the bounds** ([`Timeouts::SETTLEMENT`]): 20s to connect, 30s per
//!   request, and an idle pooled connection dropped after 30s, so one whose
//!   circuit died quietly is not reused for long (ADR 0073 decision 4). They
//!   apply to every settlement client, proxied or not, because an unbounded
//!   request is a defect on a direct connection too;
//! - **the route**: direct, or through the node's one `socks_proxy` on a
//!   [`Circuit`] pinned per chain by a fixed SOCKS username (decision 3). It
//!   fails closed: a proxy that is down is an error, and nothing here ever
//!   falls back to a direct dial, because a proxied `reqwest::Client` has no
//!   direct path to fall back to;
//! - **the refusal rule** ([`refusal_backoff`]): an HTTP 403 or 429 is an
//!   exit relay's shared IP being rate-limited or blocked, not an answer, so
//!   both chains' adapters retry it with backoff inside a fixed budget
//!   (decision 5).
//!
//! The chain adapters are behind features so a crate takes only its own
//! chain's SDK: [`evm::EvmRpc`] is the ethers `JsonRpcClient`, and
//! [`solana::rpc_client`] builds the SDK's `RpcClient` over a sender that
//! applies the same refusal rule. The crate holds no key, signs nothing and
//! knows nothing about channels; it is the part of a dial that is the same on
//! both chains.

use std::time::Duration;

use reqwest::header::{HeaderMap, RETRY_AFTER};
use url::Url;

#[cfg(feature = "evm")]
pub mod evm;
#[cfg(feature = "test-support")]
mod fake;
#[cfg(feature = "solana")]
pub mod solana;
#[cfg(feature = "test-support")]
pub use fake::{FakeRpc, RpcCall, RpcReply};

/// How long a settlement RPC client may wait, per stage (ADR 0073 decision
/// 4). Sized from the evidence with a margin of at least twice the worst
/// value seen: the slowest call that needed a brand-new circuit took 13s.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timeouts {
    /// The SOCKS handshake, the circuit, TCP and TLS, together.
    pub connect: Duration,
    /// One whole request, connect included.
    pub request: Duration,
    /// How long a pooled connection may sit idle before it is dropped
    /// rather than reused. A circuit can die under an idle connection
    /// without the socket noticing, and the next request would then spend
    /// its whole `request` budget finding out.
    pub pool_idle: Duration,
}

impl Timeouts {
    /// The bounds every settlement RPC client runs under.
    pub const SETTLEMENT: Timeouts = Timeouts {
        connect: Duration::from_secs(20),
        request: Duration::from_secs(30),
        pool_idle: Duration::from_secs(30),
    };
}

/// Which pinned circuit a table's clients ride (ADR 0073 decision 3).
///
/// A circuit is pinned by the SOCKS username a client authenticates with:
/// the `anon` daemon's `IsolateSOCKSAuth` (on by default) keeps streams
/// with different credentials on different circuits and streams with the
/// same credentials on one. So each chain gets a fixed username of its own.
/// That keeps one exit from seeing both chains' RPCs, and it keeps
/// settlement off the ILP wire's circuits, whose dials present no
/// credentials at all. The daemon still rotates a circuit after
/// `MaxCircuitDirtiness`, and that is left alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Circuit {
    /// `[settlement.evm]`: the backend, the channel-index syncer and the
    /// rate source.
    EvmSettlement,
    /// `[settlement.solana]`: the backend.
    SolanaSettlement,
}

impl Circuit {
    /// The SOCKS username this circuit authenticates with. Fixed rather
    /// than per-process, so the pinning survives a restart the same way it
    /// survives a reconnect.
    pub fn socks_username(self) -> &'static str {
        match self {
            Circuit::EvmSettlement => "toon-settlement-evm",
            Circuit::SolanaSettlement => "toon-settlement-solana",
        }
    }

    /// The SOCKS password. RFC 1929 requires one; `IsolateSOCKSAuth`
    /// isolates on the pair, and the username alone already says which
    /// circuit this is, so the password is a constant.
    pub fn socks_password(self) -> &'static str {
        "pinned"
    }
}

/// Where a settlement table's requests go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// Straight to `rpc_url`, from this host's own address.
    Direct,
    /// Through `proxy` (a `socks5h://` URL: the node's one `socks_proxy`,
    /// already validated by `Config::load`), on `circuit`.
    Circuit { proxy: Url, circuit: Circuit },
}

/// A settlement table's endpoint: its URL, and the one HTTP client every
/// RPC client of that URL shares (ADR 0073 decision 2).
///
/// Cloning is cheap and shares the connection pool, which is the point:
/// the backend, the syncer and the rate source reuse one another's
/// connections, and so one another's circuit.
#[derive(Debug, Clone)]
pub struct RpcTransport {
    url: Url,
    http: reqwest::Client,
    route: Route,
}

/// Why a transport could not be built. Each is a configuration fact, and
/// `Config::load` refuses all of them first, so reaching one here means the
/// value did not come from a loaded config.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("settlement rpc_url '{value}' is not a URL: {source}")]
    InvalidUrl {
        value: String,
        #[source]
        source: url::ParseError,
    },
    #[error("settlement rpc_url '{value}' is not http(s)")]
    UnsupportedScheme { value: String },
    #[error(
        "socks_proxy '{value}' is not a socks5h:// URL, and a settlement circuit takes no other"
    )]
    ProxyNotSocks5h { value: String },
    #[error("could not build the settlement HTTP client: {0}")]
    Client(#[source] reqwest::Error),
}

impl RpcTransport {
    /// A direct transport under [`Timeouts::SETTLEMENT`].
    pub fn direct(rpc_url: &str) -> Result<RpcTransport, TransportError> {
        RpcTransport::new(rpc_url, Route::Direct, Timeouts::SETTLEMENT)
    }

    /// A transport through `proxy` on `circuit`, under
    /// [`Timeouts::SETTLEMENT`].
    pub fn through(
        rpc_url: &str,
        proxy: &Url,
        circuit: Circuit,
    ) -> Result<RpcTransport, TransportError> {
        RpcTransport::new(
            rpc_url,
            Route::Circuit {
                proxy: proxy.clone(),
                circuit,
            },
            Timeouts::SETTLEMENT,
        )
    }

    /// The general form. Production takes [`Timeouts::SETTLEMENT`] through
    /// [`direct`](Self::direct) or [`through`](Self::through); a test that
    /// needs to watch a timeout fire without waiting 30s for it passes its
    /// own.
    pub fn new(
        rpc_url: &str,
        route: Route,
        timeouts: Timeouts,
    ) -> Result<RpcTransport, TransportError> {
        let url = Url::parse(rpc_url).map_err(|source| TransportError::InvalidUrl {
            value: rpc_url.to_string(),
            source,
        })?;
        if url.scheme() != "http" && url.scheme() != "https" {
            return Err(TransportError::UnsupportedScheme {
                value: rpc_url.to_string(),
            });
        }
        let mut builder = reqwest::Client::builder()
            .connect_timeout(timeouts.connect)
            .timeout(timeouts.request)
            .pool_idle_timeout(timeouts.pool_idle);
        match &route {
            Route::Direct => {
                // Never inherit HTTP(S)_PROXY from the environment: which
                // way a settlement dial goes is the config file's to say
                // (ADR 0009), and a direct route that quietly went through
                // someone's proxy would be neither.
                builder = builder.no_proxy();
            }
            Route::Circuit { proxy, circuit } => {
                if proxy.scheme() != "socks5h" {
                    return Err(TransportError::ProxyNotSocks5h {
                        value: proxy.to_string(),
                    });
                }
                // `Proxy::all` means every request this client makes goes
                // to the proxy: there is no host this client dials direct,
                // so a proxy that is down fails the request rather than
                // routing around it (ADR 0073 decision 2).
                let proxy = reqwest::Proxy::all(proxy.as_str())
                    .map_err(TransportError::Client)?
                    .basic_auth(circuit.socks_username(), circuit.socks_password());
                builder = builder.proxy(proxy);
            }
        }
        let http = builder.build().map_err(TransportError::Client)?;
        Ok(RpcTransport { url, http, route })
    }

    /// The endpoint every request goes to.
    pub fn url(&self) -> &Url {
        &self.url
    }

    /// The shared HTTP client.
    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    /// Which way requests go.
    pub fn route(&self) -> &Route {
        &self.route
    }

    /// Whether requests ride a circuit. A backend polls more slowly when
    /// they do, since every poll then costs a circuit round trip.
    pub fn is_proxied(&self) -> bool {
        matches!(self.route, Route::Circuit { .. })
    }

    /// `scheme://host[:port]`, for error messages and logs.
    ///
    /// Never the full URL: a keyed RPC endpoint carries its API key in the
    /// path (`/v2/<key>`), and a log line is not where that belongs.
    pub fn endpoint(&self) -> String {
        match (self.url.host_str(), self.url.port()) {
            (Some(host), Some(port)) => format!("{}://{host}:{port}", self.url.scheme()),
            (Some(host), None) => format!("{}://{host}", self.url.scheme()),
            _ => self.url.scheme().to_string(),
        }
    }
}

/// How many times a refused request is retried before the refusal is
/// reported (ADR 0073 decision 5).
pub const REFUSAL_RETRIES: u32 = 4;

/// The first wait after a refusal. Each later one doubles it.
const FIRST_REFUSAL_BACKOFF: Duration = Duration::from_millis(500);

/// The most a `Retry-After` header is honoured for. A server asking for
/// longer is asking for more than a settlement call can spend, and the
/// refusal is reported instead of slept through.
const RETRY_AFTER_CAP: Duration = Duration::from_secs(8);

/// Whether an HTTP status is a **refusal**: the endpoint declined to serve
/// this client just now, which says nothing about the request.
///
/// 429 is rate limiting. 403 is what public RPCs answer when they block an
/// address, and an exit relay's address is shared with everyone else on it.
/// Neither is a JSON-RPC answer, so neither may end a confirmation poll or be
/// read as "the transaction failed". The 3,200 calls ADR 0073 measured saw
/// none, which is why this retries rather than assumes.
pub fn is_refusal(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::FORBIDDEN || status == reqwest::StatusCode::TOO_MANY_REQUESTS
}

/// How long to wait before retry number `retry` (0-based) of a refused
/// request, or `None` when the retry budget is spent.
///
/// 0.5s, 1s, 2s, 4s: 7.5s in all, well inside one request's 30s. A
/// `Retry-After` in seconds replaces the computed wait when it is shorter
/// than [`RETRY_AFTER_CAP`]; one longer than that ends the retries.
pub fn refusal_backoff(retry: u32, headers: &HeaderMap) -> Option<Duration> {
    if retry >= REFUSAL_RETRIES {
        return None;
    }
    let computed = FIRST_REFUSAL_BACKOFF * 2u32.pow(retry);
    match retry_after(headers) {
        Some(asked) if asked > RETRY_AFTER_CAP => None,
        Some(asked) => Some(asked),
        None => Some(computed),
    }
}

/// A `Retry-After` given in seconds. The HTTP-date form is not honoured:
/// no public RPC was seen to send it, and a clock comparison is one more way
/// to sleep for the wrong length of time.
fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    headers
        .get(RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .map(Duration::from_secs)
}

/// How many times a read that has nothing riding on it yet is retried
/// before its failure is reported: three, as ADR 0073 decision 5 sets for
/// boot.
pub const READ_RETRIES: u32 = 3;

/// The first wait between attempts. Each later one doubles it, so a read
/// that never succeeds costs 0.5s + 1s + 2s before it is reported.
const FIRST_READ_BACKOFF: Duration = Duration::from_millis(500);

/// Run a read, retrying any failure [`READ_RETRIES`] times with backoff
/// before returning the last one.
///
/// For the reads a node makes before anything depends on them: every one
/// boot makes (boot is where a circuit is youngest, and one lost round trip
/// used to fail the whole node), and the blockhash or nonce a write reads
/// before it signs. Every failure is retried, the node's own answers
/// included, because retrying a read that was definitively refused only
/// costs 3.5s before the same refusal is reported, and telling the two
/// apart would need a per-call judgement that is easy to get wrong.
///
/// **Never a send.** A transaction is not passed through this: a send whose
/// answer was lost is resolved by looking up what it did, never by sending
/// it again blind.
pub async fn retry_read<T, E, F, Fut>(mut call: F) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
{
    let mut attempt = 0;
    loop {
        match call().await {
            Ok(value) => return Ok(value),
            Err(error) if attempt >= READ_RETRIES => return Err(error),
            Err(_) => {
                tokio::time::sleep(FIRST_READ_BACKOFF * 2u32.pow(attempt)).await;
                attempt += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;

    #[test]
    fn a_refusal_is_a_403_or_a_429_and_nothing_else() {
        assert!(is_refusal(reqwest::StatusCode::FORBIDDEN));
        assert!(is_refusal(reqwest::StatusCode::TOO_MANY_REQUESTS));
        for other in [200, 400, 401, 404, 500, 502, 503] {
            assert!(!is_refusal(reqwest::StatusCode::from_u16(other).unwrap()));
        }
    }

    #[test]
    fn refusals_back_off_doubling_and_stop_after_the_budget() {
        let none = HeaderMap::new();
        let waits: Vec<_> = (0..=REFUSAL_RETRIES)
            .map(|retry| refusal_backoff(retry, &none))
            .collect();
        assert_eq!(
            waits,
            vec![
                Some(Duration::from_millis(500)),
                Some(Duration::from_secs(1)),
                Some(Duration::from_secs(2)),
                Some(Duration::from_secs(4)),
                None,
            ]
        );
    }

    #[test]
    fn a_short_retry_after_is_honoured_and_a_long_one_ends_the_retries() {
        let mut headers = HeaderMap::new();
        headers.insert(RETRY_AFTER, HeaderValue::from_static("3"));
        assert_eq!(refusal_backoff(0, &headers), Some(Duration::from_secs(3)));
        headers.insert(RETRY_AFTER, HeaderValue::from_static("120"));
        assert_eq!(refusal_backoff(0, &headers), None);
    }

    #[test]
    fn the_endpoint_never_carries_the_path_where_an_api_key_lives() {
        let transport =
            RpcTransport::direct("https://base-sepolia.g.alchemy.com/v2/SECRETKEY").unwrap();
        assert_eq!(transport.endpoint(), "https://base-sepolia.g.alchemy.com");
        let transport = RpcTransport::direct("http://127.0.0.1:8545/").unwrap();
        assert_eq!(transport.endpoint(), "http://127.0.0.1:8545");
    }

    #[test]
    fn a_circuit_takes_a_socks5h_proxy_and_no_other() {
        let proxy = Url::parse("socks5://127.0.0.1:9050").unwrap();
        let refused = RpcTransport::through("https://rpc.example", &proxy, Circuit::EvmSettlement);
        assert!(matches!(
            refused,
            Err(TransportError::ProxyNotSocks5h { .. })
        ));
        let proxy = Url::parse("socks5h://127.0.0.1:9050").unwrap();
        let built =
            RpcTransport::through("https://rpc.example", &proxy, Circuit::EvmSettlement).unwrap();
        assert!(built.is_proxied());
    }

    #[tokio::test(start_paused = true)]
    async fn a_read_is_retried_three_times_and_then_its_failure_is_reported() {
        let mut attempts = 0;
        let failed: Result<(), &str> = retry_read(|| {
            attempts += 1;
            async { Err("unreachable") }
        })
        .await;
        assert_eq!(failed, Err("unreachable"));
        assert_eq!(attempts, 1 + READ_RETRIES);

        let mut attempts = 0;
        let served: Result<u8, &str> = retry_read(|| {
            attempts += 1;
            let result = if attempts < 3 { Err("not yet") } else { Ok(7) };
            async move { result }
        })
        .await;
        assert_eq!(served, Ok(7));
    }

    #[test]
    fn each_chain_pins_its_own_circuit() {
        assert_ne!(
            Circuit::EvmSettlement.socks_username(),
            Circuit::SolanaSettlement.socks_username()
        );
    }
}
