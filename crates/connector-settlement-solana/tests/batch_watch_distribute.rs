//! `SolanaBatchWatcher`'s `distribute` (ADR 0074 decision 5, issue #1358),
//! against solana-foundation's own `payment-channels` binary in a disposable
//! validator.
//!
//! - **Two payouts.** A channel sealed with a voucher above zero and below
//!   its deposit pays two parties, this node its share and the payer the
//!   rest -- the ordinary case. The program sends both payouts in one SPL
//!   Token `Batch` CPI, which only p-token implements; the validator runs it
//!   at the SPL Token id because `SolanaValidator::spawn` loads the committed
//!   mainnet-beta dump there (`test_support::token_program_fixture`).
//! - **A `distribute` that keeps failing.** A Sealed channel whose
//!   `distribute` fails pass after pass is reported by
//!   [`SolanaBatchWatcher::stuck`] (and logged as an error), not only retried;
//!   the report clears once it goes through.
//!
//! The watcher is driven one pass at a time, as in `batch_watch.rs`.

use std::str::FromStr;
use std::sync::{Arc, RwLock};

use connector_settlement::batch::{ChannelPresentation, HeldVoucher, HeldVouchers, Voucher};
use connector_settlement::ChannelId;
use connector_settlement_solana::batch::wire::{self, ChannelStatus, PAYMENT_CHANNELS_PROGRAM_ID};
use connector_settlement_solana::batch::{
    SolanaBatchSettlement, SolanaBatchWatcher, Step, STUCK_AFTER_FAILED_PASSES,
};
use connector_settlement_solana::test_support::{
    create_mint, fund, mint_to, require_solana_test_validator, send, BatchPayer, SolanaValidator,
};
use connector_settlement_solana::RpcTransport;
use solana_rpc_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{Keypair, Signer};

/// x402's floor for a published minimum `grace_period`.
const GRACE: u32 = 900;
const DEPOSIT: u64 = 1_000;
/// The landed voucher: more than nothing, less than the deposit.
const VOUCHER: u64 = 350;

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

/// A validator, a sponsor-keyed backend and its watcher, and one channel
/// that watcher has sealed with [`VOUCHER`] of its [`DEPOSIT`] landed.
struct Sealed {
    _validator: SolanaValidator,
    rpc: RpcClient,
    sponsor: Keypair,
    payer: BatchPayer,
    mint: Pubkey,
    watcher: SolanaBatchWatcher,
    channel: Pubkey,
}

async fn a_channel_sealed_below_its_deposit() -> Sealed {
    let validator = SolanaValidator::spawn().await;
    let rpc =
        RpcClient::new_with_commitment(validator.rpc_url.clone(), CommitmentConfig::confirmed());
    let authority = Keypair::new();
    fund(&rpc, &authority.pubkey()).await;
    let mint = create_mint(&rpc, &authority, 6).await;
    let sponsor = Keypair::new();
    fund(&rpc, &sponsor.pubkey()).await;
    let program = Pubkey::from_str(PAYMENT_CHANNELS_PROGRAM_ID).expect("base58");
    let payer = BatchPayer::new(&validator.rpc_url, program).await;
    mint_to(&rpc, &authority, &mint, &payer.payer.pubkey(), 1_000_000).await;

    let seed: [u8; 32] = sponsor.to_bytes()[..32].try_into().expect("a seed");
    let backend = Arc::new(
        SolanaBatchSettlement::connect(
            &RpcTransport::direct(&validator.rpc_url).expect("transport"),
            &seed,
            mint,
            u64::from(GRACE),
            1,
        )
        .await
        .expect("connect"),
    );
    let channel = payer
        .open(
            &payer
                .admissible_open(&sponsor.pubkey(), &mint, DEPOSIT, GRACE)
                .await,
            &sponsor,
        )
        .await
        .expect("open");
    let vouchers: Arc<RwLock<Vec<HeldVoucher>>> = Arc::new(RwLock::new(vec![HeldVoucher {
        presentation: ChannelPresentation::Solana {
            channel: ChannelId(channel.to_string()),
        },
        voucher: Voucher {
            cumulative_amount: u128::from(VOUCHER),
            signature: payer.sign(&channel, VOUCHER).to_vec(),
        },
    }]));
    let watcher = SolanaBatchWatcher::new(backend, vouchers as Arc<dyn HeldVouchers>);

    payer.request_close(&channel).await.expect("request_close");
    let taken = watcher.tick(false).await.expect("tick");
    assert_eq!(
        steps_on(&taken, &channel),
        vec![Step::SettleAndSeal { amount: VOUCHER }]
    );
    let sealed = read(&rpc, &channel).await.expect("the channel");
    assert_eq!(
        (sealed.status, sealed.settled),
        (ChannelStatus::Sealed, VOUCHER)
    );

    Sealed {
        _validator: validator,
        rpc,
        sponsor,
        payer,
        mint,
        watcher,
        channel,
    }
}

#[tokio::test]
async fn a_distribute_that_pays_the_node_and_refunds_the_payer_reaches_distributed() {
    if !require_solana_test_validator() {
        return;
    }
    let Sealed {
        _validator,
        rpc,
        sponsor,
        payer,
        mint,
        watcher,
        channel,
    } = a_channel_sealed_below_its_deposit().await;

    let node_before = token_balance(&rpc, &sponsor.pubkey(), &mint).await;
    let payer_before = token_balance(&rpc, &payer.payer.pubkey(), &mint).await;
    let taken = watcher.tick(false).await.expect("tick");
    assert_eq!(steps_on(&taken, &channel), vec![Step::Distribute]);
    assert_eq!(
        token_balance(&rpc, &sponsor.pubkey(), &mint).await - node_before,
        VOUCHER,
        "this node's share: the voucher it landed"
    );
    assert_eq!(
        token_balance(&rpc, &payer.payer.pubkey(), &mint).await - payer_before,
        DEPOSIT - VOUCHER,
        "the payer's refund: what no voucher claimed"
    );
    assert_eq!(
        read(&rpc, &channel).await.map(|account| account.status),
        Some(ChannelStatus::Distributed)
    );
    assert!(watcher.stuck().is_empty());
}

/// The sponsor pays for `distribute`. With no SOL left it cannot, and the
/// channel stays Sealed: a real chain failure, pass after pass, until the
/// sponsor is funded again.
#[tokio::test]
async fn a_sealed_channel_whose_distribute_keeps_failing_is_reported_stuck() {
    if !require_solana_test_validator() {
        return;
    }
    let Sealed {
        _validator,
        rpc,
        sponsor,
        payer,
        watcher,
        channel,
        ..
    } = a_channel_sealed_below_its_deposit().await;

    // Empty the sponsor: every lamport but the transfer's own fee.
    let balance = rpc.get_balance(&sponsor.pubkey()).await.expect("balance");
    let drain = |lamports| {
        solana_sdk::system_instruction::transfer(&sponsor.pubkey(), &payer.payer.pubkey(), lamports)
    };
    let blockhash = rpc.get_latest_blockhash().await.expect("blockhash");
    let fee = rpc
        .get_fee_for_message(&solana_sdk::message::Message::new_with_blockhash(
            &[drain(balance)],
            Some(&sponsor.pubkey()),
            &blockhash,
        ))
        .await
        .expect("the transfer's fee");
    send(&rpc, &[drain(balance - fee)], &sponsor, &[])
        .await
        .expect("empty the sponsor");
    assert_eq!(
        rpc.get_balance(&sponsor.pubkey()).await.expect("balance"),
        0
    );

    for pass in 1..STUCK_AFTER_FAILED_PASSES {
        let taken = watcher.tick(false).await.expect("tick");
        assert!(steps_on(&taken, &channel).is_empty(), "distribute failed");
        assert!(
            watcher.stuck().is_empty(),
            "one failed pass ({pass}) is a retry, not yet a stuck channel"
        );
    }
    watcher.tick(false).await.expect("tick");
    let stuck = watcher.stuck();
    assert_eq!(stuck.len(), 1, "{stuck:?}");
    assert_eq!(stuck[0].channel, ChannelId(channel.to_string()));
    assert_eq!(stuck[0].failed_passes, STUCK_AFTER_FAILED_PASSES);
    assert!(!stuck[0].last_error.is_empty());
    assert_eq!(
        read(&rpc, &channel).await.map(|account| account.status),
        Some(ChannelStatus::Sealed)
    );

    // Funded again, the next pass distributes and the report clears.
    fund(&rpc, &sponsor.pubkey()).await;
    let taken = watcher.tick(false).await.expect("tick");
    assert_eq!(steps_on(&taken, &channel), vec![Step::Distribute]);
    assert!(watcher.stuck().is_empty());
    assert_eq!(
        read(&rpc, &channel).await.map(|account| account.status),
        Some(ChannelStatus::Distributed)
    );
}
