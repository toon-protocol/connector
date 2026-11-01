//! `XrplBatchWatcher` (ADR 0087 decision 8, issue #1617) against a real
//! stand-alone `xrpld`: a payer's exit is answered by landing the latest held
//! voucher with `tfClose`, in one transaction, and a channel with nothing worth
//! landing is closed alone. The watcher is driven one pass at a time.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use axum::extract::State;
use axum::routing::post;
use axum::Router;
use connector_chain_rpc::xrpl::{TxWait, XrplRpc};
use connector_settlement::batch::{
    BatchSettlementBackend, BatchSettlementError, BatchSettlementPayer, ChannelPresentation,
    HeldVoucher, HeldVouchers, ReceiverTerms, XrplReceiverTerms,
};
use connector_settlement::ChannelId;
use connector_settlement_xrpl::test_support::{
    require_xrpld, LedgerCloser, Wallet, Xrpld, STANDALONE_NETWORK_ID,
};
use connector_settlement_xrpl::{RpcTransport, XrplBatchSettlement, XrplBatchWatcher};
use connector_signer::XrplSigner;

const SETTLE_DELAY: u64 = 1_000;
const DEPOSIT: u128 = 10_000;
const WAIT: TxWait = TxWait {
    poll: Duration::from_millis(50),
    give_up_after: Duration::from_secs(30),
};

struct World {
    rpc: XrplRpc,
    payer: XrplBatchSettlement,
    receiver_wallet: Wallet,
    receiver: Arc<XrplBatchSettlement>,
    held: Arc<RwLock<Vec<HeldVoucher>>>,
    watcher: XrplBatchWatcher,
    _closer: LedgerCloser,
}

async fn connect(chain: &Xrpld, wallet: &Wallet) -> XrplBatchSettlement {
    XrplBatchSettlement::connect(
        RpcTransport::direct(&chain.rpc_url).expect("rpc transport"),
        STANDALONE_NETWORK_ID,
        XrplSigner::from_family_seed(&wallet.seed).expect("a family seed"),
        SETTLE_DELAY,
    )
    .await
    .expect("connect to the stand-alone chain")
    .with_wait(WAIT)
}

async fn world(chain: &Xrpld) -> World {
    let rpc = chain.rpc();
    let payer_wallet = chain.funded_wallet(&rpc, 100_000_000_000).await;
    let receiver_wallet = chain.funded_wallet(&rpc, 1_000_000_000).await;
    let closer = chain.close_ledgers_every(Duration::from_millis(200));
    let payer = connect(chain, &payer_wallet).await;
    let receiver = Arc::new(connect(chain, &receiver_wallet).await);
    let held = Arc::new(RwLock::new(Vec::new()));
    let watcher = XrplBatchWatcher::new(
        Arc::clone(&receiver),
        Arc::clone(&held) as Arc<dyn HeldVouchers>,
    );
    World {
        rpc,
        payer,
        receiver_wallet,
        receiver,
        held,
        watcher,
        _closer: closer,
    }
}

impl World {
    /// A channel the payer opens toward the receiver, which admits it.
    async fn channel(&self) -> ChannelId {
        let opened = self
            .payer
            .open(
                ReceiverTerms::Xrpl(XrplReceiverTerms {
                    receiver: self.receiver_wallet.address.clone(),
                    min_settle_delay_secs: SETTLE_DELAY,
                }),
                DEPOSIT,
            )
            .await
            .expect("open");
        self.receiver
            .admit(opened.presentation.clone())
            .await
            .expect("admit");
        opened.presentation.channel().clone()
    }

    /// The payer signs `amount` on `channel` and the receiver holds it.
    async fn hold(&self, channel: &ChannelId, amount: u128) {
        let voucher = self
            .payer
            .sign_voucher(channel, amount)
            .await
            .expect("sign");
        self.held.write().expect("held").push(HeldVoucher {
            presentation: ChannelPresentation::Xrpl {
                channel: channel.clone(),
            },
            voucher,
        });
    }

    async fn drops(&self, address: &str) -> u64 {
        self.rpc
            .account_info(address)
            .await
            .expect("account_info")
            .expect("exists")
            .balance_drops
    }
}

#[tokio::test]
async fn an_exit_with_a_held_voucher_is_landed_with_tf_close() {
    if !require_xrpld() {
        return;
    }
    let chain = Xrpld::spawn().await;
    let world = world(&chain).await;
    let channel = world.channel().await;
    world.hold(&channel, 4_000).await;

    // Nothing is touched while the payer stays.
    assert!(world.watcher.tick().await.expect("tick").is_empty());
    assert!(world
        .rpc
        .ledger_entry(&channel.0)
        .await
        .expect("read")
        .is_some());

    let receiver_before = world.drops(&world.receiver_wallet.address).await;
    world.payer.start_withdrawal(&channel).await.expect("exit");
    assert!(
        world
            .rpc
            .ledger_entry(&channel.0)
            .await
            .expect("read")
            .is_some(),
        "an exit with something unlanded sets Expiration and leaves the entry"
    );
    let payer_before = world.drops(world.payer.address()).await;

    let closed = world.watcher.tick().await.expect("tick");
    assert_eq!(closed.len(), 1);
    assert_eq!(closed[0].channel, channel);
    assert_eq!(closed[0].delivered, 4_000);

    assert_eq!(
        world.rpc.ledger_entry(&channel.0).await.expect("read"),
        None
    );
    let receiver_after = world.drops(&world.receiver_wallet.address).await;
    // The voucher, less the fee for the transaction that landed it.
    assert!(receiver_after > receiver_before + 3_000);
    assert!(receiver_after <= receiver_before + 4_000);
    let payer_after = world.drops(world.payer.address()).await;
    assert_eq!(
        payer_after,
        payer_before + (DEPOSIT as u64 - 4_000),
        "the payer is refunded the rest"
    );

    // The receiving half reads it as gone, and the next pass does not retry.
    assert!(matches!(
        world.receiver.channel_state(&channel).await,
        Err(BatchSettlementError::ChannelNotFound(_))
    ));
    assert!(world.watcher.tick().await.expect("tick").is_empty());
}

#[tokio::test]
async fn an_exit_with_no_voucher_is_closed_and_the_payer_refunded_in_full() {
    if !require_xrpld() {
        return;
    }
    let chain = Xrpld::spawn().await;
    let world = world(&chain).await;
    let channel = world.channel().await;
    // A voucher already landed is not worth landing again.
    world.hold(&channel, 2_500).await;
    let voucher = world.held.read().expect("held")[0].voucher.clone();
    world.receiver.land(&channel, voucher).await.expect("land");
    let quiet = world.channel().await;

    world.payer.start_withdrawal(&channel).await.expect("exit");
    world.payer.start_withdrawal(&quiet).await.expect("exit");
    let payer_before = world.drops(world.payer.address()).await;

    let closed = world.watcher.tick().await.expect("tick");
    assert_eq!(closed.len(), 2);
    assert!(closed.iter().all(|closed| closed.delivered == 0));
    for id in [&channel, &quiet] {
        assert_eq!(world.rpc.ledger_entry(&id.0).await.expect("read"), None);
    }
    let payer_after = world.drops(world.payer.address()).await;
    assert_eq!(
        payer_after,
        payer_before + (DEPOSIT as u64 - 2_500) + DEPOSIT as u64,
        "the payer is refunded what was not landed"
    );
}

/// A real proxy in front of the spawned `xrpld` that makes its *validated*
/// `ledger_entry` reads lag: while pinned, such a read is answered from the
/// pinned ledger, and the node's own answer names that ledger. Every other
/// call, and every unpinned one, goes straight through. It also counts the
/// `ledger_entry` requests it forwards, and the pinned ones the node answered
/// `entryNotFound`, so a test can see the lag really read as an absence.
struct LagProxy {
    url: String,
    pin: Arc<AtomicU32>,
    entry_reads: Arc<AtomicUsize>,
    lagged_absences: Arc<AtomicUsize>,
    down: Arc<AtomicBool>,
    submits: Arc<AtomicUsize>,
}

const UNPINNED: u32 = u32::MAX;

impl LagProxy {
    fn spawn(upstream: &str) -> LagProxy {
        #[derive(Clone)]
        struct Shared {
            upstream: String,
            pin: Arc<AtomicU32>,
            entry_reads: Arc<AtomicUsize>,
            lagged_absences: Arc<AtomicUsize>,
            down: Arc<AtomicBool>,
            submits: Arc<AtomicUsize>,
            client: reqwest::Client,
        }
        async fn forward(
            State(shared): State<Shared>,
            axum::Json(mut request): axum::Json<serde_json::Value>,
        ) -> axum::Json<serde_json::Value> {
            let pin = shared.pin.load(Ordering::SeqCst);
            let mut pinned = false;
            if request["method"] == "submit" {
                shared.submits.fetch_add(1, Ordering::SeqCst);
            }
            if request["method"] == "ledger_entry" {
                shared.entry_reads.fetch_add(1, Ordering::SeqCst);
                if shared.down.load(Ordering::SeqCst) {
                    // Unwinding out of the handler ends hyper's connection
                    // task, so the socket closes with no answer at all.
                    std::panic::resume_unwind(Box::new("the proxy is down"));
                }
                if pin != UNPINNED && request["params"][0]["ledger_index"] == "validated" {
                    request["params"][0]["ledger_index"] = serde_json::json!(pin);
                    pinned = true;
                }
            }
            let response: serde_json::Value = shared
                .client
                .post(&shared.upstream)
                .json(&request)
                .send()
                .await
                .expect("the chain answers")
                .json()
                .await
                .expect("a JSON answer");
            if pinned && response["result"]["error"] == "entryNotFound" {
                shared.lagged_absences.fetch_add(1, Ordering::SeqCst);
            }
            axum::Json(response)
        }

        let down = Arc::new(AtomicBool::new(false));
        let submits = Arc::new(AtomicUsize::new(0));
        let pin = Arc::new(AtomicU32::new(UNPINNED));
        let entry_reads = Arc::new(AtomicUsize::new(0));
        let lagged_absences = Arc::new(AtomicUsize::new(0));
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind the proxy");
        let addr = listener.local_addr().expect("the proxy's address");
        let app = Router::new().route("/", post(forward)).with_state(Shared {
            upstream: upstream.to_string(),
            pin: pin.clone(),
            entry_reads: entry_reads.clone(),
            lagged_absences: lagged_absences.clone(),
            down: down.clone(),
            submits: submits.clone(),
            client: reqwest::Client::builder()
                .no_proxy()
                .build()
                .expect("the proxy's client"),
        });
        tokio::spawn(async move {
            let _ = axum::Server::from_tcp(listener)
                .expect("serve the proxy")
                .serve(app.into_make_service())
                .await;
        });
        LagProxy {
            url: format!("http://{addr}"),
            pin,
            entry_reads,
            lagged_absences,
            down,
            submits,
        }
    }

    /// While down, every `ledger_entry` request's connection is dropped
    /// unanswered.
    fn set_down(&self, down: bool) {
        self.down.store(down, Ordering::SeqCst);
    }

    fn lag_at(&self, ledger: u32) {
        self.pin.store(ledger, Ordering::SeqCst);
    }

    fn catch_up(&self) {
        self.lag_at(UNPINNED);
    }

    fn reads(&self) -> usize {
        self.entry_reads.load(Ordering::SeqCst)
    }

    /// Every transaction sent through the proxy.
    fn submits(&self) -> usize {
        self.submits.load(Ordering::SeqCst)
    }

    fn lagged_absences(&self) -> usize {
        self.lagged_absences.load(Ordering::SeqCst)
    }
}

fn presentation(channel: &ChannelId) -> ChannelPresentation {
    ChannelPresentation::Xrpl {
        channel: channel.clone(),
    }
}

async fn validated_ledger(rpc: &XrplRpc) -> u32 {
    rpc.server_info()
        .await
        .expect("server_info")
        .validated_ledger
        .expect("a validated ledger")
}

impl World {
    /// A watcher whose every `ledger_entry` read goes through `proxy`, over
    /// a backend that has admitted `channels`, reading the same held vouchers.
    async fn proxied_watcher(&self, proxy: &LagProxy, channels: &[&ChannelId]) -> XrplBatchWatcher {
        let (backend, watcher) = self.bare_proxied_watcher(proxy).await;
        for channel in channels {
            backend
                .admit(presentation(channel))
                .await
                .expect("admit through the proxy");
        }
        watcher
    }

    /// A watcher over a backend reached through `proxy` that has admitted
    /// nothing; `restore` is how a test hands it a channel.
    async fn bare_proxied_watcher(
        &self,
        proxy: &LagProxy,
    ) -> (Arc<XrplBatchSettlement>, XrplBatchWatcher) {
        let backend = Arc::new(
            XrplBatchSettlement::connect(
                RpcTransport::direct(&proxy.url).expect("rpc transport"),
                STANDALONE_NETWORK_ID,
                XrplSigner::from_family_seed(&self.receiver_wallet.seed).expect("a family seed"),
                SETTLE_DELAY,
            )
            .await
            .expect("connect through the proxy")
            .with_wait(WAIT),
        );
        let watcher = XrplBatchWatcher::new(
            Arc::clone(&backend),
            Arc::clone(&self.held) as Arc<dyn HeldVouchers>,
        );
        (backend, watcher)
    }

    async fn next_ledger(&self) {
        let now = validated_ledger(&self.rpc).await;
        while validated_ledger(&self.rpc).await <= now {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

/// An endpoint behind the channel's creation reads it as absent. That is no
/// reason to stop watching: once it catches up, an exit is answered.
#[tokio::test]
async fn a_watcher_behind_a_channels_creation_still_answers_its_exit() {
    if !require_xrpld() {
        return;
    }
    let chain = Xrpld::spawn().await;
    let world = world(&chain).await;
    let proxy = LagProxy::spawn(&chain.rpc_url);
    let before_creation = validated_ledger(&world.rpc).await;
    let channel = world.channel().await;
    world.hold(&channel, 4_000).await;
    let watcher = world.proxied_watcher(&proxy, &[&channel]).await;

    proxy.lag_at(before_creation);
    for _ in 0..2 {
        assert!(watcher.tick().await.expect("tick").is_empty());
        world.next_ledger().await;
    }
    assert_eq!(
        proxy.lagged_absences(),
        2,
        "every lagged pass read the channel as absent"
    );
    proxy.catch_up();
    world.payer.start_withdrawal(&channel).await.expect("exit");

    let closed = watcher.tick().await.expect("tick");
    assert_eq!(closed.len(), 1, "the exit is answered with no restart");
    assert_eq!(closed[0].delivered, 4_000);
}

/// The same when the lag starts after the watcher has found the channel: the
/// absent read is below the floor, so it proves nothing.
#[tokio::test]
async fn an_absence_below_the_floor_proves_nothing() {
    if !require_xrpld() {
        return;
    }
    let chain = Xrpld::spawn().await;
    let world = world(&chain).await;
    let proxy = LagProxy::spawn(&chain.rpc_url);
    let before_creation = validated_ledger(&world.rpc).await;
    let channel = world.channel().await;
    world.hold(&channel, 4_000).await;
    let watcher = world.proxied_watcher(&proxy, &[&channel]).await;

    assert!(watcher.tick().await.expect("tick").is_empty());
    world.next_ledger().await;
    proxy.lag_at(before_creation);
    for _ in 0..2 {
        assert!(watcher.tick().await.expect("tick").is_empty());
        world.next_ledger().await;
    }
    assert_eq!(
        proxy.lagged_absences(),
        2,
        "every lagged pass read the channel as absent"
    );
    proxy.catch_up();
    world.payer.start_withdrawal(&channel).await.expect("exit");

    let closed = watcher.tick().await.expect("tick");
    assert_eq!(closed.len(), 1);
    assert_eq!(closed[0].delivered, 4_000);
}

/// A channel really deleted, read at or past its floor, is not read again; one
/// a watcher never found (a fresh watcher, as after a restart) is read every
/// pass, closes nothing, and the pass does not fail.
#[tokio::test]
async fn a_deleted_channel_is_read_no_more_only_with_a_floor() {
    if !require_xrpld() {
        return;
    }
    let chain = Xrpld::spawn().await;
    let world = world(&chain).await;
    let proxy = LagProxy::spawn(&chain.rpc_url);
    let channel = world.channel().await;
    world.hold(&channel, 4_000).await;
    let with_floor = world.proxied_watcher(&proxy, &[&channel]).await;
    let without_floor = world.proxied_watcher(&proxy, &[&channel]).await;

    // Finding the channel sets the floor.
    assert!(with_floor.tick().await.expect("tick").is_empty());

    // Another watcher answers the exit and deletes the channel.
    world.payer.start_withdrawal(&channel).await.expect("exit");
    assert_eq!(world.watcher.tick().await.expect("tick").len(), 1);

    let before = proxy.reads();
    assert!(with_floor.tick().await.expect("tick").is_empty());
    assert_eq!(proxy.reads(), before + 1, "the absence is read once");
    assert!(with_floor.tick().await.expect("tick").is_empty());
    assert_eq!(proxy.reads(), before + 1, "and then the channel is done");

    for pass in 1..=3 {
        assert!(without_floor.tick().await.expect("tick").is_empty());
        assert_eq!(
            proxy.reads(),
            before + 1 + pass,
            "with no floor the channel is read every pass"
        );
    }
}

/// A restore that fails on a read error leaves the channel watched, though
/// not admitted: once the endpoint recovers and the payer exits, the channel
/// with no voucher is closed and the payer refunded in full, no restart.
#[tokio::test]
async fn a_restore_that_failed_on_a_read_error_leaves_the_channel_watched() {
    if !require_xrpld() {
        return;
    }
    let chain = Xrpld::spawn().await;
    let world = world(&chain).await;
    let proxy = LagProxy::spawn(&chain.rpc_url);
    let channel = world.channel().await;
    let (backend, watcher) = world.bare_proxied_watcher(&proxy).await;

    proxy.set_down(true);
    let error = backend
        .restore(presentation(&channel))
        .await
        .expect_err("the endpoint is down");
    assert!(!matches!(error, BatchSettlementError::ChannelNotFound(_)));
    proxy.set_down(false);

    assert!(
        matches!(
            backend.channel_state(&channel).await,
            Err(BatchSettlementError::ChannelNotAdmitted(_))
        ),
        "watched is not admitted"
    );

    world.payer.start_withdrawal(&channel).await.expect("exit");
    let payer_before = world.drops(world.payer.address()).await;
    let closed = watcher.tick().await.expect("tick");
    assert_eq!(closed.len(), 1);
    assert_eq!(closed[0].channel, channel);
    assert_eq!(
        world.rpc.ledger_entry(&channel.0).await.expect("read"),
        None
    );
    assert_eq!(
        world.drops(world.payer.address()).await,
        payer_before + DEPOSIT as u64,
        "the payer is refunded in full"
    );
    assert!(matches!(
        backend.channel_state(&channel).await,
        Err(BatchSettlementError::ChannelNotAdmitted(_))
    ));
}

/// A restore the ledger answered `ChannelNotFound` is not watched: later
/// passes do not read the channel.
#[tokio::test]
async fn a_restore_answered_not_found_is_not_watched() {
    if !require_xrpld() {
        return;
    }
    let chain = Xrpld::spawn().await;
    let world = world(&chain).await;
    let proxy = LagProxy::spawn(&chain.rpc_url);
    let channel = world.channel().await;
    world.payer.start_withdrawal(&channel).await.expect("exit");
    assert_eq!(world.watcher.tick().await.expect("tick").len(), 1);
    let (backend, watcher) = world.bare_proxied_watcher(&proxy).await;

    assert!(matches!(
        backend.restore(presentation(&channel)).await,
        Err(BatchSettlementError::ChannelNotFound(_))
    ));
    let before = proxy.reads();
    for _ in 0..3 {
        assert!(watcher.tick().await.expect("tick").is_empty());
    }
    assert_eq!(proxy.reads(), before, "the channel is never read");
}

/// A channel kept watched by a failed restore is closed only if this node is
/// its destination.
#[tokio::test]
async fn a_watched_channel_payable_to_another_account_is_not_closed() {
    if !require_xrpld() {
        return;
    }
    let chain = Xrpld::spawn().await;
    let world = world(&chain).await;
    let proxy = LagProxy::spawn(&chain.rpc_url);
    let other = chain.funded_wallet(&world.rpc, 1_000_000_000).await;
    let opened = world
        .payer
        .open(
            ReceiverTerms::Xrpl(XrplReceiverTerms {
                receiver: other.address.clone(),
                min_settle_delay_secs: SETTLE_DELAY,
            }),
            DEPOSIT,
        )
        .await
        .expect("open");
    let channel = opened.presentation.channel().clone();
    let (backend, watcher) = world.bare_proxied_watcher(&proxy).await;

    proxy.set_down(true);
    backend
        .restore(presentation(&channel))
        .await
        .expect_err("the endpoint is down");
    proxy.set_down(false);

    world.payer.start_withdrawal(&channel).await.expect("exit");
    let before = proxy.reads();
    assert!(watcher.tick().await.expect("tick").is_empty());
    assert!(proxy.reads() > before, "the channel is read");
    assert_eq!(proxy.submits(), 0, "no close is even attempted");
    assert!(world
        .rpc
        .ledger_entry(&channel.0)
        .await
        .expect("read")
        .is_some());
}
