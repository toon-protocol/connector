//! The ethers `JsonRpcClient` every `[settlement.evm]` client rides: the
//! backend, the channel-index syncer and the rate source (ADR 0073).
//!
//! It exists because ethers' own `Http` transport cannot express two of
//! ADR 0073's rules:
//!
//! - it is built on `reqwest::Client::new()`, so it has no timeout;
//! - it never looks at the HTTP status. A 403 or 429 arrives as "could not
//!   deserialize this HTML" and is indistinguishable from a broken node.
//!
//! [`EvmRpc`] posts the same JSON-RPC 2.0 body over the table's shared
//! [`RpcTransport`], retries a refusal with backoff ([`crate::refusal_backoff`])
//! and otherwise decodes the answer the way ethers does. Its error keeps one
//! distinction every caller needs: a JSON-RPC error **answered** by the node
//! is a definitive answer about the request, and everything else (a timeout,
//! a reset, a refusal that outlasted its retries) says nothing about whether
//! the request took effect. `ProviderError::as_error_response` returns
//! `Some` for exactly the first kind, so a caller holding the ethers error
//! asks it through [`answered`].

use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use ethers::providers::{JsonRpcClient, JsonRpcError, Provider, ProviderError, RpcError};
use serde::de::DeserializeOwned;
use serde::de::Error as _;
use serde::Serialize;

use crate::{is_refusal, refusal_backoff, RpcTransport};

/// A JSON-RPC client over one settlement table's [`RpcTransport`].
#[derive(Debug)]
pub struct EvmRpc {
    transport: RpcTransport,
    next_id: AtomicU64,
}

impl EvmRpc {
    pub fn new(transport: RpcTransport) -> EvmRpc {
        EvmRpc {
            transport,
            next_id: AtomicU64::new(1),
        }
    }

    /// An ethers `Provider` over `transport`: what every EVM client of a
    /// settlement table is built from.
    pub fn provider(transport: RpcTransport) -> Provider<EvmRpc> {
        Provider::new(EvmRpc::new(transport))
    }

    /// The transport this client rides.
    pub fn transport(&self) -> &RpcTransport {
        &self.transport
    }
}

/// Whether `error` is the node's own JSON-RPC answer (a revert, "nonce too
/// low", "insufficient funds"): something it said about the request, as
/// opposed to a failure to hear what it said.
pub fn answered(error: &ProviderError) -> bool {
    error.as_error_response().is_some()
}

/// Why a request to an EVM settlement endpoint did not produce a result.
#[derive(Debug, thiserror::Error)]
pub enum EvmRpcError {
    /// The request did not complete: a connect or request timeout, a reset
    /// connection, a proxy that refused the dial. Whether it reached the
    /// node is unknown.
    #[error("{endpoint} did not answer: {source}")]
    Transport {
        endpoint: String,
        #[source]
        source: reqwest::Error,
    },
    /// The endpoint kept refusing (403 or 429) after every retry.
    #[error("{endpoint} refused the request with HTTP {status}, {attempts} times in a row")]
    Refused {
        endpoint: String,
        status: u16,
        attempts: u32,
    },
    /// The node answered with a JSON-RPC error: a definitive answer about
    /// this request.
    #[error(transparent)]
    JsonRpc(JsonRpcError),
    /// The body was not a JSON-RPC response this client can read.
    #[error("{endpoint} answered HTTP {status} with a body that is not a JSON-RPC response: {reason}. Body: {body}")]
    Unreadable {
        endpoint: String,
        status: u16,
        reason: serde_json::Error,
        body: String,
    },
}

impl RpcError for EvmRpcError {
    fn as_error_response(&self) -> Option<&JsonRpcError> {
        match self {
            EvmRpcError::JsonRpc(error) => Some(error),
            _ => None,
        }
    }

    fn as_serde_error(&self) -> Option<&serde_json::Error> {
        match self {
            EvmRpcError::Unreadable { reason, .. } => Some(reason),
            _ => None,
        }
    }
}

impl From<EvmRpcError> for ProviderError {
    fn from(error: EvmRpcError) -> ProviderError {
        ProviderError::JsonRpcClientError(Box::new(error))
    }
}

#[derive(Serialize)]
struct Request<'a> {
    jsonrpc: &'static str,
    id: u64,
    method: &'a str,
    #[serde(skip_serializing_if = "serde_json::Value::is_null")]
    params: serde_json::Value,
}

#[async_trait]
impl JsonRpcClient for EvmRpc {
    type Error = EvmRpcError;

    async fn request<T, R>(&self, method: &str, params: T) -> Result<R, EvmRpcError>
    where
        T: std::fmt::Debug + Serialize + Send + Sync,
        R: DeserializeOwned + Send,
    {
        let endpoint = || self.transport.endpoint();
        // `()` serializes to `null`, which some nodes refuse as `params`;
        // ethers omits it, and so does this.
        let params = serde_json::to_value(&params).map_err(|reason| EvmRpcError::Unreadable {
            endpoint: endpoint(),
            status: 0,
            reason,
            body: String::new(),
        })?;
        let body = Request {
            jsonrpc: "2.0",
            id: self.next_id.fetch_add(1, Ordering::Relaxed),
            method,
            params,
        };

        let mut retry = 0;
        let (status, bytes) = loop {
            let response = self
                .transport
                .http()
                .post(self.transport.url().clone())
                .json(&body)
                .send()
                .await
                .map_err(|source| EvmRpcError::Transport {
                    endpoint: endpoint(),
                    source,
                })?;
            let status = response.status();
            if is_refusal(status) {
                match refusal_backoff(retry, response.headers()) {
                    Some(wait) => {
                        retry += 1;
                        tokio::time::sleep(wait).await;
                        continue;
                    }
                    None => {
                        return Err(EvmRpcError::Refused {
                            endpoint: endpoint(),
                            status: status.as_u16(),
                            attempts: retry + 1,
                        })
                    }
                }
            }
            let bytes = response
                .bytes()
                .await
                .map_err(|source| EvmRpcError::Transport {
                    endpoint: endpoint(),
                    source,
                })?;
            break (status, bytes);
        };

        let unreadable = |reason: serde_json::Error| EvmRpcError::Unreadable {
            endpoint: endpoint(),
            status: status.as_u16(),
            reason,
            body: String::from_utf8_lossy(&bytes).chars().take(300).collect(),
        };
        // Parsed whatever the status: several nodes answer a JSON-RPC error
        // with a 4xx or 5xx, and the error is still their answer.
        let mut response: serde_json::Value = serde_json::from_slice(&bytes).map_err(unreadable)?;
        if let Some(error) = response.get_mut("error").filter(|error| !error.is_null()) {
            let error: JsonRpcError = serde_json::from_value(error.take()).map_err(unreadable)?;
            return Err(EvmRpcError::JsonRpc(error));
        }
        // `"result": null` is an answer (no receipt yet, no such block); a
        // body with no `result` at all is not one.
        match response.get_mut("result") {
            Some(result) => serde_json::from_value(result.take()).map_err(unreadable),
            None => Err(unreadable(serde_json::Error::custom(
                "neither a result nor an error",
            ))),
        }
    }
}
