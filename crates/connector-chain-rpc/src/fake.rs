//! A JSON-RPC-over-HTTP server that answers, fails, stalls or drops as a
//! test scripts it (ADR 0073).
//!
//! Every hardening seam ADR 0073 decision 5 requires is about what a client
//! does when an endpoint misbehaves: a poll that times out, a 403 from a
//! shared exit address, a connection that dies after the node accepted a
//! transaction but before its answer came back. None of that can be produced
//! against `anvil` or `solana-test-validator`, which are too well behaved,
//! and a mock of the client would test the mock. So this is a real server
//! speaking real HTTP/1.1 on a real socket, and the client under test cannot
//! tell it from an RPC node having a bad day.
//!
//! It is a fake, not a mock (ADR 0007): it upholds the transport's contract
//! (one JSON-RPC request in, one HTTP response or a dead connection out) and
//! a test asserts on what the **client** concluded, not on which calls were
//! made. [`FakeRpc::calls`] exists for the few tests whose subject is traffic
//! itself, such as "no request reached the endpoint direct".
//!
//! In front of a real chain ([`FakeRpc::spawn_in_front_of`]), a script can
//! pass a request through ([`RpcReply::Forward`]) or pass it through and then
//! **lose the answer** ([`RpcReply::ForwardThenDrop`]). The second is the
//! ambiguous send, reproduced exactly: the chain has the transaction and the
//! client has no idea.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// One request the fake received.
#[derive(Debug, Clone)]
pub struct RpcCall {
    pub method: String,
    pub params: Value,
    /// How many earlier calls to the same method this server has seen, so a
    /// script can say "fail the first two polls".
    pub nth: usize,
}

/// What the fake does with one request.
#[derive(Debug, Clone)]
pub enum RpcReply {
    /// `{"result": <value>}`.
    Result(Value),
    /// `{"error": {"code", "message"}}`: the node's own answer.
    Error { code: i64, message: String },
    /// A bare HTTP status with a non-JSON body, the way a CDN or a rate
    /// limiter answers.
    Status(u16),
    /// Never answer. The client's request timeout is what ends it.
    Hang,
    /// Close the connection without answering.
    Drop,
    /// Wait, then do the inner reply.
    Slow(Duration, Box<RpcReply>),
    /// Pass the request to the upstream chain and return its answer.
    Forward,
    /// Pass the request to the upstream chain, then close the connection
    /// without returning the answer.
    ForwardThenDrop,
}

type Script = dyn Fn(&RpcCall) -> RpcReply + Send + Sync;

/// A scripted JSON-RPC server on `127.0.0.1`.
pub struct FakeRpc {
    addr: std::net::SocketAddr,
    calls: Arc<Mutex<Vec<RpcCall>>>,
    _shutdown: tokio::sync::oneshot::Sender<()>,
}

impl FakeRpc {
    /// A server that answers every request with `script`'s reply.
    pub async fn spawn(script: impl Fn(&RpcCall) -> RpcReply + Send + Sync + 'static) -> FakeRpc {
        FakeRpc::start(None, Arc::new(script)).await
    }

    /// A server in front of `upstream` (a real chain's RPC URL), where
    /// `script` decides per request whether to pass it through, lose the
    /// answer, or misbehave.
    pub async fn spawn_in_front_of(
        upstream: &str,
        script: impl Fn(&RpcCall) -> RpcReply + Send + Sync + 'static,
    ) -> FakeRpc {
        FakeRpc::start(Some(upstream.to_string()), Arc::new(script)).await
    }

    async fn start(upstream: Option<String>, script: Arc<Script>) -> FakeRpc {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("a loopback port for the fake RPC");
        let addr = listener.local_addr().expect("the bound address");
        let calls = Arc::new(Mutex::new(Vec::new()));
        let (shutdown, mut stopped) = tokio::sync::oneshot::channel();
        let forwarder = reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("a plain HTTP client for forwarding");

        let seen = Arc::clone(&calls);
        tokio::spawn(async move {
            loop {
                let accepted = tokio::select! {
                    accepted = listener.accept() => accepted,
                    _ = &mut stopped => return,
                };
                let Ok((stream, _)) = accepted else { return };
                let connection = Connection {
                    script: Arc::clone(&script),
                    calls: Arc::clone(&seen),
                    upstream: upstream.clone(),
                    forwarder: forwarder.clone(),
                };
                tokio::spawn(async move {
                    let _ = connection.serve(stream).await;
                });
            }
        });

        FakeRpc {
            addr,
            calls,
            _shutdown: shutdown,
        }
    }

    /// `http://127.0.0.1:<port>`.
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// The socket address, for a SOCKS test server's route table.
    pub fn addr(&self) -> std::net::SocketAddr {
        self.addr
    }

    /// Every request received, in arrival order.
    pub fn calls(&self) -> Vec<RpcCall> {
        self.calls.lock().expect("calls lock").clone()
    }

    /// How many requests named `method`.
    pub fn count(&self, method: &str) -> usize {
        self.calls()
            .iter()
            .filter(|call| call.method == method)
            .count()
    }
}

struct Connection {
    script: Arc<Script>,
    calls: Arc<Mutex<Vec<RpcCall>>>,
    upstream: Option<String>,
    forwarder: reqwest::Client,
}

impl Connection {
    /// Requests on one keep-alive connection, until the client hangs up or
    /// the script drops it.
    async fn serve(self, mut stream: TcpStream) -> std::io::Result<()> {
        loop {
            let Some(body) = read_request(&mut stream).await? else {
                return Ok(());
            };
            let request: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            let method = request["method"].as_str().unwrap_or_default().to_string();
            let call = {
                let mut calls = self.calls.lock().expect("calls lock");
                let nth = calls.iter().filter(|call| call.method == method).count();
                let call = RpcCall {
                    method,
                    params: request["params"].clone(),
                    nth,
                };
                calls.push(call.clone());
                call
            };
            let mut reply = (self.script)(&call);
            while let RpcReply::Slow(wait, inner) = reply {
                tokio::time::sleep(wait).await;
                reply = *inner;
            }
            let id = request["id"].clone();
            let answer = match reply {
                RpcReply::Result(result) => {
                    json_response(json!({"jsonrpc": "2.0", "id": id, "result": result}))
                }
                RpcReply::Error { code, message } => json_response(
                    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}),
                ),
                RpcReply::Status(status) => status_response(status),
                RpcReply::Hang => {
                    std::future::pending::<()>().await;
                    unreachable!("a pending future never resolves")
                }
                RpcReply::Drop => return Ok(()),
                RpcReply::Forward => raw_json_response(&self.forward(body).await),
                RpcReply::ForwardThenDrop => {
                    self.forward(body).await;
                    return Ok(());
                }
                RpcReply::Slow(..) => unreachable!("unwrapped above"),
            };
            stream.write_all(&answer).await?;
        }
    }

    async fn forward(&self, body: Vec<u8>) -> Vec<u8> {
        let upstream = self
            .upstream
            .as_deref()
            .expect("RpcReply::Forward needs FakeRpc::spawn_in_front_of");
        self.forwarder
            .post(upstream)
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await
            .expect("the upstream chain answers")
            .bytes()
            .await
            .expect("the upstream chain's body")
            .to_vec()
    }
}

/// One HTTP/1.1 request's body, or `None` when the client closed the
/// connection between requests.
async fn read_request(stream: &mut TcpStream) -> std::io::Result<Option<Vec<u8>>> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        if let Some(end) = find(&buffer, b"\r\n\r\n") {
            break end + 4;
        }
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Ok(None);
        }
        buffer.extend_from_slice(&chunk[..read]);
    };
    let headers = String::from_utf8_lossy(&buffer[..header_end]).to_ascii_lowercase();
    let length = headers
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = buffer[header_end..].to_vec();
    while body.len() < length {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Ok(None);
        }
        body.extend_from_slice(&chunk[..read]);
    }
    body.truncate(length);
    Ok(Some(body))
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn json_response(value: Value) -> Vec<u8> {
    raw_json_response(value.to_string().as_bytes())
}

fn raw_json_response(body: &[u8]) -> Vec<u8> {
    let mut response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(body);
    response
}

fn status_response(status: u16) -> Vec<u8> {
    let body = format!("<html><body>{status}</body></html>");
    format!(
        "HTTP/1.1 {status} Refused\r\ncontent-type: text/html\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}
