//! `XrplBatchSettlement`'s paying half against a real stand-alone `xrpld`,
//! held to the paying contract suite unmodified (ADR 0007, ADR 0075 decision
//! 8, ADR 0087), plus what the suite cannot ask of every chain: a lapsed
//! open, a retry that creates nothing new, one sequence allocator, the
//! pre-flight shortfall, no `CancelAfter` and a channel that is gone
//! (issue #1613).

use std::sync::Arc;
use std::time::Duration;

use connector_chain_rpc::xrpl::{TxWait, XrplRpc};
use connector_settlement::batch::contract::{
    assert_upholds_the_paying_contract, PayingContractFixture,
};
use connector_settlement::batch::{
    BatchChannelStatus, BatchSettlementBackend, BatchSettlementError, BatchSettlementPayer,
    OutboundChannelRecord, ReceiverTerms, VoucherSigner, XrplReceiverTerms,
};
use connector_settlement::ChannelId;
use connector_settlement_xrpl::test_support::{
    require_xrpld, LedgerCloser, Wallet, Xrpld, STANDALONE_NETWORK_ID,
};
use connector_settlement_xrpl::{RpcTransport, XrplBatchSettlement};
use connector_signer::XrplSigner;
use serde_json::json;

/// The receiver's published minimum `SettleDelay`: short, so a withdrawal's
/// delay can be waited out in closes, and long against the few seconds the
/// suite takes between starting one and being refused finishing it.
const SETTLE_DELAY: u64 = 120;
const WAIT: TxWait = TxWait {
    poll: Duration::from_millis(50),
    give_up_after: Duration::from_secs(30),
};
const PAYER_DROPS: u64 = 100_000_000_000;

struct World {
    chain: Xrpld,
    rpc: XrplRpc,
    payer_wallet: Wallet,
    payer: XrplBatchSettlement,
    receiver_wallet: Wallet,
    receiver: XrplBatchSettlement,
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

async fn world() -> World {
    let chain = Xrpld::spawn().await;
    let rpc = chain.rpc();
    let payer_wallet = chain.funded_wallet(&rpc, PAYER_DROPS).await;
    let receiver_wallet = chain.funded_wallet(&rpc, 1_000_000_000).await;
    let closer = chain.close_ledgers_every(Duration::from_millis(200));
    let payer = connect(&chain, &payer_wallet).await;
    let receiver = connect(&chain, &receiver_wallet).await;
    World {
        chain,
        rpc,
        payer_wallet,
        payer,
        receiver_wallet,
        receiver,
        _closer: closer,
    }
}

impl World {
    fn terms(&self) -> ReceiverTerms {
        ReceiverTerms::Xrpl(XrplReceiverTerms {
            receiver: self.receiver_wallet.address.clone(),
            min_settle_delay_secs: SETTLE_DELAY,
        })
    }

    async fn restarted(&self) -> XrplBatchSettlement {
        connect(&self.chain, &self.payer_wallet).await
    }
}

/// The drops `address` holds in a validated ledger, plus every fee it has
/// paid in one: where a chain's fee is paid in the asset under test, the
/// suite's exact balance assertions need the fees added back (ADR 0075,
/// "Update (issue #1565)" rule 6).
async fn balance_plus_fees(rpc: &XrplRpc, address: &str) -> u128 {
    let balance = rpc
        .account_info(address)
        .await
        .expect("account_info")
        .expect("the account exists")
        .balance_drops;
    let history = rpc
        .call(
            "account_tx",
            json!({
                "account": address,
                "ledger_index_min": -1,
                "ledger_index_max": -1,
                "limit": 1000,
            }),
        )
        .await
        .expect("account_tx");
    let fees: u128 = history["transactions"]
        .as_array()
        .expect("transactions")
        .iter()
        .filter(|entry| entry["validated"].as_bool() == Some(true))
        .filter_map(|entry| {
            let tx = if entry["tx"].is_object() {
                &entry["tx"]
            } else {
                &entry["tx_json"]
            };
            (tx["Account"].as_str() == Some(address))
                .then(|| tx["Fee"].as_str()?.parse::<u128>().ok())
                .flatten()
        })
        .sum();
    u128::from(balance) + fees
}

/// A channel the payer is not the source of: made by the receiver toward
/// the payer.
async fn not_outbound(world: &World) -> ChannelId {
    let channel_terms = ReceiverTerms::Xrpl(XrplReceiverTerms {
        receiver: world.payer_wallet.address.clone(),
        min_settle_delay_secs: SETTLE_DELAY,
    });
    world
        .receiver
        .open(channel_terms, 1_000)
        .await
        .expect("the receiver opens a channel toward the payer")
        .presentation
        .channel()
        .clone()
}

#[tokio::test]
async fn xrpl_batch_settlement_upholds_the_paying_contract() {
    if !require_xrpld() {
        return;
    }
    let world = Arc::new(world().await);
    let not_outbound = not_outbound(&world).await;

    let fixture = PayingContractFixture {
        payer: Arc::new(world.restarted().await) as Arc<dyn BatchSettlementPayer>,
        receiver: Arc::new(connect(&world.chain, &world.receiver_wallet).await)
            as Arc<dyn BatchSettlementBackend>,
        terms: world.terms(),
        payer_balance: {
            let world = Arc::clone(&world);
            Box::new(move || {
                let world = Arc::clone(&world);
                Box::pin(
                    async move { balance_plus_fees(&world.rpc, &world.payer_wallet.address).await },
                )
            })
        },
        payer_settlement_key: VoucherSigner::Xrpl(world.payer.settlement_public_key()),
        let_delay_pass: {
            let world = Arc::clone(&world);
            Box::new(move || {
                let world = Arc::clone(&world);
                Box::pin(async move {
                    world
                        .chain
                        .let_close_time_pass(&world.rpc, SETTLE_DELAY + 5)
                        .await;
                })
            })
        },
        not_outbound,
        restart: {
            let world = Arc::clone(&world);
            Box::new(move || {
                let world = Arc::clone(&world);
                Box::pin(async move {
                    Arc::new(world.restarted().await) as Arc<dyn BatchSettlementPayer>
                })
            })
        },
    };
    assert_upholds_the_paying_contract(|| async move { fixture }).await;
}

fn channel_of(record: &OutboundChannelRecord) -> ChannelId {
    record.channel().clone()
}

/// A prepared open past its `LastLedgerSequence` without landing is
/// `OpenLapsed`; so is one whose `Sequence` something else spent. Neither
/// spends anything, and a fresh open afterwards is safe.
#[tokio::test]
async fn a_lapsed_open_is_open_lapsed() {
    if !require_xrpld() {
        return;
    }
    let world = world().await;
    let payer = world.restarted().await.with_open_window(3);
    let before = balance_plus_fees(&world.rpc, &world.payer_wallet.address).await;

    // Past its LastLedgerSequence.
    let record = payer
        .prepare_open(world.terms(), 1_000)
        .await
        .expect("prepare");
    for _ in 0..8 {
        world.rpc.ledger_accept().await.expect("close a ledger");
    }
    assert_eq!(
        payer.open_prepared(&record).await.unwrap_err(),
        BatchSettlementError::OpenLapsed(channel_of(&record))
    );
    assert_eq!(
        payer.open_prepared(&record).await.unwrap_err(),
        BatchSettlementError::OpenLapsed(channel_of(&record)),
        "a record that has lapsed opens nothing, ever"
    );

    // Its Sequence taken by something else, inside the window.
    let payer = world.restarted().await;
    let record = payer
        .prepare_open(world.terms(), 1_000)
        .await
        .expect("prepare");
    let taken = world
        .rpc
        .sign_and_submit(
            json!({
                "TransactionType": "AccountSet",
                "Account": world.payer_wallet.address,
            }),
            &world.payer_wallet.seed,
        )
        .await
        .expect("submit");
    assert_eq!(taken.engine_result, "tesSUCCESS");
    world.rpc.ledger_accept().await.expect("close a ledger");
    world.rpc.ledger_accept().await.expect("close a ledger");
    assert_eq!(
        payer.open_prepared(&record).await.unwrap_err(),
        BatchSettlementError::OpenLapsed(channel_of(&record))
    );

    assert_eq!(
        world
            .rpc
            .ledger_entry(&channel_of(&record).0)
            .await
            .expect("read"),
        None
    );
    let after = balance_plus_fees(&world.rpc, &world.payer_wallet.address).await;
    assert_eq!(after, before, "a lapsed open spends nothing but fees");
    payer
        .open(world.terms(), 1_000)
        .await
        .expect("a fresh open is safe");
}

/// A retry after the blob landed creates nothing new: one channel, one
/// deposit, however many times and from however many restarts the record is
/// opened -- the retried open lands the same channel id.
#[tokio::test]
async fn a_retried_open_lands_the_same_channel_and_creates_nothing_new() {
    if !require_xrpld() {
        return;
    }
    let world = world().await;
    let record = world
        .payer
        .prepare_open(world.terms(), 2_500)
        .await
        .expect("prepare");
    let channel = channel_of(&record);

    // The id is known before anything is sent, and no `CancelAfter` is in
    // the blob.
    assert_eq!(
        world.rpc.ledger_entry(&channel.0).await.expect("read"),
        None
    );
    let first = world.payer.open_prepared(&record).await.expect("open");
    assert_eq!(first.presentation.channel(), &channel);
    let held = world
        .rpc
        .account_channels(&world.payer_wallet.address, None)
        .await
        .expect("channels");
    assert_eq!(held.len(), 1);
    let spent = balance_plus_fees(&world.rpc, &world.payer_wallet.address).await;

    // Same process, then a restarted one, then a decoded journal entry.
    let again = world.payer.open_prepared(&record).await.expect("retry");
    assert_eq!(again, first);
    let restarted = world.restarted().await;
    assert_eq!(
        restarted.open_prepared(&record).await.expect("retry"),
        first
    );
    let journaled = OutboundChannelRecord::decode(&record.encode_on("xrpl:1000"))
        .expect("the journal's bytes decode")
        .record;
    assert_eq!(journaled, record);
    assert_eq!(
        restarted.open_prepared(&journaled).await.expect("retry"),
        first
    );

    let held = world
        .rpc
        .account_channels(&world.payer_wallet.address, None)
        .await
        .expect("channels");
    assert_eq!(held.len(), 1, "no second channel");
    assert_eq!(held[0]["amount"], "2500");
    assert_eq!(
        balance_plus_fees(&world.rpc, &world.payer_wallet.address).await,
        spent,
        "no second deposit"
    );
}

/// No channel this node opens carries `CancelAfter`, and the receiver that
/// refuses one by name admits every one of these.
#[tokio::test]
async fn no_channel_this_node_opens_carries_cancel_after() {
    if !require_xrpld() {
        return;
    }
    let world = world().await;
    let opened = world.payer.open(world.terms(), 1_000).await.expect("open");
    let channel = opened.presentation.channel().clone();
    let entry = world
        .rpc
        .ledger_entry(&channel.0)
        .await
        .expect("read")
        .expect("the channel exists");
    assert_eq!(entry["LedgerEntryType"], "PayChannel");
    assert!(entry.get("CancelAfter").is_none(), "{entry}");
    assert!(entry.get("Expiration").is_none(), "{entry}");
    assert_eq!(entry["SettleDelay"], SETTLE_DELAY);
    assert_eq!(entry["Destination"], world.receiver_wallet.address);
    world
        .receiver
        .admit(opened.presentation)
        .await
        .expect("the receiver admits it");
}

/// Opens, top-ups and landings from one account, all at once, never collide
/// on `Sequence`: the same node is payer on its own channels and receiver on
/// one a payer opened toward it.
#[tokio::test]
async fn concurrent_open_fund_and_land_never_collide_on_sequence() {
    if !require_xrpld() {
        return;
    }
    let world = world().await;
    // The receiver node is the account under test. A channel toward it, made
    // by the payer, is one it lands on.
    let inbound_terms = ReceiverTerms::Xrpl(XrplReceiverTerms {
        receiver: world.receiver_wallet.address.clone(),
        min_settle_delay_secs: SETTLE_DELAY,
    });
    let inbound = world
        .payer
        .open(inbound_terms, 5_000)
        .await
        .expect("the payer opens toward the node");
    let inbound_channel = inbound.presentation.channel().clone();
    world
        .receiver
        .admit(inbound.presentation)
        .await
        .expect("admit");

    // The node's own channel toward the payer, to top up.
    let toward_payer = ReceiverTerms::Xrpl(XrplReceiverTerms {
        receiver: world.payer_wallet.address.clone(),
        min_settle_delay_secs: SETTLE_DELAY,
    });
    let own = world
        .receiver
        .open(toward_payer.clone(), 10_000)
        .await
        .expect("open");
    let own_channel = own.presentation.channel().clone();

    let mut vouchers = Vec::new();
    for amount in [100, 200, 300] {
        vouchers.push(
            world
                .payer
                .sign_voucher(&inbound_channel, amount)
                .await
                .expect("sign"),
        );
    }
    let node = &world.receiver;
    let (open_a, open_b, top_up_a, top_up_b, land) = tokio::join!(
        node.open(toward_payer.clone(), 1_000),
        node.open(toward_payer.clone(), 1_000),
        node.top_up(&own_channel, 500),
        node.top_up(&own_channel, 700),
        node.land(&inbound_channel, vouchers.pop().expect("a voucher")),
    );
    open_a.expect("open a");
    open_b.expect("open b");
    top_up_a.expect("top-up a");
    top_up_b.expect("top-up b");
    assert_eq!(land.expect("land").state.landed, 300);
    assert_eq!(
        node.outbound_state(&own_channel)
            .await
            .expect("state")
            .on_chain
            .collateral,
        10_000 + 500 + 700
    );

    // A prepared open holds its `Sequence` against this node's own other
    // transactions: a top-up sent while it waits to be opened goes after it,
    // and the open still lands.
    let prepared = node
        .prepare_open(toward_payer, 1_000)
        .await
        .expect("prepare");
    let (opened, topped_up) = tokio::join!(
        async {
            tokio::time::sleep(Duration::from_millis(500)).await;
            node.open_prepared(&prepared).await
        },
        node.top_up(&own_channel, 300),
    );
    assert_eq!(
        opened.expect("the prepared open").presentation.channel(),
        prepared.channel()
    );
    topped_up.expect("the top-up that waited");
}

/// An open or a top-up beyond the spendable balance -- above the reserve and
/// the fee -- is refused by name with the shortfall, and nothing is signed.
#[tokio::test]
async fn an_unaffordable_open_or_top_up_is_refused_with_its_shortfall() {
    if !require_xrpld() {
        return;
    }
    let world = world().await;
    let poor_wallet = world.chain.funded_wallet(&world.rpc, 50_000_000).await;
    let poor = connect(&world.chain, &poor_wallet).await;

    let fee = world.rpc.fee().await.expect("fee");
    let fee = u128::from(fee.open_ledger_fee_drops.max(fee.base_fee_drops));
    let reserve = world.rpc.reserve().await.expect("reserve");
    let account = world
        .rpc
        .account_info(&poor_wallet.address)
        .await
        .expect("info")
        .expect("exists");
    let spendable = u128::from(account.balance_drops)
        - u128::from(reserve.base_drops)
        - u128::from(reserve.owner_drops) * u128::from(account.owner_count + 1);

    let deposit = 80_000_000u128;
    let error = poor.prepare_open(world.terms(), deposit).await.unwrap_err();
    assert_eq!(
        error,
        BatchSettlementError::Underfunded {
            required: deposit + fee,
            spendable,
            shortfall: deposit + fee - spendable,
        }
    );
    assert!(error
        .to_string()
        .contains(&(deposit + fee - spendable).to_string()));
    assert_eq!(
        poor.open(world.terms(), deposit).await.unwrap_err(),
        error,
        "open refuses the same way"
    );
    let untouched = world
        .rpc
        .account_info(&poor_wallet.address)
        .await
        .expect("info")
        .expect("exists");
    assert_eq!(untouched.sequence, account.sequence, "nothing was sent");

    // Affordable, then a top-up beyond what is left. A top-up locks no new
    // reserve.
    let opened = poor.open(world.terms(), 1_000_000).await.expect("open");
    let channel = opened.presentation.channel().clone();
    let account = world
        .rpc
        .account_info(&poor_wallet.address)
        .await
        .expect("info")
        .expect("exists");
    let spendable = u128::from(account.balance_drops)
        - u128::from(reserve.base_drops)
        - u128::from(reserve.owner_drops) * u128::from(account.owner_count);
    let increment = 500_000_000u128;
    assert_eq!(
        poor.top_up(&channel, increment).await.unwrap_err(),
        BatchSettlementError::Underfunded {
            required: increment + fee,
            spendable,
            shortfall: increment + fee - spendable,
        }
    );
    poor.top_up(&channel, 1_000)
        .await
        .expect("an affordable top-up still goes through");
}

/// A fee above the table's cap is refused, naming it.
#[tokio::test]
async fn a_fee_above_the_cap_is_refused() {
    if !require_xrpld() {
        return;
    }
    let world = world().await;
    let capped = world.restarted().await.with_max_fee_drops(1);
    let error = capped.prepare_open(world.terms(), 1_000).await.unwrap_err();
    assert!(
        matches!(&error, BatchSettlementError::Backend(reason) if reason.contains("max_fee_drops")),
        "{error:?}"
    );
}

/// Once the ledger no longer holds a channel, `outbound_state` answers from
/// the cache, `finish_withdrawal` is `NoWithdrawalPending`, and nothing more
/// is signed or funded on it.
#[tokio::test]
async fn a_deleted_channel_is_answered_from_the_cache() {
    if !require_xrpld() {
        return;
    }
    let world = world().await;
    let opened = world.payer.open(world.terms(), 1_000).await.expect("open");
    let channel = opened.presentation.channel().clone();
    world
        .receiver
        .admit(opened.presentation)
        .await
        .expect("admit");

    // The receiver holds everything the channel does, so the payer's close
    // has nothing unlanded to wait for and deletes the entry at once.
    let voucher = world
        .payer
        .sign_voucher(&channel, 1_000)
        .await
        .expect("sign");
    world.receiver.land(&channel, voucher).await.expect("land");
    let state = world.payer.start_withdrawal(&channel).await.expect("start");
    assert_eq!(state.on_chain.landed, 1_000);
    assert_eq!(state.on_chain.collateral, 0);
    assert_eq!(state.signed, 1_000);
    assert_eq!(
        world.rpc.ledger_entry(&channel.0).await.expect("read"),
        None,
        "the ledger has deleted it"
    );

    assert_eq!(
        world.payer.outbound_state(&channel).await.expect("cache"),
        state
    );
    assert_eq!(
        world.payer.finish_withdrawal(&channel).await.unwrap_err(),
        BatchSettlementError::NoWithdrawalPending(channel.clone())
    );
    assert_eq!(
        world.payer.top_up(&channel, 1).await.unwrap_err(),
        BatchSettlementError::ChannelSealed(channel.clone())
    );
    assert_eq!(
        world.payer.start_withdrawal(&channel).await.unwrap_err(),
        BatchSettlementError::ChannelSealed(channel.clone())
    );
    // The receiver, by contrast, finds nothing.
    assert_eq!(
        world.receiver.channel_state(&channel).await.unwrap_err(),
        BatchSettlementError::ChannelNotFound(channel.clone())
    );
}

/// A channel the receiver closes itself (a watcher's `tfClose`, ADR 0087
/// decision 8) is gone from the ledger too: the payer answers from the cache.
#[tokio::test]
async fn a_channel_the_receiver_closed_is_answered_from_the_cache() {
    if !require_xrpld() {
        return;
    }
    let world = world().await;
    let opened = world.payer.open(world.terms(), 1_000).await.expect("open");
    let channel = opened.presentation.channel().clone();
    let before = world.payer.outbound_state(&channel).await.expect("state");
    assert_eq!(before.on_chain.status, BatchChannelStatus::Open);

    let submitted = world
        .rpc
        .sign_and_submit(
            json!({
                "TransactionType": "PaymentChannelClaim",
                "Account": world.receiver_wallet.address,
                "Channel": channel.0,
                "Flags": 0x0002_0000u32,
            }),
            &world.receiver_wallet.seed,
        )
        .await
        .expect("submit");
    assert_eq!(submitted.engine_result, "tesSUCCESS", "{submitted:?}");
    world.rpc.ledger_accept().await.expect("close a ledger");
    world.rpc.ledger_accept().await.expect("close a ledger");
    assert_eq!(
        world.rpc.ledger_entry(&channel.0).await.expect("read"),
        None
    );

    // Absent proves a deletion only past the open's `LastLedgerSequence`
    // (#1637): until then the read is inconclusive and records nothing.
    assert!(matches!(
        world.payer.outbound_state(&channel).await.unwrap_err(),
        BatchSettlementError::ReadInconclusive(_)
    ));
    world.chain.close_ledgers(&world.rpc, 105).await;

    let after = world.payer.outbound_state(&channel).await.expect("cache");
    assert_eq!(after.on_chain.id, channel);
    assert_eq!(after.on_chain.collateral, 0);
    assert_eq!(
        world.payer.finish_withdrawal(&channel).await.unwrap_err(),
        BatchSettlementError::NoWithdrawalPending(channel.clone())
    );
    assert!(matches!(
        world.payer.sign_voucher(&channel, 10).await.unwrap_err(),
        BatchSettlementError::ChannelSealed(_)
    ));
}
