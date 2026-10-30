//! The XRPL JSON-RPC client every `[settlement.xrpl.<network_id>]` client
//! rides (ADR 0073).
//!
//! It is built only from a table's one [`RpcTransport`], like [`crate::evm`]
//! and [`crate::solana`]: the bounds, the refusal rule and, when the table
//! opts in, the pinned circuit all come from the transport. No other XRPL
//! client may be built, `xrpl-rust`'s own included.
//!
//! The wire is rippled's JSON-RPC, not JSON-RPC 2.0: a request is
//! `{"method": …, "params": [{…}]}` and the answer is
//! `{"result": {"status": "success" | "error", …}}`, an error answered with
//! HTTP 200. [`XrplRpcError::Rpc`] is that answer: the node said something
//! definitive about the request. Everything else says nothing about whether
//! the request took effect.
//!
//! **Every wait is bounded by `LastLedgerSequence`** ([`XrplRpc::wait_for_tx`]).
//! A transaction carries the last ledger it may land in, and the ledger
//! itself says when that has passed, so the wait ends in [`TxFate::Landed`]
//! or [`TxFate::KnownNotLanded`] and never in a hang. A failed poll does not
//! end it; an endpoint that answers nothing for [`TxWait::give_up_after`]
//! does. This is the answer to "did my send land?" that EVM gets from a
//! receipt and a nonce.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::{is_refusal, refusal_backoff, AsRefused, Refused, RpcTransport};

/// A JSON-RPC client over one settlement table's [`RpcTransport`].
#[derive(Debug)]
pub struct XrplRpc {
    transport: RpcTransport,
}

/// Why a request to an XRPL settlement endpoint did not produce a result.
#[derive(Debug, thiserror::Error)]
pub enum XrplRpcError {
    /// The request did not complete: a connect or request timeout, a reset
    /// connection, a proxy that refused the dial. Whether it reached the
    /// node is unknown.
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
    /// The node answered `"status": "error"`: a definitive answer about this
    /// request, such as `actNotFound` or `entryNotFound`.
    #[error("{method} was answered with the error '{error}': {message}")]
    Rpc {
        method: String,
        error: String,
        message: String,
    },
    /// The body was not an XRPL JSON-RPC response this client can read.
    #[error("{endpoint} answered HTTP {status} with a body this client cannot read: {reason}. Body: {body}")]
    Unreadable {
        endpoint: String,
        status: u16,
        reason: String,
        body: String,
    },
}

impl AsRefused for XrplRpcError {
    fn refusal(&self) -> Option<Refused> {
        match self {
            XrplRpcError::Refused {
                endpoint, status, ..
            } => Some(Refused {
                endpoint: endpoint.clone(),
                status: *status,
            }),
            _ => None,
        }
    }
}

impl XrplRpcError {
    /// The node's own error code, when this is the node's answer.
    pub fn node_error(&self) -> Option<&str> {
        match self {
            XrplRpcError::Rpc { error, .. } => Some(error),
            _ => None,
        }
    }
}

/// What `server_info` says about the node, as far as boot and the harness
/// read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerInfo {
    /// The network id the node was configured with. Absent from the answer
    /// on a node that has none (mainnet reports none).
    pub network_id: Option<u32>,
    /// `full`, `proposing`, `connected`, …
    pub server_state: String,
    /// The newest validated ledger, once there is one.
    pub validated_ledger: Option<u32>,
    /// The base reserve of the validated ledger, in drops: what an account
    /// must hold to exist. Absent until the node has a validated ledger.
    pub reserve_base_drops: Option<u64>,
}

/// An account's `AccountRoot`, as far as funding and sequencing read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountInfo {
    /// XRP, in drops.
    pub balance_drops: u64,
    /// The next sequence number the account will sign with.
    pub sequence: u32,
    /// Objects the account owns (each raises its reserve).
    pub owner_count: u32,
}

/// The fee the node would charge now, in drops.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fee {
    pub base_fee_drops: u64,
    pub open_ledger_fee_drops: u64,
    /// The ledger the open ledger will become: a transaction submitted now
    /// can land in it at the earliest.
    pub ledger_current_index: u32,
}

/// What `submit` answered about a transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmitOutcome {
    /// The transaction's hash, whether or not it was applied.
    pub hash: String,
    /// `tesSUCCESS`, `terPRE_SEQ`, `tefPAST_SEQ`, …
    pub engine_result: String,
    /// Whether the node applied it to its open ledger. Not finality.
    pub applied: bool,
    /// Whether it was kept to retry in a later ledger.
    pub queued_or_held: bool,
}

/// A transaction the node knows of, validated or not.
#[derive(Debug, Clone, PartialEq)]
pub struct TxStatus {
    pub validated: bool,
    /// The ledger it was included in, when it has been.
    pub ledger_index: Option<u32>,
    /// `meta.TransactionResult`, once it has been included.
    pub transaction_result: Option<String>,
    /// The transaction's metadata, which says what a `tesSUCCESS` actually
    /// delivered.
    pub meta: Value,
}

/// How [`XrplRpc::wait_for_tx`] paces itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TxWait {
    /// The wait between polls while the endpoint answers.
    pub poll: Duration,
    /// How long the endpoint may stay unreadable before the wait gives up
    /// and returns the last error.
    pub give_up_after: Duration,
}

/// How a bounded wait on a transaction ended.
#[derive(Debug, Clone, PartialEq)]
pub enum TxFate {
    /// In a validated ledger, with `transaction_result` its result code.
    /// A `tec` code is a landed failure: fee spent, effect none.
    Landed {
        ledger_index: u32,
        transaction_result: String,
        meta: Value,
    },
    /// A validated ledger past `LastLedgerSequence` does not hold it, and
    /// none ever can. Safe to rebuild and resend.
    KnownNotLanded { last_ledger_sequence: u32 },
}

impl XrplRpc {
    pub fn new(transport: RpcTransport) -> XrplRpc {
        XrplRpc { transport }
    }

    /// The transport this client rides.
    pub fn transport(&self) -> &RpcTransport {
        &self.transport
    }

    /// One call: `params` is the single params object, and the answer is the
    /// `result` object of a `"status": "success"` response.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value, XrplRpcError> {
        let endpoint = || self.transport.endpoint();
        // Serialised once, so a refused request is resent byte for byte.
        let body = json!({ "method": method, "params": [params] }).to_string();

        let mut retry = 0;
        let (status, bytes) = loop {
            let http = self
                .transport
                .http()
                .map_err(|reason| XrplRpcError::ProxyUnusable {
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
                    XrplRpcError::Transport {
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
                        return Err(XrplRpcError::Refused {
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
                .map_err(|source| XrplRpcError::Transport {
                    endpoint: endpoint(),
                    source: source.without_url(),
                })?;
            break (status, bytes);
        };

        let unreadable = |reason: String| XrplRpcError::Unreadable {
            endpoint: endpoint(),
            status: status.as_u16(),
            reason,
            body: String::from_utf8_lossy(&bytes).chars().take(300).collect(),
        };
        let mut response: Value =
            serde_json::from_slice(&bytes).map_err(|e| unreadable(e.to_string()))?;
        let result = response
            .get_mut("result")
            .map(Value::take)
            .filter(Value::is_object)
            .ok_or_else(|| unreadable("no `result` object".to_string()))?;
        match result.get("status").and_then(Value::as_str) {
            Some("success") => Ok(result),
            Some("error") => Err(XrplRpcError::Rpc {
                method: method.to_string(),
                error: str_field(&result, "error").unwrap_or_default(),
                message: str_field(&result, "error_message").unwrap_or_default(),
            }),
            other => Err(unreadable(format!("unknown status {other:?}"))),
        }
    }

    pub async fn server_info(&self) -> Result<ServerInfo, XrplRpcError> {
        let result = self.call("server_info", json!({})).await?;
        let info = &result["info"];
        Ok(ServerInfo {
            network_id: info["network_id"]
                .as_u64()
                .and_then(|n| u32::try_from(n).ok()),
            server_state: str_field(info, "server_state").unwrap_or_default(),
            validated_ledger: info["validated_ledger"]["seq"]
                .as_u64()
                .and_then(|n| u32::try_from(n).ok()),
            // `reserve_base_xrp` is XRP, whole or fractional: scaled to
            // drops and rounded, so a float's last digit cannot lose one.
            reserve_base_drops: info["validated_ledger"]["reserve_base_xrp"]
                .as_f64()
                .filter(|xrp| xrp.is_finite() && *xrp >= 0.0)
                .map(|xrp| (xrp * 1_000_000.0).round() as u64),
        })
    }

    /// `None` when the account does not exist on the ledger (`actNotFound`).
    /// Read from the **validated** ledger: a balance is not final before it.
    pub async fn account_info(&self, account: &str) -> Result<Option<AccountInfo>, XrplRpcError> {
        let result = match self
            .call(
                "account_info",
                json!({ "account": account, "ledger_index": "validated" }),
            )
            .await
        {
            Ok(result) => result,
            Err(e) if e.node_error() == Some("actNotFound") => return Ok(None),
            Err(e) => return Err(e),
        };
        let data = &result["account_data"];
        let unreadable = |what: &str| XrplRpcError::Unreadable {
            endpoint: self.transport.endpoint(),
            status: 200,
            reason: format!("account_data has no {what}"),
            body: String::new(),
        };
        Ok(Some(AccountInfo {
            balance_drops: str_field(data, "Balance")
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| unreadable("Balance"))?,
            sequence: data["Sequence"]
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| unreadable("Sequence"))?,
            owner_count: data["OwnerCount"]
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .unwrap_or(0),
        }))
    }

    /// The ledger object at `index` (a 64-hex ledger index, such as a
    /// channel id), from the validated ledger. `None` when there is none
    /// (`entryNotFound`): a deleted channel reads as absent.
    pub async fn ledger_entry(&self, index: &str) -> Result<Option<Value>, XrplRpcError> {
        match self
            .call(
                "ledger_entry",
                json!({ "index": index, "ledger_index": "validated" }),
            )
            .await
        {
            Ok(mut result) => Ok(Some(result["node"].take())),
            Err(e) if e.node_error() == Some("entryNotFound") => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Every payment channel `account` is the source of, optionally only
    /// those to `destination`, following the node's paging marker.
    pub async fn account_channels(
        &self,
        account: &str,
        destination: Option<&str>,
    ) -> Result<Vec<Value>, XrplRpcError> {
        let mut channels = Vec::new();
        let mut marker = Value::Null;
        loop {
            let mut params = json!({ "account": account, "ledger_index": "validated" });
            if let Some(destination) = destination {
                params["destination_account"] = json!(destination);
            }
            if !marker.is_null() {
                params["marker"] = marker.clone();
            }
            let mut result = self.call("account_channels", params).await?;
            if let Some(page) = result["channels"].as_array_mut() {
                channels.append(page);
            }
            marker = result["marker"].take();
            if marker.is_null() {
                return Ok(channels);
            }
        }
    }

    /// Submit a signed transaction (`tx_blob`, hex).
    ///
    /// **Never retried here.** A submit whose answer was lost is resolved by
    /// [`wait_for_tx`](Self::wait_for_tx) on the transaction's hash, which the
    /// caller knows before it submits; resubmitting the same blob is safe
    /// only because it is the same transaction, and that decision is the
    /// caller's.
    pub async fn submit(&self, tx_blob: &str) -> Result<SubmitOutcome, XrplRpcError> {
        let result = self.call("submit", json!({ "tx_blob": tx_blob })).await?;
        Ok(submit_outcome(&result))
    }

    /// `None` when the node has never heard of the hash (`txnNotFound`).
    pub async fn tx(&self, hash: &str) -> Result<Option<TxStatus>, XrplRpcError> {
        let result = match self
            .call("tx", json!({ "transaction": hash, "binary": false }))
            .await
        {
            Ok(result) => result,
            Err(e) if e.node_error() == Some("txnNotFound") => return Ok(None),
            Err(e) => return Err(e),
        };
        Ok(Some(TxStatus {
            validated: result["validated"].as_bool().unwrap_or(false),
            ledger_index: result["ledger_index"]
                .as_u64()
                .and_then(|n| u32::try_from(n).ok()),
            transaction_result: str_field(&result["meta"], "TransactionResult"),
            meta: result["meta"].clone(),
        }))
    }

    pub async fn fee(&self) -> Result<Fee, XrplRpcError> {
        let result = self.call("fee", json!({})).await?;
        let drops = |name: &str| str_field(&result["drops"], name).and_then(|s| s.parse().ok());
        let unreadable = |what: &str| XrplRpcError::Unreadable {
            endpoint: self.transport.endpoint(),
            status: 200,
            reason: format!("fee answer has no {what}"),
            body: String::new(),
        };
        Ok(Fee {
            base_fee_drops: drops("base_fee").ok_or_else(|| unreadable("drops.base_fee"))?,
            open_ledger_fee_drops: drops("open_ledger_fee")
                .ok_or_else(|| unreadable("drops.open_ledger_fee"))?,
            ledger_current_index: result["ledger_current_index"]
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| unreadable("ledger_current_index"))?,
        })
    }

    /// Wait for the fate of a submitted transaction, bounded by its
    /// `LastLedgerSequence`.
    ///
    /// Polls every `wait.poll`. The validated ledger's index is read
    /// **before** the transaction is looked up: if that index is already
    /// past `last_ledger_sequence` and the lookup then finds the transaction
    /// in no validated ledger, no ledger that could hold it is left, so the
    /// answer is [`TxFate::KnownNotLanded`]. Read the other way round, a
    /// transaction validated between the two reads would be called lost.
    ///
    /// A failed read does not end the wait (ADR 0073 decision 5): the
    /// transaction may still land, so the loop backs off and polls again.
    /// Only when nothing has answered for `wait.give_up_after` does it
    /// return the last error, the one honest "I do not know"; the caller
    /// holds the hash and can ask again.
    pub async fn wait_for_tx(
        &self,
        hash: &str,
        last_ledger_sequence: u32,
        wait: TxWait,
    ) -> Result<TxFate, XrplRpcError> {
        // The last time any read answered: `give_up_after` is measured from
        // here, so it means nothing could be read for that long.
        let mut last_answer = Instant::now();
        let mut failures = 0u32;
        loop {
            let polled = async {
                let validated_before = self.server_info().await?.validated_ledger;
                Ok::<_, XrplRpcError>((validated_before, self.tx(hash).await?))
            };
            match polled.await {
                Ok((validated_before, status)) => {
                    last_answer = Instant::now();
                    failures = 0;
                    if let Some(TxStatus {
                        validated: true,
                        ledger_index: Some(ledger_index),
                        transaction_result: Some(transaction_result),
                        meta,
                    }) = status
                    {
                        return Ok(TxFate::Landed {
                            ledger_index,
                            transaction_result,
                            meta,
                        });
                    }
                    if validated_before.is_some_and(|seq| seq > last_ledger_sequence) {
                        return Ok(TxFate::KnownNotLanded {
                            last_ledger_sequence,
                        });
                    }
                }
                Err(error) => {
                    if last_answer.elapsed() >= wait.give_up_after {
                        return Err(error);
                    }
                    failures += 1;
                }
            }
            // Backs off while polls fail, so an endpoint that is refusing or
            // down is not polled at full speed; the first answer resets it.
            let backoff = wait
                .poll
                .saturating_mul(2u32.saturating_pow(failures.min(4)))
                .min(Duration::from_secs(8).max(wait.poll));
            tokio::time::sleep(backoff).await;
        }
    }
}

/// Calls only a stand-alone `xrpld` in a test answers: they close ledgers by
/// hand, or take a secret. The shipped binary must carry none of them.
#[cfg(feature = "test-support")]
impl XrplRpc {
    /// Close the open ledger (`ledger_accept`, admin, stand-alone only) and
    /// return the index of the ledger now open.
    pub async fn ledger_accept(&self) -> Result<u32, XrplRpcError> {
        let result = self.call("ledger_accept", json!({})).await?;
        result["ledger_current_index"]
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| XrplRpcError::Unreadable {
                endpoint: self.transport.endpoint(),
                status: 200,
                reason: "ledger_accept answered no ledger_current_index".to_string(),
                body: String::new(),
            })
    }

    /// Have the node sign `tx_json` with `secret` and submit it. A stand-alone
    /// node's convenience: a production node is only ever handed a blob.
    pub async fn sign_and_submit(
        &self,
        tx_json: Value,
        secret: &str,
    ) -> Result<SubmitOutcome, XrplRpcError> {
        let result = self
            .call(
                "submit",
                json!({ "tx_json": tx_json, "secret": secret, "fee_mult_max": 1000 }),
            )
            .await?;
        Ok(submit_outcome(&result))
    }

    /// A fresh keypair from the node's `wallet_propose`: the classic address
    /// and the family seed.
    pub async fn wallet_propose(&self) -> Result<(String, String), XrplRpcError> {
        let result = self.call("wallet_propose", json!({})).await?;
        match (
            str_field(&result, "account_id"),
            str_field(&result, "master_seed"),
        ) {
            (Some(address), Some(seed)) => Ok((address, seed)),
            _ => Err(XrplRpcError::Unreadable {
                endpoint: self.transport.endpoint(),
                status: 200,
                reason: "wallet_propose answered no account_id or master_seed".to_string(),
                body: String::new(),
            }),
        }
    }
}

fn str_field(value: &Value, name: &str) -> Option<String> {
    value.get(name)?.as_str().map(str::to_string)
}

fn submit_outcome(result: &Value) -> SubmitOutcome {
    SubmitOutcome {
        hash: str_field(&result["tx_json"], "hash").unwrap_or_default(),
        engine_result: str_field(result, "engine_result").unwrap_or_default(),
        applied: result["applied"].as_bool().unwrap_or(false),
        queued_or_held: result["queued"].as_bool().unwrap_or(false)
            || result["kept"].as_bool().unwrap_or(false),
    }
}
