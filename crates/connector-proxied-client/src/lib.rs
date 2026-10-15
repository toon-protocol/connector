//! The one cached `reqwest` client that dials through the node's `socks_proxy`
//! (ADR 0070), and the one place that builds a `reqwest::Proxy` for it.
//!
//! `reqwest::Proxy::all("socks5h://host:port")` resolves the **proxy's** host
//! at that call, with a blocking local lookup, and keeps one `SocketAddr`; the
//! `h` only makes the *target* resolve remotely. A client built once and kept
//! for the life of the process therefore has two failures: a proxy that was
//! not resolvable at construction (the daemon's container is not up yet) leaves
//! a cached error that outlives the cause, and a proxy that moved afterwards
//! (the daemon was recreated with a new address) is dialled at its old one
//! until restart.
//!
//! [`ProxiedClient`] keeps the pooled client -- the ILP-over-HTTP carriage
//! sends every packet down it and must not lose keep-alive -- and rebuilds it
//! in the two cases that need it: the last build failed, or a request through
//! it failed because the *proxy* could not be connected to ([`ProxiedClient::observe`]).
//! A failure beyond the proxy (the hidden service is down, or the proxy
//! refused the CONNECT for the target) is not a reason to rebuild and does not.
//!
//! It fails closed. There is no direct client in here to fall back to: while
//! the proxy is unusable, [`ProxiedClient::client`] returns the reason.

use std::fmt;
use std::sync::Mutex;

use reqwest::ClientBuilder;
use url::Url;

type Tune = Box<dyn Fn(ClientBuilder) -> ClientBuilder + Send + Sync>;

/// A client through one SOCKS proxy that re-resolves the proxy's host when it
/// cannot be reached. See the crate docs.
pub struct ProxiedClient {
    proxy: Url,
    credentials: Option<(String, String)>,
    tune: Tune,
    slot: Mutex<Slot>,
}

struct Slot {
    client: Result<reqwest::Client, String>,
    builds: u32,
}

impl ProxiedClient {
    /// A client through `proxy`, with `tune` applied to its builder (timeouts,
    /// redirect policy) on every build. Builds once now; a failure is kept and
    /// retried by the next [`client`](Self::client), not frozen.
    pub fn new(
        proxy: &Url,
        tune: impl Fn(ClientBuilder) -> ClientBuilder + Send + Sync + 'static,
    ) -> ProxiedClient {
        ProxiedClient::with_credentials(proxy, None, tune)
    }

    /// As [`new`](Self::new), authenticating to the proxy with SOCKS
    /// username/password -- what pins a settlement circuit (ADR 0073). The
    /// rebuilt client sends the same pair.
    pub fn with_credentials(
        proxy: &Url,
        credentials: Option<(String, String)>,
        tune: impl Fn(ClientBuilder) -> ClientBuilder + Send + Sync + 'static,
    ) -> ProxiedClient {
        let tune: Tune = Box::new(tune);
        let client = build(proxy, credentials.as_ref(), &tune);
        ProxiedClient {
            proxy: proxy.clone(),
            credentials,
            tune,
            slot: Mutex::new(Slot { client, builds: 1 }),
        }
    }

    /// The proxy this client dials through.
    pub fn proxy(&self) -> &Url {
        &self.proxy
    }

    /// The current client, built again first if the last build failed. The
    /// error names the configured `socks_proxy` value.
    pub fn client(&self) -> Result<reqwest::Client, String> {
        let mut slot = self.slot.lock().expect("proxied client lock");
        if slot.client.is_err() {
            slot.client = build(&self.proxy, self.credentials.as_ref(), &self.tune);
            slot.builds += 1;
        }
        slot.client.clone()
    }

    /// Report a failed request made on a client from [`client`](Self::client).
    /// When the failure was in reaching the proxy itself, the client is
    /// rebuilt, re-resolving the proxy's host, so the next dial recovers.
    /// Anything else -- the target unreachable, a SOCKS refusal for it, a
    /// timeout -- leaves the client alone.
    pub fn observe(&self, error: &reqwest::Error) {
        if !proxy_unreachable(error) {
            return;
        }
        let mut slot = self.slot.lock().expect("proxied client lock");
        slot.client = build(&self.proxy, self.credentials.as_ref(), &self.tune);
        slot.builds += 1;
    }

    /// How many times the client has been built, the first included.
    pub fn builds(&self) -> u32 {
        self.slot.lock().expect("proxied client lock").builds
    }
}

impl fmt::Debug for ProxiedClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProxiedClient")
            .field("proxy", &self.proxy.as_str())
            .finish_non_exhaustive()
    }
}

fn build(
    proxy: &Url,
    credentials: Option<&(String, String)>,
    tune: &Tune,
) -> Result<reqwest::Client, String> {
    let attempt = || {
        let mut socks = reqwest::Proxy::all(proxy.as_str())?;
        if let Some((username, password)) = credentials {
            socks = socks.basic_auth(username, password);
        }
        tune(reqwest::Client::builder()).proxy(socks).build()
    };
    attempt().map_err(|error| format!("socks_proxy '{proxy}' could not be used: {error}"))
}

/// Whether `error` is a failure to reach the proxy itself, as opposed to one
/// past it.
///
/// `reqwest` 0.11 flattens the SOCKS dial's error into text
/// (`socks connect error: ...`), so the text is what there is to read. A
/// failure to reach the proxy is `tokio-socks`' `Io` variant, which prints as
/// the OS error (`Connection refused (os error 111)`, a reset, an early EOF
/// mid-handshake) or `Proxy server unreachable`; a refusal *by* the proxy is a reply code, which prints as a
/// bare phrase (`Host unreachable`, `Connection refused`) and has no OS error.
fn proxy_unreachable(error: &reqwest::Error) -> bool {
    if !error.is_connect() {
        return false;
    }
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(cause) = source {
        let text = cause.to_string();
        if let Some((_, tail)) = text.split_once("socks connect error: ") {
            return tail.contains("(os error ")
                || tail.contains("Proxy server unreachable")
                || tail.contains("unexpected end of file");
        }
        source = cause.source();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// A SOCKS5 server (RFC 1928; username/password accepted if offered) that
    /// answers every CONNECT with `reply`, then -- for a success -- answers
    /// the HTTP request that follows with `ok`. Returns the usernames seen.
    async fn socks_server(
        listener: TcpListener,
        reply: u8,
    ) -> std::sync::Arc<Mutex<Vec<Option<String>>>> {
        let seen = std::sync::Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let log = log.clone();
                tokio::spawn(async move {
                    let mut head = [0u8; 2];
                    stream.read_exact(&mut head).await?;
                    let mut methods = vec![0u8; head[1] as usize];
                    stream.read_exact(&mut methods).await?;
                    let mut username = None;
                    if methods.contains(&0x02) {
                        stream.write_all(&[5, 2]).await?;
                        let mut ver = [0u8; 2];
                        stream.read_exact(&mut ver).await?;
                        let mut name = vec![0u8; ver[1] as usize];
                        stream.read_exact(&mut name).await?;
                        let mut plen = [0u8; 1];
                        stream.read_exact(&mut plen).await?;
                        let mut pass = vec![0u8; plen[0] as usize];
                        stream.read_exact(&mut pass).await?;
                        stream.write_all(&[1, 0]).await?;
                        username = Some(String::from_utf8_lossy(&name).into_owned());
                    } else {
                        stream.write_all(&[5, 0]).await?;
                    }
                    let mut request = [0u8; 4];
                    stream.read_exact(&mut request).await?;
                    let skip = match request[3] {
                        1 => 4 + 2,
                        4 => 16 + 2,
                        _ => {
                            let mut len = [0u8; 1];
                            stream.read_exact(&mut len).await?;
                            len[0] as usize + 2
                        }
                    };
                    let mut rest = vec![0u8; skip];
                    stream.read_exact(&mut rest).await?;
                    log.lock().unwrap().push(username);
                    stream
                        .write_all(&[5, reply, 0, 1, 0, 0, 0, 0, 0, 0])
                        .await?;
                    if reply == 0 {
                        let mut buf = [0u8; 1024];
                        let _ = stream.read(&mut buf).await?;
                        stream
                            .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok")
                            .await?;
                    }
                    std::io::Result::Ok(())
                });
            }
        });
        seen
    }

    fn url_for(host: &str, port: u16) -> Url {
        Url::parse(&format!("socks5h://{host}:{port}")).unwrap()
    }

    async fn get(client: &ProxiedClient) -> Result<String, String> {
        let http = client.client()?;
        match http.get("http://hidden.onion/").send().await {
            Ok(response) => Ok(response.text().await.unwrap()),
            Err(error) => {
                client.observe(&error);
                Err(error.to_string())
            }
        }
    }

    async fn free_port() -> (u16, SocketAddr) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        (addr.port(), addr)
    }

    #[tokio::test]
    async fn a_proxy_host_that_never_resolved_is_tried_again_not_cached() {
        let proxy = Url::parse("socks5h://proxy.invalid:9050").unwrap();
        let client = ProxiedClient::new(&proxy, |b| b);
        let first = client.client().expect_err("`.invalid` never resolves");
        assert!(first.contains("socks5h://proxy.invalid:9050"), "{first}");
        let second = client.client().expect_err("still does not");
        assert!(second.contains("socks_proxy"), "{second}");
        assert_eq!(client.builds(), 3, "each call tried to build it again");
    }

    #[tokio::test]
    async fn a_proxy_that_starts_listening_is_dialled_without_a_restart() {
        let (port, _) = free_port().await; // listener dropped: nothing listens
        let client = ProxiedClient::new(&url_for("localhost", port), |b| b);

        let down = get(&client).await.expect_err("nothing listens");
        assert!(!down.is_empty());
        assert_eq!(
            client.builds(),
            2,
            "an unreachable proxy rebuilt the client"
        );

        // `localhost` resolves to whichever family the OS lists first; bind
        // by name so the listener is on the address the client resolves to.
        let listener = TcpListener::bind(("localhost", port)).await.unwrap();
        let seen = socks_server(listener, 0).await;
        assert_eq!(get(&client).await.as_deref(), Ok("ok"));
        assert_eq!(seen.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_failure_past_the_proxy_does_not_rebuild_the_client() {
        // 0x04 host unreachable, 0x05 connection refused: the proxy answered.
        for reply in [4u8, 5] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            socks_server(listener, reply).await;
            let client = ProxiedClient::new(&url_for("127.0.0.1", port), |b| b);
            for _ in 0..3 {
                get(&client).await.expect_err("the target is refused");
            }
            assert_eq!(
                client.builds(),
                1,
                "reply {reply} is not the proxy's absence"
            );
        }
    }

    #[tokio::test]
    async fn the_rebuilt_client_still_sends_the_circuits_username() {
        let (port, _) = free_port().await;
        let credentials = Some(("toon-circuit".to_string(), "pinned".to_string()));
        let client =
            ProxiedClient::with_credentials(&url_for("localhost", port), credentials, |b| b);
        get(&client).await.expect_err("nothing listens");

        // `localhost` resolves to whichever family the OS lists first; bind
        // by name so the listener is on the address the client resolves to.
        let listener = TcpListener::bind(("localhost", port)).await.unwrap();
        let seen = socks_server(listener, 0).await;
        assert_eq!(get(&client).await.as_deref(), Ok("ok"));
        assert_eq!(
            seen.lock().unwrap().as_slice(),
            &[Some("toon-circuit".to_string())]
        );
    }
}
