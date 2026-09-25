//! Signing, sending and confirming one EVM transaction, with nonces that
//! cannot rewind and a wait that ends (ADR 0073 decision 5).
//!
//! # What this replaces
//!
//! Every write used to go through ethers' `NonceManagerMiddleware` and then
//! wait on `PendingTransaction`. ADR 0073 found three defects there, all of
//! which a circuit turns from rare into routine:
//!
//! - **No deadline.** `PendingTransaction` polls until a receipt appears, and
//!   the HTTP client under it had no timeout, so a circuit that stopped
//!   answering held the caller (and `fund`'s `deposit_lock`) forever.
//! - **"Not observed" was declared in a few seconds.** Three empty
//!   `eth_getTransactionByHash` polls and four receipt reads, 1s apart, and
//!   the operator was told to check by hash. At circuit latency that is 5-10s
//!   against a transaction that usually lands.
//! - **The nonce manager rewinds.** It seeds from `latest`, not `pending`,
//!   and on **any** send error (including a timeout after the node had
//!   already accepted the transaction) it re-reads `latest` and re-sends at
//!   that nonce. With two writes in flight that can replace a different
//!   pending transaction.
//!
//! # What this does instead
//!
//! [`Sender::send`] holds one lock across choosing a nonce, signing and
//! sending, so concurrent writes still get distinct, ordered nonces. The
//! nonce is seeded from `eth_getTransactionCount(own, "pending")` and then
//! counted locally. What happens after `eth_sendRawTransaction` depends on
//! what the node said:
//!
//! - **It accepted the transaction:** the nonce advances.
//! - **It answered with an error** (a JSON-RPC error, so it definitely did
//!   not take the transaction). A nonce conflict re-seeds from `pending` and
//!   signs this same operation once more. That is not a resubmit, because
//!   the operation was never accepted. Any other error is returned and
//!   nothing was sent.
//! - **The answer was lost** (a timeout, a reset, a refusal that outlasted
//!   its retries). The transaction's hash is known before it is sent, so it
//!   is looked up **by hash**. If the node does not have it, the **same
//!   signed bytes** are broadcast again. Identical bytes carry the same hash
//!   and nonce, so they cannot run twice. Then the hash goes to
//!   [`confirm`] like any other. Nothing is ever signed again at a nonce
//!   that might already be used.
//!
//! [`confirm`] polls for the receipt by hash until it has one, bounded two
//! ways by [`ConfirmPolicy`]:
//!
//! - a transaction no endpoint poll has **observed** for 30s is reported as
//!   not observed. The report carries the hash and says it is not proof the
//!   transaction was dropped (issue #907's wording);
//! - one that was observed but has not mined within 180s is reported the
//!   same way.
//!
//! Both errors say plainly that a retry is not known to be safe.

use std::sync::Arc;
use std::time::{Duration, Instant};

use connector_chain_rpc::evm::{answered, EvmRpc};
use connector_chain_rpc::{retry_read, RpcTransport};
use connector_settlement::SettlementError;
use ethers::providers::{Middleware, Provider, ProviderError};
use ethers::signers::{LocalWallet, Signer as EvmSigner};
use ethers::types::transaction::eip2718::TypedTransaction;
use ethers::types::{Address, BlockNumber, TransactionReceipt, TxHash, U256, U64};
use ethers::utils::keccak256;

/// How [`confirm`] paces itself and when it stops.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ConfirmPolicy {
    /// The wait between receipt polls. 100ms direct, which is what the
    /// backend has always polled `anvil` at. Over a circuit each poll costs
    /// a round trip of about 0.3s anyway, and a public endpoint polled ten
    /// times a second through a shared exit earns a 429, so a proxied
    /// transport polls every second (ADR 0073).
    pub(crate) poll: Duration,
    /// How long a transaction no poll has seen may stay unseen before it is
    /// reported as not observed. At least 30s (ADR 0073 decision 5), up
    /// from about 4s.
    pub(crate) unobserved: Duration,
    /// How long anything may take to mine: 180s, 90 Base blocks.
    pub(crate) deadline: Duration,
}

impl ConfirmPolicy {
    pub(crate) fn for_transport(transport: &RpcTransport) -> ConfirmPolicy {
        ConfirmPolicy {
            poll: if transport.is_proxied() {
                Duration::from_secs(1)
            } else {
                Duration::from_millis(100)
            },
            unobserved: Duration::from_secs(30),
            deadline: Duration::from_secs(180),
        }
    }
}

/// The one place this backend signs and sends.
pub(crate) struct Sender {
    provider: Arc<Provider<EvmRpc>>,
    wallet: LocalWallet,
    /// Where the next write's nonce comes from, under the one lock every
    /// write takes.
    nonces: tokio::sync::Mutex<NonceState>,
}

/// The sender's nonce bookkeeping.
#[derive(Default)]
struct NonceState {
    /// The next nonce to use, or `None` when it must be read from `pending`
    /// first: at start, and after anything that leaves it uncertain.
    next: Option<U256>,
    /// A transaction whose send answer was lost and which no lookup found:
    /// its nonce and its exact signed bytes. It may be in some backend's
    /// pool, or nowhere. So the next write never signs a different
    /// operation at that nonce. If `pending` has not moved past it, the
    /// same bytes are offered again and the next write takes the nonce
    /// after it.
    unresolved: Option<(U256, ethers::types::Bytes)>,
}

impl Sender {
    /// `wallet` must already carry the chain id.
    pub(crate) fn new(provider: Arc<Provider<EvmRpc>>, wallet: LocalWallet) -> Sender {
        Sender {
            provider,
            wallet,
            nonces: tokio::sync::Mutex::new(NonceState::default()),
        }
    }

    pub(crate) fn address(&self) -> Address {
        self.wallet.address()
    }

    /// Fill, sign and send `transaction`, returning its hash once the node
    /// has it, or once the answer was lost and the hash is all there is to
    /// go on. See the module doc for each outcome.
    pub(crate) async fn send(
        &self,
        mut transaction: TypedTransaction,
    ) -> Result<TxHash, SettlementError> {
        let own = self.wallet.address();
        let mut nonces = self.nonces.lock().await;
        let mut reseeded = false;
        loop {
            let nonce = match nonces.next {
                Some(nonce) => nonce,
                None => self.seed_nonce(&mut nonces).await?,
            };
            transaction.set_from(own);
            transaction.set_nonce(nonce);
            transaction.set_chain_id(self.wallet.chain_id());
            // Gas and fees are estimated here. A failure sends nothing, so
            // it is returned as it is, reverts included.
            self.provider
                .fill_transaction(&mut transaction, None)
                .await
                .map_err(|error| SettlementError::Backend(error.to_string()))?;
            let signature = self
                .wallet
                .sign_transaction(&transaction)
                .await
                .map_err(|error| SettlementError::Backend(error.to_string()))?;
            let raw = transaction.rlp_signed(&signature);
            let hash = TxHash::from(keccak256(&raw));

            match self.provider.send_raw_transaction(raw.clone()).await {
                Ok(_) => {
                    nonces.next = Some(nonce + 1);
                    return Ok(hash);
                }
                Err(error) if answered(&error) && already_known(&error) => {
                    nonces.next = Some(nonce + 1);
                    return Ok(hash);
                }
                Err(error) if answered(&error) => {
                    // The node refused it, so this nonce is unspent. Whether
                    // the local count is still right is another matter, so
                    // the next write reads `pending` again.
                    nonces.next = None;
                    if nonce_conflict(&error) && !reseeded {
                        // Never accepted, so signing it again at a fresh
                        // `pending` nonce is this operation's first send,
                        // not a second one.
                        reseeded = true;
                        continue;
                    }
                    return Err(SettlementError::Backend(format!(
                        "transaction {hash:#x} was refused by the node and never sent: {error}"
                    )));
                }
                Err(lost) => {
                    let seen = self.resolve_lost_send(hash, raw.clone()).await;
                    tracing::warn!(
                        tx_hash = %format!("{hash:#x}"),
                        error = %lost,
                        seen,
                        "send answer lost"
                    );
                    // Seen: the nonce is spent. Not seen: it may or may not
                    // be, so the next write reads `pending` and, if `pending`
                    // has not moved past this nonce, offers these same bytes
                    // again rather than signing something else over them.
                    if seen {
                        nonces.next = Some(nonce + 1);
                    } else {
                        nonces.next = None;
                        nonces.unresolved = Some((nonce, raw));
                    }
                    return Ok(hash);
                }
            }
        }
    }

    /// Read the next nonce from `pending`, never below a transaction whose
    /// send is unresolved: if `pending` has not moved past it, its same
    /// signed bytes are offered again (they cannot run twice) and the next
    /// write takes the nonce after it. That is what keeps a lagging
    /// backend's `pending` from rewinding the count onto a nonce that may
    /// already carry a transaction.
    async fn seed_nonce(&self, nonces: &mut NonceState) -> Result<U256, SettlementError> {
        let own = self.wallet.address();
        let pending = retry_read(|| {
            self.provider
                .get_transaction_count(own, Some(BlockNumber::Pending.into()))
        })
        .await
        .map_err(|error| {
            SettlementError::Backend(format!(
                "could not read this account's pending nonce: {error}"
            ))
        })?;
        let nonce = match nonces.unresolved.take() {
            Some((stuck, raw)) if pending <= stuck => {
                if let Err(error) = self.provider.send_raw_transaction(raw).await {
                    tracing::warn!(
                        nonce = %stuck,
                        error = %error,
                        "unresolved transaction re-offer failed"
                    );
                }
                stuck + 1
            }
            _ => pending,
        };
        nonces.next = Some(nonce);
        Ok(nonce)
    }

    /// Whether the node has `hash` after a send whose answer was lost:
    /// looked up first, then offered the **same** signed bytes once more.
    async fn resolve_lost_send(&self, hash: TxHash, raw: ethers::types::Bytes) -> bool {
        if let Ok(Some(_)) = self.provider.get_transaction(hash).await {
            return true;
        }
        match self.provider.send_raw_transaction(raw).await {
            Ok(_) => true,
            Err(error) => answered(&error) && already_known(&error),
        }
    }
}

/// "already known" (geth, anvil) or "known transaction" (older nodes): the
/// node has these exact bytes already.
fn already_known(error: &ProviderError) -> bool {
    let message = error.to_string().to_ascii_lowercase();
    message.contains("already known") || message.contains("known transaction")
}

/// The node refused the nonce itself: it is spent, or another pending
/// transaction holds it.
fn nonce_conflict(error: &ProviderError) -> bool {
    let message = error.to_string().to_ascii_lowercase();
    message.contains("nonce too low")
        || message.contains("replacement transaction underpriced")
        || message.contains("nonce has already been used")
}

/// Wait for `hash`'s receipt and require it to have succeeded (issue #425:
/// a reverted transaction is still mined, and only `status` tells). See the
/// module doc for how long this waits and what it says when it stops.
pub(crate) async fn confirm(
    provider: &Provider<EvmRpc>,
    hash: TxHash,
    policy: ConfirmPolicy,
) -> Result<TransactionReceipt, SettlementError> {
    let started = Instant::now();
    let mut observed = false;
    // When polls began answering "not found" without a failure in between.
    // Only answered polls count toward "not observed": a circuit that is
    // down says nothing about the transaction (ADR 0073 decision 5).
    let mut unseen_since: Option<Instant> = None;
    let mut failures = 0u32;
    let mut last_error = String::from("none");
    loop {
        let mut failed = false;
        match provider.get_transaction_receipt(hash).await {
            Ok(Some(receipt)) => {
                if receipt.status == Some(U64::zero()) {
                    return Err(SettlementError::Backend(format!(
                        "transaction {:#x} reverted on chain",
                        receipt.transaction_hash
                    )));
                }
                return Ok(receipt);
            }
            Ok(None) => {}
            Err(error) => {
                failed = true;
                last_error = error.to_string();
            }
        }

        // A receipt is the answer. The transaction itself is looked for only
        // to tell "the endpoint has never seen it" from "it is pending".
        // Against a load-balanced endpoint the first can be a lagging
        // backend, so it gets a window rather than a verdict.
        if !observed {
            match provider.get_transaction(hash).await {
                Ok(Some(_)) => observed = true,
                Ok(None) => {}
                Err(error) => {
                    failed = true;
                    last_error = error.to_string();
                }
            }
        }

        if failed {
            unseen_since = None;
            failures += 1;
        } else {
            failures = 0;
            if !observed {
                unseen_since.get_or_insert_with(Instant::now);
            }
        }

        let unseen_for = unseen_since.map(|since| since.elapsed());
        if !observed && unseen_for.is_some_and(|unseen| unseen >= policy.unobserved) {
            return Err(SettlementError::Backend(format!(
                "transaction {hash:#x} was not observed: this endpoint answered 'not found' for \
                 {}s -- this is not proof it was dropped, only that this endpoint has not \
                 confirmed it yet; check its status by hash (a block explorer or a fresh \
                 eth_getTransactionReceipt call) before resubmitting, since a transaction that \
                 later mines would be double-spent by a retry",
                policy.unobserved.as_secs()
            )));
        }
        let waited = started.elapsed();
        if waited >= policy.deadline {
            let seen = if observed {
                "was observed but not mined"
            } else {
                "could not be confirmed"
            };
            return Err(SettlementError::Backend(format!(
                "transaction {hash:#x} {seen} within {}s (last RPC error: {last_error}); it may \
                 still mine, so check its status by hash before resubmitting, since a retry of a \
                 transaction that later mines is a double spend",
                waited.as_secs()
            )));
        }
        tokio::time::sleep(failure_backoff(policy.poll, failures)).await;
    }
}

/// The wait before the next poll: `poll`, doubled for each poll in a row
/// that failed, up to 8s (ADR 0073 decision 5: "keeps polling after a
/// transient error, with backoff"). An endpoint that is refusing or down is
/// not polled at full speed, and the first answered poll resets it.
pub(crate) fn failure_backoff(poll: Duration, failures: u32) -> Duration {
    const CEILING: Duration = Duration::from_secs(8);
    poll.saturating_mul(2u32.saturating_pow(failures.min(8)))
        .min(CEILING.max(poll))
}

/// Issue #907 and ADR 0073, against a real chain behind a [`FakeRpc`] that
/// misbehaves on cue: a transaction that mines must never be reported as
/// failed or dropped, and a lost send answer must never become a second
/// transaction.
///
/// [`FakeRpc`]: connector_chain_rpc::FakeRpc
#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use connector_chain_rpc::{FakeRpc, RpcReply};
    use ethers::types::TransactionRequest;

    use super::*;
    use crate::test_support::{require_anvil, Anvil, DEPLOYER_PRIVATE_KEY};

    const FAST: ConfirmPolicy = ConfirmPolicy {
        poll: Duration::from_millis(50),
        unobserved: Duration::from_secs(2),
        deadline: Duration::from_secs(10),
    };

    fn sender(url: &str) -> (Arc<Provider<EvmRpc>>, Sender) {
        let provider = Arc::new(EvmRpc::provider(
            RpcTransport::direct(url).expect("transport"),
        ));
        let wallet: LocalWallet = DEPLOYER_PRIVATE_KEY
            .parse::<LocalWallet>()
            .expect("valid key")
            .with_chain_id(31_337u64);
        (Arc::clone(&provider), Sender::new(provider, wallet))
    }

    fn self_transfer(sender: &Sender) -> TypedTransaction {
        TransactionRequest::new()
            .to(sender.address())
            .value(1u64)
            .into()
    }

    async fn nonce_on_chain(anvil: &Anvil, address: Address) -> U256 {
        EvmRpc::provider(RpcTransport::direct(&anvil.rpc_url).expect("transport"))
            .get_transaction_count(address, Some(BlockNumber::Latest.into()))
            .await
            .expect("nonce")
    }

    /// The repro from #907: a load-balanced endpoint whose
    /// `eth_getTransactionByHash` never shows the transaction, although it
    /// mined.
    #[tokio::test]
    async fn a_transaction_the_endpoint_never_showed_pending_but_did_mine_still_confirms() {
        if !require_anvil() {
            return;
        }
        let anvil = Anvil::spawn(19_700).await;
        let flaky = FakeRpc::spawn_in_front_of(&anvil.rpc_url, |call| {
            if call.method == "eth_getTransactionByHash" {
                RpcReply::Result(serde_json::Value::Null)
            } else {
                RpcReply::Forward
            }
        })
        .await;
        let (provider, sender) = sender(&flaky.url());

        let hash = sender.send(self_transfer(&sender)).await.expect("send");
        let receipt = confirm(&provider, hash, FAST)
            .await
            .expect("a transaction that mined confirms");
        assert_eq!(receipt.transaction_hash, hash);
    }

    #[tokio::test]
    async fn a_transaction_truly_never_observed_is_reported_as_not_observed_not_dropped() {
        if !require_anvil() {
            return;
        }
        let anvil = Anvil::spawn(19_750).await;
        let (provider, _) = sender(&anvil.rpc_url);
        let hash = TxHash::from_low_u64_be(1);

        let SettlementError::Backend(message) = confirm(&provider, hash, FAST).await.unwrap_err()
        else {
            panic!("expected SettlementError::Backend");
        };
        assert!(!message.contains("dropped before mining"), "{message}");
        assert!(message.contains("not observed"), "{message}");
        assert!(message.contains(&format!("{hash:#x}")), "{message}");
    }

    /// ADR 0073: receipt polls that time out, get a 403 or lose their
    /// connection are not the transaction's outcome.
    #[tokio::test]
    async fn receipt_polls_that_fail_do_not_end_the_wait_for_a_transaction_that_mines() {
        if !require_anvil() {
            return;
        }
        let anvil = Anvil::spawn(19_800).await;
        let flaky = FakeRpc::spawn_in_front_of(&anvil.rpc_url, |call| {
            match (call.method.as_str(), call.nth) {
                ("eth_getTransactionReceipt", 0) => RpcReply::Drop,
                ("eth_getTransactionReceipt", 1) => RpcReply::Status(403),
                ("eth_getTransactionReceipt", 2) => RpcReply::Status(502),
                _ => RpcReply::Forward,
            }
        })
        .await;
        let (provider, sender) = sender(&flaky.url());

        let hash = sender.send(self_transfer(&sender)).await.expect("send");
        confirm(&provider, hash, FAST)
            .await
            .expect("failed polls are not an outcome");
    }

    /// ADR 0073: the wait ends. A transaction the endpoint holds as pending
    /// forever is reported, by hash, once the deadline passes.
    #[tokio::test]
    async fn a_transaction_that_never_mines_is_reported_at_the_deadline_by_hash() {
        let hash = TxHash::from_low_u64_be(7);
        let pending = FakeRpc::spawn(move |call| match call.method.as_str() {
            "eth_getTransactionReceipt" => RpcReply::Result(serde_json::Value::Null),
            "eth_getTransactionByHash" => RpcReply::Result(serde_json::json!({
                "hash": format!("{hash:#x}"),
                "nonce": "0x0", "blockHash": null, "blockNumber": null,
                "transactionIndex": null, "from": format!("{:#x}", Address::zero()),
                "to": null, "value": "0x0", "gasPrice": "0x1", "gas": "0x5208",
                "input": "0x", "v": "0x1b", "r": "0x1", "s": "0x1", "type": "0x0",
            })),
            other => panic!("confirm does not call {other}"),
        })
        .await;
        let provider = EvmRpc::provider(RpcTransport::direct(&pending.url()).expect("transport"));
        let policy = ConfirmPolicy {
            poll: Duration::from_millis(20),
            unobserved: Duration::from_millis(100),
            deadline: Duration::from_millis(400),
        };

        let started = Instant::now();
        let error = confirm(&provider, hash, policy)
            .await
            .expect_err("it never mines");
        assert!(started.elapsed() < Duration::from_secs(5));
        let text = error.to_string();
        assert!(text.contains("observed but not mined"), "{text}");
        assert!(text.contains(&format!("{hash:#x}")), "{text}");
    }

    /// The ambiguous send, exactly: the node took the transaction and the
    /// answer never came back. The operation must confirm as the one
    /// transaction it is, and the next write must not reuse its nonce.
    #[tokio::test]
    async fn a_send_whose_answer_was_lost_confirms_once_and_the_next_write_gets_the_next_nonce() {
        if !require_anvil() {
            return;
        }
        let anvil = Anvil::spawn(19_850).await;
        let lost_once = Arc::new(AtomicBool::new(false));
        let lost = Arc::clone(&lost_once);
        let flaky = FakeRpc::spawn_in_front_of(&anvil.rpc_url, move |call| {
            if call.method == "eth_sendRawTransaction" && !lost.swap(true, Ordering::SeqCst) {
                RpcReply::ForwardThenDrop
            } else {
                RpcReply::Forward
            }
        })
        .await;
        let (provider, sender) = sender(&flaky.url());
        let before = nonce_on_chain(&anvil, sender.address()).await;

        let first = sender.send(self_transfer(&sender)).await.expect("send");
        confirm(&provider, first, FAST)
            .await
            .expect("the transaction the node took confirms");
        let second = sender.send(self_transfer(&sender)).await.expect("send");
        confirm(&provider, second, FAST)
            .await
            .expect("the next write is not stuck behind a reused nonce");

        assert!(lost_once.load(Ordering::SeqCst), "the answer was lost");
        assert_eq!(
            nonce_on_chain(&anvil, sender.address()).await,
            before + 2,
            "two writes, two transactions: the lost answer did not become a third"
        );
    }

    #[test]
    fn failed_polls_back_off_doubling_to_a_ceiling_and_answered_ones_do_not() {
        let poll = Duration::from_millis(100);
        assert_eq!(failure_backoff(poll, 0), poll);
        assert_eq!(failure_backoff(poll, 1), Duration::from_millis(200));
        assert_eq!(failure_backoff(poll, 3), Duration::from_millis(800));
        assert_eq!(failure_backoff(poll, 30), Duration::from_secs(8));
    }

    /// The case the lookup cannot settle: the answer to a send is lost and
    /// no backend will say it has the transaction. The next write must not
    /// sign a different operation at that nonce (which could replace it).
    /// It offers the same bytes again and takes the nonce after.
    #[tokio::test]
    async fn an_unresolved_lost_send_is_never_overwritten_by_the_next_write() {
        if !require_anvil() {
            return;
        }
        let anvil = Anvil::spawn(19_950).await;
        let blind = Arc::new(AtomicBool::new(true));
        let still_blind = Arc::clone(&blind);
        let lagging = FakeRpc::spawn_in_front_of(&anvil.rpc_url, move |call| {
            let blind = still_blind.load(Ordering::SeqCst);
            match call.method.as_str() {
                // The first send reaches the chain; its answer is lost, and
                // the re-offer's answer is lost too.
                "eth_sendRawTransaction" if blind => RpcReply::ForwardThenDrop,
                // A lagging backend: it has never seen the transaction.
                "eth_getTransactionByHash" if blind => RpcReply::Result(serde_json::Value::Null),
                // Its `pending` never counts the first transaction, for
                // either write.
                "eth_getTransactionCount" => RpcReply::Result(serde_json::json!("0x0")),
                _ => RpcReply::Forward,
            }
        })
        .await;
        let (provider, sender) = sender(&lagging.url());
        let first = sender.send(self_transfer(&sender)).await.expect("send");

        // The backend's lookups catch up for the second write; its
        // `pending` read still says 0, which is the rewind this guards.
        let second = {
            blind.store(false, Ordering::SeqCst);
            sender.send(self_transfer(&sender)).await.expect("send")
        };
        confirm(&provider, first, FAST)
            .await
            .expect("the first operation still lands, at its own nonce");
        confirm(&provider, second, FAST)
            .await
            .expect("the second lands at the next nonce, not over the first");
        assert_ne!(first, second);
    }

    /// A nonce spent behind this sender's back (another client on the same
    /// key) is refused by the node; the operation re-reads `pending` and is
    /// sent once, at the right nonce.
    #[tokio::test]
    async fn a_nonce_spent_elsewhere_is_reseeded_from_pending_and_the_write_still_lands() {
        if !require_anvil() {
            return;
        }
        let anvil = Anvil::spawn(19_900).await;
        let (provider, sender) = sender(&anvil.rpc_url);
        let hash = sender.send(self_transfer(&sender)).await.expect("send");
        confirm(&provider, hash, FAST).await.expect("confirm");

        // Someone else spends this key's next nonce.
        let (other_provider, other) = self::sender(&anvil.rpc_url);
        let hash = other.send(self_transfer(&other)).await.expect("send");
        confirm(&other_provider, hash, FAST).await.expect("confirm");

        let hash = sender
            .send(self_transfer(&sender))
            .await
            .expect("a nonce conflict re-reads pending rather than failing the write");
        confirm(&provider, hash, FAST).await.expect("confirm");
    }
}
