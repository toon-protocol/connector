//! A **real SOCKS5 server** (RFC 1928, no authentication), for the one
//! question no other seam in this repository can answer: *which socket did
//! that dial reach?*
//!
//! # Why this exists at all
//!
//! ADR 0070 selects a proxy by host: an endpoint whose host ends in
//! `.onion` is dialed through the configured SOCKS5 proxy, and every other
//! endpoint is dialed direct. `Config::load` cannot assert that -- it knows
//! what an endpoint *is*, never where a connection *went* -- and the two
//! carriage crates' existing carriage tests explicitly cannot either: their
//! own headers say the socket is not exercised, and a test whose whole
//! subject is which socket cannot live where the socket is stubbed.
//!
//! So the assertion has to be made against something that really accepts a
//! connection. This is that something, and it is a legitimate test subject
//! rather than a mock (ADR 0007): it **upholds SOCKS5's contract** -- it
//! negotiates, it connects to the target, and it copies bytes in both
//! directions until either end hangs up -- and it asserts no sequence of
//! calls. What a test reads off it afterwards ([`Socks5TestServer::targets`])
//! is not a record of which functions ran; it is the destination the
//! protocol itself carries in its CONNECT request, which is exactly the
//! fact under test.
//!
//! # Why the target table, and why it is the point
//!
//! A `.onion` name resolves nowhere. That is the whole reason ADR 0070
//! requires `socks5h://` rather than `socks5://`: the `h` means the client
//! sends the **name** and the proxy resolves it, and no local resolver can
//! resolve a `.onion`. So this server is handed a table from requested
//! `host:port` to a real address, which is the same job the onion daemon
//! does with a circuit -- and a client that resolved locally would never
//! reach the table at all, because its lookup would have failed first. A
//! test that finds the onion name in [`Socks5TestServer::targets`] has
//! therefore proved two things at once: the dial went through the proxy,
//! and it went through as a name.
//!
//! # Not `#[cfg(test)]`
//!
//! This crate's other `test_support` is private to its own `mod tests`.
//! This one is compiled always and exported, for the same reason
//! [`crate::FakeAppClient`] and [`crate::TestClock`] are: both carriage
//! crates' tests need it, and a `#[cfg(test)]` item is invisible outside
//! the crate that declares it. `connector-runtime` is the one crate both
//! carriages already depend on, and it is already the shared home for the
//! in-process transport, the fake app client and the test clock.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use url::Url;

/// SOCKS5, and only SOCKS5 (RFC 1928 §3). A client that greets with
/// anything else is a client that would not have reached a real proxy
/// either.
const VERSION: u8 = 0x05;

/// "No authentication required" (§3): the method this server offers, and
/// the one every dial in this repository presents, since a proxy URL here
/// carries no credentials.
const NO_AUTH: u8 = 0x00;

/// CONNECT (§4). BIND and UDP ASSOCIATE are not implemented and are
/// refused with `COMMAND_NOT_SUPPORTED` rather than half-served -- nothing
/// this connector dials uses either, and a server that quietly mishandled
/// one would make a future failure look like a network problem.
const CMD_CONNECT: u8 = 0x01;

const ATYP_IPV4: u8 = 0x01;
const ATYP_DOMAIN: u8 = 0x03;
const ATYP_IPV6: u8 = 0x04;

const REP_SUCCEEDED: u8 = 0x00;
const REP_HOST_UNREACHABLE: u8 = 0x04;
const REP_COMMAND_NOT_SUPPORTED: u8 = 0x07;
const REP_ADDRESS_TYPE_NOT_SUPPORTED: u8 = 0x08;

/// A SOCKS5 proxy on loopback that really proxies bytes.
///
/// Dropping it stops accepting new connections; connections already
/// established finish on their own tasks, which is what lets a test drop
/// the server while a response is still in flight without turning that
/// into a spurious failure.
pub struct Socks5TestServer {
    addr: SocketAddr,
    targets: Arc<Mutex<Vec<String>>>,
    /// Dropped with the server, which ends the accept loop. Held rather
    /// than read.
    _shutdown: tokio::sync::oneshot::Sender<()>,
}

impl Socks5TestServer {
    /// Start a proxy on `127.0.0.1:0`, resolving each requested
    /// `"host:port"` through `routes`.
    ///
    /// A CONNECT for a target with no row is answered
    /// `REP_HOST_UNREACHABLE` -- the same answer a real proxy gives for a
    /// name it cannot reach -- and the target is still recorded, so a test
    /// asserting *that a dial arrived here* does not need the dial to have
    /// succeeded.
    pub async fn spawn(routes: HashMap<String, SocketAddr>) -> Socks5TestServer {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("a loopback port for the SOCKS5 test server");
        let addr = listener.local_addr().expect("the bound address");
        let targets = Arc::new(Mutex::new(Vec::new()));
        let (shutdown, mut stopped) = tokio::sync::oneshot::channel();

        let routes = Arc::new(routes);
        let seen = Arc::clone(&targets);
        tokio::spawn(async move {
            loop {
                let accepted = tokio::select! {
                    accepted = listener.accept() => accepted,
                    _ = &mut stopped => return,
                };
                let Ok((stream, _)) = accepted else { return };
                let routes = Arc::clone(&routes);
                let seen = Arc::clone(&seen);
                tokio::spawn(async move {
                    // A client that hangs up mid-handshake is a client that
                    // hung up: nothing to report, and nothing a test could
                    // do with a report of it.
                    let _ = serve(stream, routes, seen).await;
                });
            }
        });

        Socks5TestServer {
            addr,
            targets,
            _shutdown: shutdown,
        }
    }

    /// A proxy with no routes at all: every CONNECT is recorded and then
    /// refused. Enough for a test whose assertion is "the dial arrived
    /// here", or "the dial did not".
    pub async fn spawn_recording_only() -> Socks5TestServer {
        Socks5TestServer::spawn(HashMap::new()).await
    }

    /// Where the proxy listens.
    #[must_use]
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// This proxy as the URL an operator would write into `socks_proxy`.
    ///
    /// `socks5h`, always: it is the only scheme the config key accepts, and
    /// the only one whose client sends a name for the proxy to resolve. A
    /// test that could ask for `socks5://` here would be a test that could
    /// pass while proving the opposite of what it says.
    #[must_use]
    pub fn proxy_url(&self) -> Url {
        Url::parse(&format!("socks5h://{}", self.addr)).expect("a loopback socket address is a URL")
    }

    /// Every CONNECT target this proxy was asked for, as `"host:port"`, in
    /// arrival order.
    ///
    /// For a `socks5h` client the host is the **name** as written -- which
    /// is what makes `"...onion:80"` appearing here evidence that the dial
    /// both traversed the proxy and deferred resolution to it.
    #[must_use]
    pub fn targets(&self) -> Vec<String> {
        self.targets.lock().expect("targets lock").clone()
    }

    /// Whether any CONNECT named a host ending in `suffix`. The shape most
    /// assertions want, so a test does not have to re-derive it from
    /// [`Socks5TestServer::targets`].
    #[must_use]
    pub fn saw_host_ending_in(&self, suffix: &str) -> bool {
        self.targets().iter().any(|target| {
            target
                .rsplit_once(':')
                .is_some_and(|(h, _)| h.ends_with(suffix))
        })
    }
}

/// One client connection: greet, read the CONNECT, answer it, then copy.
async fn serve(
    mut client: TcpStream,
    routes: Arc<HashMap<String, SocketAddr>>,
    seen: Arc<Mutex<Vec<String>>>,
) -> std::io::Result<()> {
    // §3: VER, NMETHODS, METHODS...
    let mut greeting = [0u8; 2];
    client.read_exact(&mut greeting).await?;
    if greeting[0] != VERSION {
        return Ok(());
    }
    let mut methods = vec![0u8; greeting[1] as usize];
    client.read_exact(&mut methods).await?;
    client.write_all(&[VERSION, NO_AUTH]).await?;

    // §4: VER, CMD, RSV, ATYP, DST.ADDR, DST.PORT
    let mut request = [0u8; 4];
    client.read_exact(&mut request).await?;
    if request[0] != VERSION {
        return Ok(());
    }
    let host = match request[3] {
        ATYP_IPV4 => {
            let mut octets = [0u8; 4];
            client.read_exact(&mut octets).await?;
            std::net::Ipv4Addr::from(octets).to_string()
        }
        ATYP_DOMAIN => {
            let mut len = [0u8; 1];
            client.read_exact(&mut len).await?;
            let mut name = vec![0u8; len[0] as usize];
            client.read_exact(&mut name).await?;
            String::from_utf8_lossy(&name).into_owned()
        }
        ATYP_IPV6 => {
            let mut octets = [0u8; 16];
            client.read_exact(&mut octets).await?;
            std::net::Ipv6Addr::from(octets).to_string()
        }
        _ => {
            reply(&mut client, REP_ADDRESS_TYPE_NOT_SUPPORTED).await?;
            return Ok(());
        }
    };
    let mut port = [0u8; 2];
    client.read_exact(&mut port).await?;
    let target = format!("{host}:{}", u16::from_be_bytes(port));

    // Recorded before the CONNECT is judged, so a target this server cannot
    // reach is still a target it was asked for.
    seen.lock().expect("targets lock").push(target.clone());

    if request[1] != CMD_CONNECT {
        reply(&mut client, REP_COMMAND_NOT_SUPPORTED).await?;
        return Ok(());
    }
    let Some(upstream) = routes.get(&target).copied() else {
        reply(&mut client, REP_HOST_UNREACHABLE).await?;
        return Ok(());
    };
    let Ok(mut server) = TcpStream::connect(upstream).await else {
        reply(&mut client, REP_HOST_UNREACHABLE).await?;
        return Ok(());
    };

    reply(&mut client, REP_SUCCEEDED).await?;
    // The proxying itself. Whatever the two ends say to each other is none
    // of this server's business -- it is a byte copy, which is the whole of
    // what a SOCKS5 proxy owes after a successful CONNECT.
    tokio::io::copy_bidirectional(&mut client, &mut server)
        .await
        .map(|_| ())
}

/// A reply with a zero `BND.ADDR`/`BND.PORT` (§6). Every client this
/// repository dials with ignores the bound address on a CONNECT reply, and
/// a real proxy is free to report `0.0.0.0:0` there.
async fn reply(client: &mut TcpStream, code: u8) -> std::io::Result<()> {
    client
        .write_all(&[VERSION, code, 0x00, ATYP_IPV4, 0, 0, 0, 0, 0, 0])
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fake really proxies: a byte written by a client through the
    /// proxy arrives at an upstream the client never connected to, and the
    /// upstream's answer comes back. Without this, every test that depends
    /// on the fake could be green because nothing reached anything.
    #[tokio::test]
    async fn it_really_proxies_bytes_to_a_named_target() {
        let upstream = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
        let upstream_addr = upstream.local_addr().expect("addr");
        tokio::spawn(async move {
            let (mut stream, _) = upstream.accept().await.expect("accept");
            let mut asked = [0u8; 4];
            stream.read_exact(&mut asked).await.expect("read");
            assert_eq!(&asked, b"ping");
            stream.write_all(b"pong").await.expect("write");
        });

        // A name no resolver on this machine can resolve, which is the
        // point: only a proxy holding the table can reach it.
        let name = "nowhere.onion:80".to_string();
        let proxy = Socks5TestServer::spawn(HashMap::from([(name.clone(), upstream_addr)])).await;

        let mut client = TcpStream::connect(proxy.addr()).await.expect("dial proxy");
        client
            .write_all(&[VERSION, 1, NO_AUTH])
            .await
            .expect("greet");
        let mut chosen = [0u8; 2];
        client.read_exact(&mut chosen).await.expect("method");
        assert_eq!(chosen, [VERSION, NO_AUTH]);

        let host = b"nowhere.onion";
        let mut connect = vec![VERSION, CMD_CONNECT, 0x00, ATYP_DOMAIN, host.len() as u8];
        connect.extend_from_slice(host);
        connect.extend_from_slice(&80u16.to_be_bytes());
        client.write_all(&connect).await.expect("connect");
        let mut answer = [0u8; 10];
        client.read_exact(&mut answer).await.expect("reply");
        assert_eq!(answer[1], REP_SUCCEEDED, "the target is in the table");

        client.write_all(b"ping").await.expect("write through");
        let mut back = [0u8; 4];
        client.read_exact(&mut back).await.expect("read through");
        assert_eq!(&back, b"pong");

        assert_eq!(proxy.targets(), vec![name]);
        assert!(proxy.saw_host_ending_in(".onion"));
    }

    /// A target with no row is refused rather than silently accepted, and
    /// is still recorded -- so a test whose assertion is "the dial arrived
    /// at the proxy" does not have to arrange for it to succeed.
    #[tokio::test]
    async fn an_unrouted_target_is_refused_and_still_recorded() {
        let proxy = Socks5TestServer::spawn_recording_only().await;

        let mut client = TcpStream::connect(proxy.addr()).await.expect("dial proxy");
        client
            .write_all(&[VERSION, 1, NO_AUTH])
            .await
            .expect("greet");
        let mut chosen = [0u8; 2];
        client.read_exact(&mut chosen).await.expect("method");

        let host = b"unrouted.onion";
        let mut connect = vec![VERSION, CMD_CONNECT, 0x00, ATYP_DOMAIN, host.len() as u8];
        connect.extend_from_slice(host);
        connect.extend_from_slice(&443u16.to_be_bytes());
        client.write_all(&connect).await.expect("connect");
        let mut answer = [0u8; 10];
        client.read_exact(&mut answer).await.expect("reply");

        assert_eq!(answer[1], REP_HOST_UNREACHABLE);
        assert_eq!(proxy.targets(), vec!["unrouted.onion:443".to_string()]);
    }

    /// `proxy_url` is the value an operator writes, and it is `socks5h`.
    #[tokio::test]
    async fn the_proxy_url_is_socks5h() {
        let proxy = Socks5TestServer::spawn_recording_only().await;
        assert_eq!(proxy.proxy_url().scheme(), "socks5h");
        assert_eq!(
            proxy.proxy_url().port(),
            Some(proxy.addr().port()),
            "and it names the port this server actually listens on"
        );
    }
}
