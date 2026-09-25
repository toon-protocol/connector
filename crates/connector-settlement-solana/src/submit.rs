//! Sending one signed transaction and learning what became of it (ADR 0073
//! decision 5).
//!
//! This replaces the SDK's `send_and_confirm_transaction`, which loops on
//! `getSignatureStatuses` and `isBlockhashValid` with `?` on every poll. One
//! poll that timed out, got a 403 or lost its circuit therefore made the
//! whole call return an error **while the transaction could still land**,
//! and the operator was told "failed" about a transaction that went on to
//! succeed. For `fund`, which deposits an increment, retrying on that report
//! deposited twice.
//!
//! [`send_and_confirm`] never reports a bare error for a transaction that may
//! have landed. It keeps polling through transient errors until one of these
//! is known, and says which by signature:
//!
//! - **confirmed**: the transaction landed and succeeded;
//! - [`SubmitError::Failed`]: it landed and failed on chain;
//! - [`SubmitError::Refused`]: the node answered the send with an error
//!   (a failed preflight), so it was never broadcast;
//! - [`SubmitError::Expired`]: the chain passed the `lastValidBlockHeight`
//!   of the transaction's blockhash without it landing, so it **cannot**
//!   land any more and a retry is safe;
//! - [`SubmitError::Unknown`]: the endpoint could not be read for the whole
//!   of [`ConfirmPolicy::give_up_after`]. This is the one honest "I do not
//!   know", and it names the signature so the answer can be looked up.
//!
//! Blockhash expiry is what makes the loop finite in the normal case: a
//! Solana transaction carries its own deadline, and once the chain's block
//! height is past it the question "might it still land?" has a definite
//! answer. `give_up_after` bounds only the case where the chain cannot be
//! read at all.

use std::time::{Duration, Instant};

use connector_chain_rpc::solana::answered;
use connector_chain_rpc::RpcTransport;
use solana_rpc_client::nonblocking::rpc_client::RpcClient;
use solana_rpc_client::rpc_client::SerializableTransaction;
use solana_rpc_client_api::config::RpcSendTransactionConfig;
use solana_sdk::commitment_config::{CommitmentConfig, CommitmentLevel};
use solana_sdk::signature::Signature;
use solana_sdk::transaction::TransactionError;
use solana_transaction_status_client_types::UiTransactionEncoding;

use connector_settlement::SettlementError;

/// How [`send_and_confirm`] paces itself.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ConfirmPolicy {
    /// The wait between polls. The SDK's own loop waits 500ms. Over a
    /// circuit every poll is a round trip of about 0.3s already, so a
    /// proxied transport polls every second instead (ADR 0073).
    pub(crate) poll: Duration,
    /// How long the endpoint may stay unreadable before the outcome is
    /// reported as unknown. Two minutes is about twice a blockhash's whole
    /// lifetime (~150 blocks, 60-90s), so a readable chain always reaches
    /// a definite answer long before this.
    pub(crate) give_up_after: Duration,
}

impl ConfirmPolicy {
    pub(crate) fn for_transport(transport: &RpcTransport) -> ConfirmPolicy {
        ConfirmPolicy {
            poll: if transport.is_proxied() {
                Duration::from_secs(1)
            } else {
                Duration::from_millis(500)
            },
            give_up_after: Duration::from_secs(120),
        }
    }
}

/// What became of a transaction that did not confirm.
#[derive(Debug, thiserror::Error)]
pub(crate) enum SubmitError {
    #[error(
        "transaction {signature} was refused by the RPC node before it was broadcast: {reason}. \
         Nothing reached the chain, so retrying is safe"
    )]
    Refused {
        signature: Signature,
        reason: String,
    },
    #[error("transaction {signature} landed and failed on chain: {error}")]
    Failed {
        signature: Signature,
        error: TransactionError,
    },
    #[error(
        "transaction {signature} did not land before its blockhash expired (block height \
         {block_height} passed its lastValidBlockHeight {last_valid_block_height}); it can no \
         longer land, so retrying is safe"
    )]
    Expired {
        signature: Signature,
        block_height: u64,
        last_valid_block_height: u64,
    },
    #[error(
        "transaction {signature}: outcome unknown. The RPC endpoint could not be read for \
         {waited_secs}s (last error: {last_error}). It may still have landed: look the signature \
         up before retrying, since a retry of a transaction that landed runs it twice"
    )]
    Unknown {
        signature: Signature,
        waited_secs: u64,
        last_error: String,
    },
}

impl From<SubmitError> for SettlementError {
    fn from(error: SubmitError) -> SettlementError {
        SettlementError::Backend(error.to_string())
    }
}

/// Send `transaction` (already signed over a blockhash whose
/// `lastValidBlockHeight` is `last_valid_block_height`) and wait until its
/// outcome is known. See the module doc for the outcomes.
///
/// Legacy and versioned transactions alike: the sponsor endpoint (issue
/// #1346) submits a client-built version-0 `open` exactly as it was signed.
pub(crate) async fn send_and_confirm(
    rpc: &RpcClient,
    transaction: &impl SerializableTransaction,
    last_valid_block_height: u64,
    policy: ConfirmPolicy,
) -> Result<Signature, SubmitError> {
    // The fee payer's signature: every caller has signed as fee payer
    // before it gets here.
    let signature = *transaction.get_signature();
    // `encoding` is set so the SDK does not first ask the node its version
    // to choose one: one fewer round trip, and one fewer to fail.
    let first_send = RpcSendTransactionConfig {
        skip_preflight: false,
        preflight_commitment: Some(CommitmentLevel::Confirmed),
        encoding: Some(UiTransactionEncoding::Base64),
        ..RpcSendTransactionConfig::default()
    };
    let mut acknowledged = match rpc
        .send_transaction_with_config(transaction, first_send)
        .await
    {
        Ok(_) => true,
        Err(error) if answered(&error) => {
            return Err(SubmitError::Refused {
                signature,
                reason: error.to_string(),
            })
        }
        // The answer was lost, not given: the node may have the
        // transaction. Poll for it exactly as if it had said yes, and
        // re-send the same signed bytes until it has; re-sending one
        // signature cannot run it twice.
        Err(_) => false,
    };

    // The last time any read answered. `Unknown` is measured from here, so
    // it means what it says: nothing could be read for that long.
    let mut last_answer = Instant::now();
    let mut failures = 0u32;
    let mut last_error = String::from("none");
    loop {
        let mut failed = false;
        match poll_status(rpc, &signature, false).await {
            Ok(PollStatus::Confirmed) => return Ok(signature),
            Ok(PollStatus::Failed(error)) => return Err(SubmitError::Failed { signature, error }),
            Ok(PollStatus::Unseen) => last_answer = Instant::now(),
            Err(error) => {
                failed = true;
                last_error = error;
            }
        }

        match rpc
            .get_block_height_with_commitment(CommitmentConfig::confirmed())
            .await
        {
            Ok(block_height) if block_height > last_valid_block_height + EXPIRY_MARGIN => {
                last_answer = Instant::now();
                // Past its deadline, with a margin for the status and the
                // height coming from different backends of a load-balanced
                // endpoint. One last look, searching the ledger's history
                // rather than a backend's recent-status cache, since it may
                // have landed in the very block the deadline names; if that
                // look fails, the loop comes round again rather than
                // guessing.
                match poll_status(rpc, &signature, true).await {
                    Ok(PollStatus::Confirmed) => return Ok(signature),
                    Ok(PollStatus::Failed(error)) => {
                        return Err(SubmitError::Failed { signature, error })
                    }
                    Ok(PollStatus::Unseen) => {
                        return Err(SubmitError::Expired {
                            signature,
                            block_height,
                            last_valid_block_height,
                        })
                    }
                    Err(error) => {
                        failed = true;
                        last_error = error;
                    }
                }
            }
            Ok(_) => last_answer = Instant::now(),
            Err(error) => {
                failed = true;
                last_error = error.to_string();
            }
        }
        failures = if failed { failures + 1 } else { 0 };

        if last_answer.elapsed() >= policy.give_up_after {
            return Err(SubmitError::Unknown {
                signature,
                waited_secs: last_answer.elapsed().as_secs(),
                last_error,
            });
        }

        if !acknowledged {
            let resend = RpcSendTransactionConfig {
                // Preflight would refuse a transaction that already landed
                // ("already processed"), which is not a reason to stop.
                skip_preflight: true,
                encoding: Some(UiTransactionEncoding::Base64),
                ..RpcSendTransactionConfig::default()
            };
            if rpc
                .send_transaction_with_config(transaction, resend)
                .await
                .is_ok()
            {
                acknowledged = true;
            }
        }
        // Backs off while polls fail, so an endpoint that is refusing or
        // down is not polled at full speed; the first answer resets it.
        let backoff = policy
            .poll
            .saturating_mul(2u32.saturating_pow(failures.min(4)))
            .min(Duration::from_secs(8).max(policy.poll));
        tokio::time::sleep(backoff).await;
    }
}

/// Blocks past `lastValidBlockHeight` before a transaction is judged
/// expired. The height and the status can come from different backends of
/// a load-balanced endpoint, and one lagging behind the other must not turn
/// a landed transaction into "expired, retry is safe". About 8s of blocks.
const EXPIRY_MARGIN: u64 = 20;

/// What one status poll said.
enum PollStatus {
    /// Landed and succeeded, at `confirmed` or deeper.
    Confirmed,
    /// Landed and failed on chain.
    Failed(TransactionError),
    /// Not seen, or seen but not yet confirmed.
    Unseen,
}

/// `Some(Ok)` confirmed, `Some(Err)` landed and failed, `None` not seen (or
/// seen but not yet confirmed), `Err` the poll itself failed.
/// One status read. `search_history` asks the node to look beyond its
/// recent-status cache, which is what the final expiry check needs. `Err`
/// is the poll itself failing, never an answer about the transaction.
async fn poll_status(
    rpc: &RpcClient,
    signature: &Signature,
    search_history: bool,
) -> Result<PollStatus, String> {
    let statuses = if search_history {
        rpc.get_signature_statuses_with_history(&[*signature]).await
    } else {
        rpc.get_signature_statuses(&[*signature]).await
    }
    .map_err(|error| error.to_string())?;
    let Some(Some(status)) = statuses.value.into_iter().next() else {
        return Ok(PollStatus::Unseen);
    };
    if let Some(error) = status.err.clone() {
        return Ok(PollStatus::Failed(error));
    }
    if status.satisfies_commitment(CommitmentConfig::confirmed()) {
        return Ok(PollStatus::Confirmed);
    }
    Ok(PollStatus::Unseen)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    use connector_chain_rpc::solana::rpc_client;
    use connector_chain_rpc::{FakeRpc, Route, RpcCall, RpcReply, Timeouts};
    use serde_json::json;
    use solana_rpc_client::rpc_client::RpcClientConfig;
    use solana_sdk::hash::Hash;
    use solana_sdk::signature::{Keypair, Signer};
    use solana_sdk::system_instruction;
    use solana_sdk::transaction::Transaction;

    use super::*;

    const LAST_VALID: u64 = 1_000;

    const FAST: ConfirmPolicy = ConfirmPolicy {
        poll: Duration::from_millis(10),
        give_up_after: Duration::from_secs(2),
    };

    fn signed_transaction() -> Transaction {
        let payer = Keypair::new();
        Transaction::new_signed_with_payer(
            &[system_instruction::transfer(
                &payer.pubkey(),
                &Keypair::new().pubkey(),
                1,
            )],
            Some(&payer.pubkey()),
            &[&payer],
            Hash::new_unique(),
        )
    }

    fn status(err: Option<serde_json::Value>, level: &str) -> RpcReply {
        RpcReply::Result(json!({
            "context": {"slot": 1},
            "value": [{
                "slot": 1,
                "confirmations": null,
                "err": err,
                "status": if err.is_some() { json!({"Err": err}) } else { json!({"Ok": null}) },
                "confirmationStatus": level,
            }],
        }))
    }

    fn not_seen() -> RpcReply {
        RpcReply::Result(json!({"context": {"slot": 1}, "value": [null]}))
    }

    /// A Solana node that answers `sendTransaction` with the signature and
    /// block height as `height(call)`, and whose status answers the script
    /// decides.
    async fn node(
        signature: Signature,
        send: impl Fn(&RpcCall) -> RpcReply + Send + Sync + 'static,
        statuses: impl Fn(&RpcCall) -> RpcReply + Send + Sync + 'static,
        height: impl Fn(&RpcCall) -> RpcReply + Send + Sync + 'static,
    ) -> FakeRpc {
        let signature = signature.to_string();
        FakeRpc::spawn(move |call| match call.method.as_str() {
            "sendTransaction" => match send(call) {
                RpcReply::Result(_) => RpcReply::Result(json!(signature)),
                other => other,
            },
            "getSignatureStatuses" => statuses(call),
            "getBlockHeight" => height(call),
            other => panic!("the confirm loop does not call {other}"),
        })
        .await
    }

    fn client(rpc: &FakeRpc) -> RpcClient {
        let transport = RpcTransport::new(
            &rpc.url(),
            Route::Direct,
            Timeouts {
                connect: Duration::from_millis(300),
                request: Duration::from_millis(300),
                pool_idle: Duration::from_millis(300),
            },
        )
        .expect("transport");
        rpc_client(
            &transport,
            RpcClientConfig::with_commitment(CommitmentConfig::confirmed()),
        )
    }

    fn accepted(_: &RpcCall) -> RpcReply {
        RpcReply::Result(json!(null))
    }

    fn below_deadline(_: &RpcCall) -> RpcReply {
        RpcReply::Result(json!(LAST_VALID - 10))
    }

    /// The defect this module exists for: polls that time out, get a 403 or
    /// lose their connection are not the transaction's outcome.
    #[tokio::test]
    async fn polls_that_fail_do_not_end_the_wait_for_a_transaction_that_lands() {
        let transaction = signed_transaction();
        let rpc = node(
            transaction.signatures[0],
            accepted,
            |call| match call.nth {
                0 => RpcReply::Drop,
                1 => RpcReply::Hang,
                2 => RpcReply::Error {
                    code: -32005,
                    message: "node is behind".to_string(),
                },
                3 => not_seen(),
                _ => status(None, "confirmed"),
            },
            |call| {
                if call.nth == 0 {
                    RpcReply::Drop
                } else {
                    below_deadline(call)
                }
            },
        )
        .await;

        let signature = send_and_confirm(&client(&rpc), &transaction, LAST_VALID, FAST)
            .await
            .expect("the transaction landed, and failed polls must not say otherwise");
        assert_eq!(signature, transaction.signatures[0]);
    }

    #[tokio::test]
    async fn a_refused_poll_is_waited_out_rather_than_read_as_an_answer() {
        let transaction = signed_transaction();
        let rpc = node(
            transaction.signatures[0],
            accepted,
            |call| {
                if call.nth < 2 {
                    RpcReply::Status(403)
                } else {
                    status(None, "finalized")
                }
            },
            below_deadline,
        )
        .await;

        send_and_confirm(&client(&rpc), &transaction, LAST_VALID, FAST)
            .await
            .expect("a 403 is an exit's shared address being refused, not an outcome");
    }

    #[tokio::test]
    async fn a_transaction_that_never_lands_is_reported_expired_once_its_blockhash_is() {
        let transaction = signed_transaction();
        let rpc = node(
            transaction.signatures[0],
            accepted,
            |_| not_seen(),
            |call| RpcReply::Result(json!(LAST_VALID - 2 + 10 * call.nth as u64)),
        )
        .await;

        let error = send_and_confirm(&client(&rpc), &transaction, LAST_VALID, FAST)
            .await
            .expect_err("it never landed");
        let text = error.to_string();
        let SubmitError::Expired { block_height, .. } = error else {
            panic!("expected Expired, got {error:?}");
        };
        assert!(block_height > LAST_VALID + EXPIRY_MARGIN);
        assert!(text.contains("retrying is safe"), "{text}");
    }

    /// The #907 pattern on Solana: the height comes from a backend that is
    /// ahead, and the recent-status cache from one that has not seen the
    /// transaction. Only the history search, past the margin, decides.
    #[tokio::test]
    async fn a_lagging_status_cache_does_not_turn_a_landed_transaction_into_expired() {
        let transaction = signed_transaction();
        let rpc = node(
            transaction.signatures[0],
            accepted,
            |call| {
                let searched = call.params[1]["searchTransactionHistory"] == json!(true);
                if searched {
                    status(None, "finalized")
                } else {
                    not_seen()
                }
            },
            |_| RpcReply::Result(json!(LAST_VALID + 1_000)),
        )
        .await;

        send_and_confirm(&client(&rpc), &transaction, LAST_VALID, FAST)
            .await
            .expect("it landed; the history search finds it");
    }

    #[tokio::test]
    async fn a_transaction_that_lands_and_fails_is_reported_failed_by_signature() {
        let transaction = signed_transaction();
        let rpc = node(
            transaction.signatures[0],
            accepted,
            |_| {
                status(
                    Some(json!({"InstructionError": [0, {"Custom": 6}]})),
                    "confirmed",
                )
            },
            below_deadline,
        )
        .await;

        let error = send_and_confirm(&client(&rpc), &transaction, LAST_VALID, FAST)
            .await
            .expect_err("it failed on chain");
        assert!(matches!(error, SubmitError::Failed { .. }), "{error:?}");
        assert!(error
            .to_string()
            .contains(&transaction.signatures[0].to_string()));
    }

    #[tokio::test]
    async fn a_send_the_node_answered_with_an_error_was_never_broadcast() {
        let transaction = signed_transaction();
        let rpc = node(
            transaction.signatures[0],
            |_| RpcReply::Error {
                code: -32002,
                message: "Transaction simulation failed".to_string(),
            },
            |_| panic!("a refused send has nothing to poll for"),
            below_deadline,
        )
        .await;

        let error = send_and_confirm(&client(&rpc), &transaction, LAST_VALID, FAST)
            .await
            .expect_err("preflight refused it");
        assert!(matches!(error, SubmitError::Refused { .. }), "{error:?}");
    }

    /// The ambiguous send: the node took the transaction and the answer was
    /// lost on the way back. The same signed bytes are re-sent until the
    /// node acknowledges them, and the outcome is still read by signature.
    #[tokio::test]
    async fn a_send_whose_answer_was_lost_is_resent_and_confirmed_not_reported_failed() {
        let transaction = signed_transaction();
        let resent = Arc::new(AtomicU64::new(0));
        let counter = Arc::clone(&resent);
        let rpc = node(
            transaction.signatures[0],
            move |call| {
                if call.nth == 0 {
                    RpcReply::Drop
                } else {
                    counter.fetch_add(1, Ordering::SeqCst);
                    accepted(call)
                }
            },
            |call| {
                if call.nth < 3 {
                    not_seen()
                } else {
                    status(None, "confirmed")
                }
            },
            below_deadline,
        )
        .await;

        send_and_confirm(&client(&rpc), &transaction, LAST_VALID, FAST)
            .await
            .expect("a lost answer is not a failed send");
        assert!(resent.load(Ordering::SeqCst) >= 1);
    }

    #[tokio::test]
    async fn an_endpoint_that_cannot_be_read_at_all_yields_unknown_naming_the_signature() {
        let transaction = signed_transaction();
        let rpc = node(
            transaction.signatures[0],
            accepted,
            |_| RpcReply::Drop,
            |_| RpcReply::Drop,
        )
        .await;

        let policy = ConfirmPolicy {
            poll: Duration::from_millis(10),
            give_up_after: Duration::from_millis(300),
        };
        let error = send_and_confirm(&client(&rpc), &transaction, LAST_VALID, policy)
            .await
            .expect_err("nothing could be read");
        assert!(matches!(error, SubmitError::Unknown { .. }), "{error:?}");
        let text = error.to_string();
        assert!(text.contains(&transaction.signatures[0].to_string()));
        assert!(text.contains("may still have landed"), "{text}");
    }
}
