//! The GraphQL client both of a zk edge's Mina endpoints are read through
//! (ADR 0080 decision 15, ADR 0073).
//!
//! Like [`crate::xrpl`], it is built only from an [`RpcTransport`]: the
//! bounds, the refusal rule and, when the table opts in, the pinned
//! circuit all come from the transport. The daemon's `graphql_url` and the
//! archive's `archive_url` are two transports, one client each, and no other
//! client of either may be built.
//!
//! A response is `{"data": …, "errors": […]}`. An `errors` array is the
//! endpoint's definitive answer about this request ([`GraphqlError::Query`]);
//! a body that is not that, or has no `data`, is [`GraphqlError::Unreadable`].
//! Nothing here retries a failure: the caller decides whether a read has
//! anything riding on it yet ([`crate::retry_read`]).

use serde_json::{json, Value};

use crate::{is_refusal, refusal_backoff, AsRefused, Refused, RpcTransport};

/// A GraphQL client over one endpoint's [`RpcTransport`].
#[derive(Debug, Clone)]
pub struct GraphqlRpc {
    transport: RpcTransport,
}

/// Why a GraphQL request did not produce data.
#[derive(Debug, thiserror::Error)]
pub enum GraphqlError {
    /// The request did not complete: a connect or request timeout, a reset
    /// connection, a proxy that refused the dial.
    #[error("{endpoint} did not answer: {source}")]
    Transport {
        endpoint: String,
        #[source]
        source: reqwest::Error,
    },
    /// The circuit's proxy could not be used, so nothing was sent.
    #[error("{endpoint} was not dialled: {reason}")]
    ProxyUnusable { endpoint: String, reason: String },
    /// The endpoint kept refusing (403 or 429) after every retry.
    #[error("{endpoint} refused the request with HTTP {status}, {attempts} times in a row")]
    Refused {
        endpoint: String,
        status: u16,
        attempts: u32,
    },
    /// The endpoint answered with GraphQL `errors`.
    #[error("{operation} was answered with errors: {messages}")]
    Query { operation: String, messages: String },
    /// The body was not a GraphQL response with data.
    #[error("{endpoint} answered HTTP {status} with a body this client cannot read: {reason}")]
    Unreadable {
        endpoint: String,
        status: u16,
        reason: String,
    },
}

impl AsRefused for GraphqlError {
    fn refusal(&self) -> Option<Refused> {
        match self {
            GraphqlError::Refused {
                endpoint, status, ..
            } => Some(Refused {
                endpoint: endpoint.clone(),
                status: *status,
            }),
            _ => None,
        }
    }
}

impl GraphqlRpc {
    pub fn new(transport: RpcTransport) -> GraphqlRpc {
        GraphqlRpc { transport }
    }

    /// The transport this client rides.
    pub fn transport(&self) -> &RpcTransport {
        &self.transport
    }

    /// One named operation with variables; the answer is the response's
    /// `data` object. Every request is named, so a recorded stand-in can
    /// answer it by name.
    pub async fn query(
        &self,
        operation: &str,
        query: &str,
        variables: Value,
    ) -> Result<Value, GraphqlError> {
        let endpoint = || self.transport.endpoint();
        // Serialised once, so a refused request is resent byte for byte.
        let body = json!({
            "operationName": operation,
            "query": query,
            "variables": variables,
        })
        .to_string();

        let mut retry = 0;
        let (status, bytes) = loop {
            let http = self
                .transport
                .http()
                .map_err(|reason| GraphqlError::ProxyUnusable {
                    endpoint: endpoint(),
                    reason,
                })?;
            let response = http
                .post(self.transport.url().clone())
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body.clone())
                .send()
                .await
                .map_err(|source| {
                    self.transport.observe(&source);
                    GraphqlError::Transport {
                        endpoint: endpoint(),
                        // reqwest prints the full URL, and a keyed endpoint's
                        // path is its API key; `endpoint` already names where.
                        source: source.without_url(),
                    }
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
                        return Err(GraphqlError::Refused {
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
                .map_err(|source| GraphqlError::Transport {
                    endpoint: endpoint(),
                    source: source.without_url(),
                })?;
            break (status, bytes);
        };

        let unreadable = |reason: String| GraphqlError::Unreadable {
            endpoint: endpoint(),
            status: status.as_u16(),
            reason,
        };
        let mut response: Value =
            serde_json::from_slice(&bytes).map_err(|e| unreadable(e.to_string()))?;
        if let Some(errors) = response
            .get("errors")
            .and_then(Value::as_array)
            .filter(|errors| !errors.is_empty())
        {
            let messages: Vec<&str> = errors
                .iter()
                .map(|error| error["message"].as_str().unwrap_or("(no message)"))
                .collect();
            return Err(GraphqlError::Query {
                operation: operation.to_string(),
                messages: messages.join("; "),
            });
        }
        if !status.is_success() {
            return Err(unreadable("not a success status".to_string()));
        }
        response
            .get_mut("data")
            .map(Value::take)
            .filter(|data| !data.is_null())
            .ok_or_else(|| unreadable("a response without data".to_string()))
    }
}
