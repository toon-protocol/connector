//! The Solana SDK's `RpcClient`, built over a `[settlement.solana]` table's
//! [`RpcTransport`] (ADR 0073).
//!
//! The SDK's `HttpSender` already takes a caller's `reqwest::Client`, so the
//! bounds and the circuit ride in on the transport's client unchanged. What
//! it lacks is half of the refusal rule: it retries a 429 (five times, 500ms
//! apart) and returns a 403 as a final error. [`RefusalRetrying`] wraps it
//! and retries both with the crate's backoff, so a 403 from an exit relay's
//! shared address is never read as an answer. The SDK's own 429 retries
//! still run first, inside each of these attempts; that only makes a
//! persistent 429 cost more time before it is reported, never less.

use async_trait::async_trait;
use solana_rpc_client::http_sender::HttpSender;
use solana_rpc_client::nonblocking::rpc_client::RpcClient;
use solana_rpc_client::rpc_client::RpcClientConfig;
use solana_rpc_client::rpc_sender::{RpcSender, RpcTransportStats};
use solana_rpc_client_api::client_error::{
    Error as ClientError, ErrorKind, Result as ClientResult,
};
use solana_rpc_client_api::request::{RpcError, RpcRequest};

use crate::{is_refusal, refusal_backoff, RpcTransport};

/// An `RpcClient` over `transport`, configured by `config` (in practice
/// `RpcClientConfig::with_commitment(CommitmentConfig::confirmed())`).
pub fn rpc_client(transport: &RpcTransport, config: RpcClientConfig) -> RpcClient {
    RpcClient::new_sender(RefusalRetrying::new(transport), config)
}

/// Whether `error` is the node's own JSON-RPC answer (a failed preflight, an
/// unhealthy node), as opposed to a failure to hear what it said. A
/// `sendTransaction` the node answered with an error was not broadcast; one
/// that timed out may have been.
pub fn answered(error: &ClientError) -> bool {
    matches!(
        error.kind(),
        ErrorKind::RpcError(RpcError::RpcResponseError { .. })
    )
}

/// The SDK's `HttpSender` over the transport's client, retrying a refusal
/// (403 or 429) with backoff.
pub struct RefusalRetrying {
    inner: HttpSender,
}

impl RefusalRetrying {
    pub fn new(transport: &RpcTransport) -> RefusalRetrying {
        RefusalRetrying {
            inner: HttpSender::new_with_client(transport.url().as_str(), transport.http().clone()),
        }
    }
}

#[async_trait]
impl RpcSender for RefusalRetrying {
    async fn send(
        &self,
        request: RpcRequest,
        params: serde_json::Value,
    ) -> ClientResult<serde_json::Value> {
        let mut retry = 0;
        loop {
            let error = match self.inner.send(request, params.clone()).await {
                Ok(value) => return Ok(value),
                Err(error) => error,
            };
            let refused = match error.kind() {
                ErrorKind::Reqwest(source) => source.status().filter(|status| is_refusal(*status)),
                _ => None,
            };
            // `HttpSender` drops the response, headers and all, before it
            // returns a status error, so `Retry-After` is not visible here
            // and the computed backoff is used.
            match refused.and_then(|_| refusal_backoff(retry, &Default::default())) {
                Some(wait) => {
                    retry += 1;
                    tokio::time::sleep(wait).await;
                }
                None => return Err(error),
            }
        }
    }

    fn get_transport_stats(&self) -> RpcTransportStats {
        self.inner.get_transport_stats()
    }

    fn url(&self) -> String {
        self.inner.url()
    }
}
