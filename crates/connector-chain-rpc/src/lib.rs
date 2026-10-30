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
//!   (decision 5). That budget is spent **once**, in the transport: a
//!   refusal that outlasts it is reported as [`Refused`], and
//!   [`retry_read`] does not retry it again. A persistent refusal therefore
//!   costs 5 requests and 7.5s on EVM (the first plus [`REFUSAL_RETRIES`]),
//!   and on Solana 6 requests for a 429 (the SDK's own retries are that
//!   chain's 429 backoff) and 5 for a 403.
//!
//! The chain adapters are behind features so a crate takes only its own
//! chain's SDK: [`evm::EvmRpc`] is the ethers `JsonRpcClient`, and
//! [`solana::rpc_client`] builds the SDK's `RpcClient` over a sender that
//! applies the same refusal rule. The crate holds no key, signs nothing and
//! knows nothing about channels; it is the part of a dial that is the same on
//! both chains.

use std::sync::Arc;
use std::time::Duration;

use connector_proxied_client::ProxiedClient;
use reqwest::header::{HeaderMap, RETRY_AFTER};
use url::Url;

#[cfg(feature = "evm")]
pub mod evm;
#[cfg(feature = "test-support")]
mod fake;
#[cfg(feature = "solana")]
pub mod solana;
#[cfg(feature = "xrpl")]
pub mod xrpl;
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
/// same credentials on one. So each network gets a fixed username of its
/// own (ADR 0086 decision 7, amending 0073 decision 3): one per EVM network,
/// named for its chain id, one per XRPL network (ADR 0087), and one for Solana. That keeps one exit from
/// seeing two networks' RPCs, and it keeps
/// settlement off the ILP wire's circuits, whose dials present no
/// credentials at all. The daemon still rotates a circuit after
/// `MaxCircuitDirtiness`, and that is left alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Circuit {
    /// `[settlement.eip155.<chain_id>]`: the backend, the channel-index
    /// syncer and the rate source of that one EVM network.
    EvmSettlement { chain_id: u64 },
    /// `[settlement.solana]`: the backend.
    SolanaSettlement,
    /// `[settlement.xrpl.<network_id>]`: the backend of that one XRPL network
    /// (ADR 0087), a circuit of its own like an EVM network's.
    XrplSettlement { network_id: u32 },
}

impl Circuit {
    /// The SOCKS username this circuit authenticates with. Fixed rather
    /// than per-process, so the pinning survives a restart the same way it
    /// survives a reconnect.
    pub fn socks_username(self) -> String {
        match self {
            Circuit::EvmSettlement { chain_id } => format!("toon-settlement-eip155-{chain_id}"),
            Circuit::SolanaSettlement => "toon-settlement-solana".to_string(),
            Circuit::XrplSettlement { network_id } => {
                format!("toon-settlement-xrpl-{network_id}")
            }
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
    http: Http,
    route: Route,
}

/// The shared client: a plain one, or one that rebuilds itself when its
/// proxy cannot be reached.
#[derive(Debug, Clone)]
enum Http {
    Direct(reqwest::Client),
    Circuit(Arc<ProxiedClient>),
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
    /// The proxy's host did not resolve, or `reqwest` refused it. Boot
    /// refuses a table that opts into the proxy when this happens.
    #[error("could not build the settlement HTTP client: {0}")]
    ProxyUnusable(String),
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
        let tune = move |builder: reqwest::ClientBuilder| {
            builder
                .connect_timeout(timeouts.connect)
                .timeout(timeouts.request)
                .pool_idle_timeout(timeouts.pool_idle)
        };
        let http = match &route {
            Route::Direct => {
                // Never inherit HTTP(S)_PROXY from the environment: which
                // way a settlement dial goes is the config file's to say
                // (ADR 0009), and a direct route that quietly went through
                // someone's proxy would be neither.
                let client = tune(reqwest::Client::builder())
                    .no_proxy()
                    .build()
                    .map_err(TransportError::Client)?;
                Http::Direct(client)
            }
            Route::Circuit { proxy, circuit } => {
                if proxy.scheme() != "socks5h" {
                    return Err(TransportError::ProxyNotSocks5h {
                        value: proxy.to_string(),
                    });
                }
                // Every request this client makes goes to the proxy: there
                // is no host it dials direct, so a proxy that is down fails
                // the request rather than routing around it (ADR 0073
                // decision 2). It is rebuilt when the proxy cannot be
                // reached, so one that moves is found again, and carries the
                // circuit's SOCKS credentials each time.
                let proxied = ProxiedClient::with_credentials(
                    proxy,
                    Some((
                        circuit.socks_username(),
                        circuit.socks_password().to_string(),
                    )),
                    tune,
                );
                // An unresolvable proxy refuses boot, as it always has.
                proxied.client().map_err(TransportError::ProxyUnusable)?;
                Http::Circuit(Arc::new(proxied))
            }
        };
        Ok(RpcTransport { url, http, route })
    }

    /// The endpoint every request goes to.
    pub fn url(&self) -> &Url {
        &self.url
    }

    /// The shared HTTP client: a handle on the table's pool. On a circuit it
    /// is the current one, so take it afresh for each request rather than
    /// holding it, and report a failed request to [`observe`](Self::observe).
    pub fn http(&self) -> Result<reqwest::Client, String> {
        match &self.http {
            Http::Direct(client) => Ok(client.clone()),
            // Only fails while the proxy's host does not resolve; the
            // request then fails closed with the reason.
            Http::Circuit(proxied) => proxied.client(),
        }
    }

    /// Report a failed request made on [`http`](Self::http). A circuit whose
    /// proxy could not be connected to is rebuilt, re-resolving the proxy's
    /// host; a failure beyond the proxy is not a reason to.
    pub fn observe(&self, error: &reqwest::Error) {
        if let Http::Circuit(proxied) = &self.http {
            proxied.observe(error);
        }
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

/// A refusal the transport has already retried and now reports: which
/// endpoint refused and with what status.
///
/// The endpoint is [`RpcTransport::endpoint`]'s form, never the URL, since a
/// keyed URL carries the API key.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{endpoint} refused the request with HTTP {status}")]
pub struct Refused {
    pub endpoint: String,
    pub status: u16,
}

/// Whether an error is a [`Refused`] the transport has reported, so a caller
/// can tell a refusal from every other failure on either chain.
pub trait AsRefused {
    fn refusal(&self) -> Option<Refused>;
}

/// What a watch loop remembers about an endpoint that is refusing, so it
/// logs the start of a run of refusals once and its end once, not once per
/// cycle.
///
/// A pass calls [`begin_pass`](RefusalLog::begin_pass), reports each
/// [`Refused`] it meets through [`refused`](RefusalLog::refused), and ends
/// with [`end_pass`](RefusalLog::end_pass). Any other failure is the
/// caller's to log as it always was.
#[derive(Debug, Default)]
pub struct RefusalLog {
    state: std::sync::Mutex<RefusalState>,
}

#[derive(Debug, Default)]
struct RefusalState {
    /// The refusal the last `warn` announced, until a pass runs clean.
    announced: Option<Refused>,
    refused_this_pass: bool,
}

impl RefusalLog {
    pub fn new() -> RefusalLog {
        RefusalLog::default()
    }

    pub fn begin_pass(&self) {
        self.state().refused_this_pass = false;
    }

    /// Record a refusal: a `warn` naming the endpoint and status when it
    /// starts, `debug` while it continues.
    pub fn refused(&self, refusal: &Refused) {
        let mut state = self.state();
        state.refused_this_pass = true;
        if state.announced.as_ref() == Some(refusal) {
            tracing::debug!(
                endpoint = %refusal.endpoint,
                status = refusal.status,
                "the settlement RPC endpoint is still refusing"
            );
        } else {
            tracing::warn!(
                endpoint = %refusal.endpoint,
                status = refusal.status,
                "the settlement RPC endpoint is refusing requests; reads are retried by the \
                 transport and the next pass tries again (further refusals are logged at \
                 debug until it answers)"
            );
            state.announced = Some(refusal.clone());
        }
    }

    /// End a pass. One that met no refusal after a run of them logs that the
    /// endpoint answers again.
    pub fn end_pass(&self) {
        let mut state = self.state();
        if !state.refused_this_pass {
            if let Some(refusal) = state.announced.take() {
                tracing::info!(
                    endpoint = %refusal.endpoint,
                    "the settlement RPC endpoint answers again"
                );
            }
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, RefusalState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// How many times a refused request is retried before the refusal is
/// reported (ADR 0073 decision 5). On EVM, which has one transport layer,
/// that is 4 retries and 5 requests.
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
/// 0.5s, 1s, 2s, 4s: 7.5s in all, well inside one request's 30s. This is
/// the whole price of a refusal: nothing above the transport retries it
/// again ([`retry_read`]). A
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
/// boot. A refusal is not one of the failures it retries.
pub const READ_RETRIES: u32 = 3;

/// The first wait between attempts. Each later one doubles it, so a read
/// that never succeeds costs 0.5s + 1s + 2s before it is reported.
const FIRST_READ_BACKOFF: Duration = Duration::from_millis(500);

/// Run a read, retrying a failure [`READ_RETRIES`] times with backoff
/// before returning the last one. A [`Refused`] is returned at once.
///
/// For the reads a node makes before anything depends on them: every one
/// boot makes (boot is where a circuit is youngest, and one lost round trip
/// used to fail the whole node), and the blockhash or nonce a write reads
/// before it signs. Every failure is retried, the node's own answers
/// included, except a refusal (HTTP 403 or 429): the transport has already
/// retried that [`REFUSAL_RETRIES`] times with backoff, so retrying it here
/// would multiply the requests a refused endpoint is sent (and its quota
/// spent) by `1 + READ_RETRIES` for the same answer.
///
/// **Never a send.** A transaction is not passed through this: a send whose
/// answer was lost is resolved by looking up what it did, never by sending
/// it again blind.
pub async fn retry_read<T, E, F, Fut>(mut call: F) -> Result<T, E>
where
    E: AsRefused,
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
{
    let mut attempt = 0;
    loop {
        match call().await {
            Ok(value) => return Ok(value),
            Err(error) if attempt >= READ_RETRIES || error.refusal().is_some() => {
                return Err(error)
            }
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
        let refused = RpcTransport::through(
            "https://rpc.example",
            &proxy,
            Circuit::EvmSettlement { chain_id: 31337 },
        );
        assert!(matches!(
            refused,
            Err(TransportError::ProxyNotSocks5h { .. })
        ));
        let proxy = Url::parse("socks5h://127.0.0.1:9050").unwrap();
        let built = RpcTransport::through(
            "https://rpc.example",
            &proxy,
            Circuit::EvmSettlement { chain_id: 31337 },
        )
        .unwrap();
        assert!(built.is_proxied());
    }

    #[derive(Debug, PartialEq)]
    enum Failure {
        Other(&'static str),
        Refusal,
    }

    impl AsRefused for Failure {
        fn refusal(&self) -> Option<Refused> {
            matches!(self, Failure::Refusal).then(|| Refused {
                endpoint: "http://rpc.example".to_string(),
                status: 429,
            })
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_read_is_retried_three_times_and_then_its_failure_is_reported() {
        let mut attempts = 0;
        let failed: Result<(), Failure> = retry_read(|| {
            attempts += 1;
            async { Err(Failure::Other("unreachable")) }
        })
        .await;
        assert_eq!(failed, Err(Failure::Other("unreachable")));
        assert_eq!(attempts, 1 + READ_RETRIES);

        let mut attempts = 0;
        let served: Result<u8, Failure> = retry_read(|| {
            attempts += 1;
            let result = if attempts < 3 {
                Err(Failure::Other("not yet"))
            } else {
                Ok(7)
            };
            async move { result }
        })
        .await;
        assert_eq!(served, Ok(7));
    }

    #[tokio::test(start_paused = true)]
    async fn a_refusal_the_transport_reported_is_not_retried_again() {
        let mut attempts = 0;
        let failed: Result<(), Failure> = retry_read(|| {
            attempts += 1;
            async { Err(Failure::Refusal) }
        })
        .await;
        assert_eq!(failed, Err(Failure::Refusal));
        assert_eq!(attempts, 1);
    }

    #[derive(Clone, Default)]
    struct Captured(Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for Captured {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Captured {
        type Writer = Captured;
        fn make_writer(&'a self) -> Captured {
            self.clone()
        }
    }

    #[test]
    fn a_run_of_refusals_is_warned_about_once_and_its_end_logged_once() {
        let captured = Captured::default();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_ansi(false)
            .with_writer(captured.clone())
            .finish();
        let refusal = Refused {
            endpoint: "https://rpc.example".to_string(),
            status: 429,
        };
        let log = RefusalLog::new();
        tracing::subscriber::with_default(subscriber, || {
            for _ in 0..5 {
                log.begin_pass();
                log.refused(&refusal);
                log.end_pass();
            }
            log.begin_pass();
            log.end_pass();
            log.begin_pass();
            log.end_pass();
            log.begin_pass();
            log.refused(&refusal);
            log.end_pass();
        });
        let text = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
        let count = |level: &str| text.lines().filter(|l| l.contains(level)).count();
        assert_eq!(count(" WARN "), 2, "one per run of refusals:\n{text}");
        assert_eq!(count(" DEBUG "), 4, "{text}");
        assert_eq!(count(" INFO "), 1, "one when calls succeed again:\n{text}");
        assert!(text.contains("https://rpc.example"), "{text}");
        assert!(text.contains("429"), "{text}");
    }

    #[test]
    fn each_network_pins_its_own_circuit() {
        let base = Circuit::EvmSettlement { chain_id: 8453 };
        let arbitrum = Circuit::EvmSettlement { chain_id: 42161 };
        assert_eq!(base.socks_username(), "toon-settlement-eip155-8453");
        assert_ne!(base.socks_username(), arbitrum.socks_username());
        assert_ne!(
            base.socks_username(),
            Circuit::SolanaSettlement.socks_username()
        );
        // An XRPL network's is named for its network id (ADR 0087).
        let xrpl = Circuit::XrplSettlement { network_id: 1 };
        assert_eq!(xrpl.socks_username(), "toon-settlement-xrpl-1");
        assert_ne!(
            xrpl.socks_username(),
            Circuit::XrplSettlement { network_id: 0 }.socks_username()
        );
        // Solana's is the one it has always been.
        assert_eq!(
            Circuit::SolanaSettlement.socks_username(),
            "toon-settlement-solana"
        );
    }
}
