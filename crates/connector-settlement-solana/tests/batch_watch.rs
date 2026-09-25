//! `SolanaBatchWatcher` (ADR 0074 decision 5, issue #1344) against
//! solana-foundation's own `payment-channels` binary in a disposable
//! validator: a payer's `request_close` is answered by `settle_and_seal` of
//! the latest voucher inside the grace period, and every sponsored channel
//! is carried on through `distribute` to a reclaimed rent float -- found by
//! `getProgramAccounts`, never by admission or the journal.
//!
//! The watcher is driven one pass at a time here, so each assertion is about
//! one pass; `connector-cli`'s end-to-end test runs it as the node does.
//!
//! **One payout per `distribute`.** The program folds two or more payouts
//! into one SPL Token `Batch` CPI, an instruction the SPL Token a local
//! validator ships refuses (`InvalidInstruction`, token error 12); only
//! p-token implements it. So each channel here is distributed with exactly
//! one nonzero payout: the closed channel's voucher is its whole deposit, so
//! there is no refund, and the quiet one holds no voucher, so the refund is
//! all there is.
//!
//! Its own test binary: `solana-test-validator` binds fixed ports, so one
//! validator per binary. The ledger is warped forward so a channel can be
//! opened with an `open_slot` far enough back that its reclaim window
//! passes within the test.

use std::str::FromStr;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use connector_settlement::batch::{ChannelPresentation, HeldVoucher, HeldVouchers, Voucher};
use connector_settlement::ChannelId;
use connector_settlement_solana::batch::wire::{self, ChannelStatus, PAYMENT_CHANNELS_PROGRAM_ID};
use connector_settlement_solana::batch::{SolanaBatchSettlement, SolanaBatchWatcher, Step};
use connector_settlement_solana::test_support::{
    create_mint, fund, mint_to, require_solana_test_validator, BatchPayer, SolanaValidator,
};
use connector_settlement_solana::RpcTransport;
use solana_rpc_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{Keypair, Signer};

/// x402's floor for a published minimum `grace_period`.
const GRACE: u32 = 900;
const DEPOSIT: u64 = 1_000;

/// How far back the reclaimable channel's `open_slot` is set: inside the
/// program's 1,500-slot window, so `open` accepts it and a sealed
/// `distribute` still leaves it Distributed rather than deallocating it, and
/// far enough back that `reclaim` unlocks after 80 more slots.
const BACKDATE_SLOTS: u64 = 1_420;

fn program_id() -> Pubkey {
    Pubkey::from_str(PAYMENT_CHANNELS_PROGRAM_ID).expect("base58")
}

fn held(channel: &Pubkey, amount: u64, signature: [u8; 64]) -> HeldVoucher {
    HeldVoucher {
        presentation: ChannelPresentation::Solana {
            channel: ChannelId(channel.to_string()),
        },
        voucher: Voucher {
            cumulative_amount: u128::from(amount),
            signature: signature.to_vec(),
        },
    }
}

async fn read(rpc: &RpcClient, channel: &Pubkey) -> Option<wire::ChannelAccount> {
    rpc.get_account_with_commitment(channel, CommitmentConfig::confirmed())
        .await
        .expect("read")
        .value
        .and_then(|account| wire::ChannelAccount::parse(&account.data))
}

async fn token_balance(rpc: &RpcClient, owner: &Pubkey, mint: &Pubkey) -> u64 {
    let ata = spl_associated_token_account::get_associated_token_address(owner, mint);
    match rpc.get_token_account_balance(&ata).await {
        Ok(balance) => balance.amount.parse().expect("an amount"),
        Err(_) => 0,
    }
}

fn steps_on(taken: &[(ChannelId, Step)], channel: &Pubkey) -> Vec<Step> {
    taken
        .iter()
        .filter(|(id, _)| id.0 == channel.to_string())
        .map(|(_, step)| *step)
        .collect()
}

#[tokio::test]
async fn a_close_is_answered_by_sealing_the_latest_voucher_and_the_rent_comes_home() {
    if !require_solana_test_validator() {
        return;
    }
    let validator = SolanaValidator::spawn_with_args(&["--warp-slot", "10000"]).await;
    let rpc =
        RpcClient::new_with_commitment(validator.rpc_url.clone(), CommitmentConfig::confirmed());
    let authority = Keypair::new();
    fund(&rpc, &authority.pubkey()).await;
    let mint = create_mint(&rpc, &authority, 6).await;
    let sponsor = Keypair::new();
    fund(&rpc, &sponsor.pubkey()).await;
    let payer = BatchPayer::new(&validator.rpc_url, program_id()).await;
    mint_to(&rpc, &authority, &mint, &payer.payer.pubkey(), 1_000_000).await;

    let seed: [u8; 32] = sponsor.to_bytes()[..32].try_into().expect("a seed");
    let backend = Arc::new(
        SolanaBatchSettlement::connect(
            &RpcTransport::direct(&validator.rpc_url).expect("transport"),
            &seed,
            mint,
            u64::from(GRACE),
        )
        .await
        .expect("connect"),
    );
    let vouchers = Arc::new(RwLock::new(Vec::new()));
    let watcher = SolanaBatchWatcher::new(
        Arc::clone(&backend),
        Arc::clone(&vouchers) as Arc<dyn HeldVouchers>,
    );

    // Three channels this node sponsors, none admitted anywhere:
    // `closing` pays its whole deposit and is closed by its payer; `quiet` holds no
    // voucher and is closed too; `open` pays 200 and stays open.
    let mut closing_open = payer
        .admissible_open(&sponsor.pubkey(), &mint, DEPOSIT, GRACE)
        .await;
    closing_open.open_slot -= BACKDATE_SLOTS;
    let closing = payer.open(&closing_open, &sponsor).await.expect("open");
    let quiet = payer
        .open(
            &payer
                .admissible_open(&sponsor.pubkey(), &mint, DEPOSIT, GRACE)
                .await,
            &sponsor,
        )
        .await
        .expect("open");
    let open = payer
        .open(
            &payer
                .admissible_open(&sponsor.pubkey(), &mint, DEPOSIT, GRACE)
                .await,
            &sponsor,
        )
        .await
        .expect("open");
    *vouchers.write().unwrap() = vec![
        held(&closing, DEPOSIT, payer.sign(&closing, DEPOSIT)),
        held(&open, 200, payer.sign(&open, 200)),
    ];

    // Between sweeps, an open channel is left alone.
    assert!(watcher.tick(false).await.expect("tick").is_empty());

    // The payers ask to close.
    payer.request_close(&closing).await.expect("request_close");
    payer.request_close(&quiet).await.expect("request_close");
    let closed = read(&rpc, &closing).await.expect("the channel");
    assert_eq!(closed.status, ChannelStatus::Closing);
    let deadline = closed.closure_started_at + i64::from(closed.grace_period);

    let taken = watcher.tick(false).await.expect("tick");
    assert_eq!(
        steps_on(&taken, &closing),
        vec![Step::SettleAndSeal { amount: DEPOSIT }]
    );
    assert_eq!(steps_on(&taken, &quiet), vec![Step::SealEarly]);
    assert!(steps_on(&taken, &open).is_empty());
    let sealed = read(&rpc, &closing).await.expect("the channel");
    assert_eq!(sealed.status, ChannelStatus::Sealed);
    assert_eq!(sealed.settled, DEPOSIT, "the latest voucher landed");
    let clock = backend.clock().await.expect("clock");
    assert!(
        clock.unix_timestamp < deadline,
        "and it landed inside the grace period"
    );

    // Sealed channels are paid out: what was paid to this node's receiving
    // account, what was not back to the payer.
    let node_before = token_balance(&rpc, &sponsor.pubkey(), &mint).await;
    let payer_before = token_balance(&rpc, &payer.payer.pubkey(), &mint).await;
    let taken = watcher.tick(false).await.expect("tick");
    assert_eq!(steps_on(&taken, &closing), vec![Step::Distribute]);
    assert_eq!(steps_on(&taken, &quiet), vec![Step::Distribute]);
    assert_eq!(
        token_balance(&rpc, &sponsor.pubkey(), &mint).await - node_before,
        DEPOSIT
    );
    assert_eq!(
        token_balance(&rpc, &payer.payer.pubkey(), &mint).await - payer_before,
        DEPOSIT,
        "the quiet channel's whole deposit"
    );
    assert_eq!(
        read(&rpc, &closing).await.expect("still allocated").status,
        ChannelStatus::Distributed,
        "inside its slot window, the channel waits for reclaim"
    );

    // The sweep settles the open channel's voucher.
    let taken = watcher.tick(true).await.expect("tick");
    assert_eq!(steps_on(&taken, &open), vec![Step::Settle { amount: 200 }]);
    let settled = read(&rpc, &open).await.expect("the channel");
    assert_eq!(
        (settled.status, settled.settled),
        (ChannelStatus::Open, 200)
    );

    // Once the window passes, the rent float comes home.
    let rent_before = rpc.get_balance(&sponsor.pubkey()).await.expect("balance");
    let unlocked = closing_open.open_slot + wire::OPEN_SLOT_WINDOW;
    let mut reclaimed = false;
    for _ in 0..240 {
        if rpc.get_slot().await.expect("slot") > unlocked {
            let taken = watcher.tick(false).await.expect("tick");
            if steps_on(&taken, &closing) == vec![Step::Reclaim] {
                reclaimed = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(reclaimed, "the Distributed channel was reclaimed");
    assert!(read(&rpc, &closing).await.is_none(), "deallocated");
    assert!(
        rpc.get_balance(&sponsor.pubkey()).await.expect("balance") > rent_before,
        "its rent returned to the sponsor"
    );
}
