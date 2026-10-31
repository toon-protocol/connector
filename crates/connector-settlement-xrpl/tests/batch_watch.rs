//! `XrplBatchWatcher` (ADR 0087 decision 8, issue #1617) against a real
//! stand-alone `xrpld`: a payer's exit is answered by landing the latest held
//! voucher with `tfClose`, in one transaction, and a channel with nothing worth
//! landing is closed alone. The watcher is driven one pass at a time.

use std::sync::{Arc, RwLock};
use std::time::Duration;

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
