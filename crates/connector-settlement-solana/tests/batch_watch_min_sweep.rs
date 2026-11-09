//! A table's `min_sweep_amount` against a real `payment-channels` in a
//! disposable validator (issue #1663): an Open channel whose held voucher is
//! below the minimum is not settled by the sweep, and once its payer calls
//! `request_close` the same voucher lands via `settle_and_seal`, minimum or
//! not -- every way a payer takes value back goes through an exit. The
//! operator's manual land ignores the minimum too.
//!
//! Its own test binary: `solana-test-validator` binds fixed ports, so one
//! validator per binary.

use std::str::FromStr;
use std::sync::{Arc, RwLock};

use connector_settlement::batch::{
    BatchSettlementBackend, ChannelPresentation, HeldVoucher, HeldVouchers, Voucher,
};
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

const GRACE: u32 = 900;
const DEPOSIT: u64 = 1_000;
const MINIMUM: u64 = 500;

async fn read(rpc: &RpcClient, channel: &Pubkey) -> wire::ChannelAccount {
    let account = rpc
        .get_account_with_commitment(channel, CommitmentConfig::confirmed())
        .await
        .expect("read")
        .value
        .expect("the channel exists");
    wire::ChannelAccount::parse(&account.data).expect("a channel account")
}

fn steps_on(taken: &[(ChannelId, Step)], channel: &Pubkey) -> Vec<Step> {
    taken
        .iter()
        .filter(|(id, _)| id.0 == channel.to_string())
        .map(|(_, step)| *step)
        .collect()
}

#[tokio::test]
async fn an_open_channel_below_the_minimum_waits_and_an_exit_lands_it_anyway() {
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
    let program = Pubkey::from_str(PAYMENT_CHANNELS_PROGRAM_ID).expect("base58");
    let payer = BatchPayer::new(&validator.rpc_url, program).await;
    mint_to(&rpc, &authority, &mint, &payer.payer.pubkey(), 1_000_000).await;

    let seed: [u8; 32] = sponsor.to_bytes()[..32].try_into().expect("a seed");
    let backend = Arc::new(
        SolanaBatchSettlement::connect(
            &RpcTransport::direct(&validator.rpc_url).expect("transport"),
            &seed,
            mint,
            6,
            u64::from(GRACE),
            1,
        )
        .await
        .expect("connect")
        .with_min_sweep_amount(MINIMUM),
    );
    let vouchers = Arc::new(RwLock::new(Vec::new()));
    let watcher = SolanaBatchWatcher::new(
        Arc::clone(&backend),
        Arc::clone(&vouchers) as Arc<dyn HeldVouchers>,
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
    let amount = MINIMUM - 1;
    *vouchers.write().unwrap() = vec![HeldVoucher {
        presentation: ChannelPresentation::Solana {
            channel: ChannelId(channel.to_string()),
        },
        voucher: Voucher {
            cumulative_amount: u128::from(amount),
            signature: payer.sign(&channel, amount).to_vec(),
        },
    }];

    // The sweep is due, and the voucher is below the minimum: nothing sent.
    let taken = watcher.tick(true).await.expect("tick");
    assert!(steps_on(&taken, &channel).is_empty());
    assert_eq!(read(&rpc, &channel).await.settled, 0);

    // The operator's manual land of another Open channel below the
    // minimum lands it: asking is the decision that it is worth the fee.
    let landed = payer
        .open(
            &payer
                .admissible_open(&sponsor.pubkey(), &mint, DEPOSIT, GRACE)
                .await,
            &sponsor,
        )
        .await
        .expect("open");
    backend
        .admit(ChannelPresentation::Solana {
            channel: ChannelId(landed.to_string()),
        })
        .await
        .expect("admit");
    backend
        .land(
            &ChannelId(landed.to_string()),
            Voucher {
                cumulative_amount: u128::from(amount),
                signature: payer.sign(&landed, amount).to_vec(),
            },
        )
        .await
        .expect("a manual land ignores the minimum");
    let manual = read(&rpc, &landed).await;
    assert_eq!(manual.status, ChannelStatus::Open);
    assert_eq!(manual.settled, amount);

    // The payer asks to close; the voucher lands, whatever the minimum.
    payer.request_close(&channel).await.expect("request_close");
    let taken = watcher.tick(false).await.expect("tick");
    assert_eq!(
        steps_on(&taken, &channel),
        vec![Step::SettleAndSeal { amount }]
    );
    let sealed = read(&rpc, &channel).await;
    assert_eq!(sealed.status, ChannelStatus::Sealed);
    assert_eq!(sealed.settled, amount);
}
