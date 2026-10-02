//! A read that follows a transaction this node sent and saw confirm is taken
//! at that transaction's block (issue #1447). Behind a load-balanced endpoint
//! `latest` can be answered by a backend that has not imported the block, and
//! for everything but an open the stale answer looks valid.
//!
//! The subject is the payer's own endpoint: a `FakeRpc` in front of anvil
//! records every call and, once armed, answers a call at a block number as a
//! backend that does not have the block yet.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use connector_chain_rpc::{FakeRpc, RpcReply};
use connector_settlement::batch::{
    BatchChannelStatus, BatchSettlementBackend, BatchSettlementError, BatchSettlementPayer,
    ChannelPresentation, EvmReceiverTerms, ReceiverTerms,
};
use connector_settlement_evm::test_support::x402::X402Chain;
use connector_settlement_evm::test_support::{
    require_anvil, Anvil, COUNTERPARTY_PRIVATE_KEY, DEPLOYER_PRIVATE_KEY,
};
use connector_settlement_evm::{EvmBatchSettlementBackend, RpcTransport};
use ethers::providers::{Http, Middleware, Provider};
use ethers::signers::{LocalWallet, Signer};
use ethers::types::Address;

const ANVIL_BASE_PORT: u16 = 24_300;
const ONE_DAY: u64 = 86_400;
const FUNDED: u128 = 1_000_000;
/// "Never": more refusals than any bound retries.
const NEVER: usize = usize::MAX;

struct Lagging {
    anvil: Anvil,
    x402: X402Chain,
    token: Address,
    /// How many more reads at a block number are answered as a backend that
    /// lacks the block.
    lacking: Arc<AtomicUsize>,
    endpoint: FakeRpc,
    payer: EvmBatchSettlementBackend,
    /// The receiver, over the same endpoint, for the claim.
    receiver: EvmBatchSettlementBackend,
}

fn is_pinned(params: &serde_json::Value) -> bool {
    params
        .get(1)
        .and_then(|block| block.as_str())
        .is_some_and(|block| block.starts_with("0x"))
}

async fn node(url: &str, key: &str, token: Address) -> EvmBatchSettlementBackend {
    EvmBatchSettlementBackend::connect(
        &RpcTransport::direct(url).expect("transport"),
        key,
        token,
        6,
        ONE_DAY,
    )
    .await
    .expect("a node")
}

impl Lagging {
    async fn spawn() -> Lagging {
        let anvil = Anvil::spawn(ANVIL_BASE_PORT).await;
        let mut x402 = X402Chain::place(&anvil.rpc_url).await;
        let token = x402.deploy_fiat_token().await;
        let payer_address = LocalWallet::from_bytes(&hex32(DEPLOYER_PRIVATE_KEY))
            .expect("key")
            .address();
        x402.mint(token, payer_address, FUNDED).await;
        let lacking = Arc::new(AtomicUsize::new(0));
        let script_lacking = Arc::clone(&lacking);
        let endpoint = FakeRpc::spawn_in_front_of(&anvil.rpc_url, move |call| {
            if call.method == "eth_call"
                && is_pinned(&call.params)
                && script_lacking
                    .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                        (left > 0).then(|| if left == NEVER { left } else { left - 1 })
                    })
                    .is_ok()
            {
                return RpcReply::Error {
                    code: -32000,
                    message: "header not found".to_string(),
                };
            }
            RpcReply::Forward
        })
        .await;
        let payer = node(&endpoint.url(), DEPLOYER_PRIVATE_KEY, token).await;
        let receiver = node(&endpoint.url(), COUNTERPARTY_PRIVATE_KEY, token).await;
        Lagging {
            anvil,
            x402,
            token,
            lacking,
            endpoint,
            payer,
            receiver,
        }
    }

    fn terms(&self) -> ReceiverTerms {
        ReceiverTerms::Evm(EvmReceiverTerms {
            receiver: self.receiver.own_address().to_fixed_bytes(),
            token: self.token.to_fixed_bytes(),
            min_withdraw_delay_secs: ONE_DAY,
        })
    }

    /// From now on, the next `reads` reads at a block number are refused.
    fn lack_the_block(&self, reads: usize) {
        self.lacking.store(reads, Ordering::SeqCst);
    }

    /// The block the chain is at: after a write with nothing mined behind
    /// it, the block its receipt names.
    async fn head(&self) -> u64 {
        Provider::<Http>::try_from(self.anvil.rpc_url.as_str())
            .expect("provider")
            .get_block_number()
            .await
            .expect("block number")
            .as_u64()
    }

    /// The block numbers every `eth_call` at one was asked for, in order.
    fn pinned_blocks(&self) -> Vec<u64> {
        self.endpoint
            .calls()
            .iter()
            .filter(|call| call.method == "eth_call" && is_pinned(&call.params))
            .map(|call| {
                u64::from_str_radix(
                    call.params[1].as_str().unwrap().trim_start_matches("0x"),
                    16,
                )
                .expect("hex block")
            })
            .collect()
    }

    async fn payer_balance(&self) -> u128 {
        self.x402
            .balance_of(self.token, self.payer.own_address())
            .await
    }
}

fn hex32(text: &str) -> [u8; 32] {
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[2 * i..2 * i + 2], 16).expect("hex");
    }
    out
}

/// After each confirmed write the read that follows names the receipt's block.
#[tokio::test]
async fn every_read_after_a_confirmed_write_is_at_its_block() {
    if !require_anvil() {
        return;
    }
    let lagging = Lagging::spawn().await;

    let opened = lagging
        .payer
        .open(lagging.terms(), 1_000)
        .await
        .expect("open");
    let channel = opened.presentation.channel().clone();
    assert_eq!(lagging.pinned_blocks(), vec![lagging.head().await], "open");

    lagging.payer.top_up(&channel, 500).await.expect("top up");
    assert_eq!(
        lagging.pinned_blocks().last().copied(),
        Some(lagging.head().await),
        "top_up"
    );

    // A claim: the receiver lands a voucher.
    let voucher = lagging
        .payer
        .sign_voucher(&channel, 300)
        .await
        .expect("sign");
    lagging
        .receiver
        .admit(opened.presentation.clone())
        .await
        .expect("admit");
    lagging
        .receiver
        .land(&channel, voucher)
        .await
        .expect("claim");
    assert_eq!(
        lagging.pinned_blocks().last().copied(),
        Some(lagging.head().await),
        "claim"
    );

    lagging
        .payer
        .start_withdrawal(&channel)
        .await
        .expect("start");
    assert_eq!(
        lagging.pinned_blocks().last().copied(),
        Some(lagging.head().await),
        "initiateWithdraw"
    );

    lagging.x402.advance_time(ONE_DAY).await;
    lagging
        .payer
        .finish_withdrawal(&channel)
        .await
        .expect("finish");
    let head = lagging.head().await;
    assert_eq!(
        lagging.pinned_blocks().last().copied(),
        Some(head),
        "finalizeWithdraw"
    );
}

/// A backend without the block, once: the open still succeeds, and the
/// deposit backs a voucher.
#[tokio::test]
async fn an_open_survives_an_endpoint_that_lacks_the_block_once() {
    if !require_anvil() {
        return;
    }
    let lagging = Lagging::spawn().await;
    lagging.lack_the_block(1);
    let opened = lagging
        .payer
        .open(lagging.terms(), 1_000)
        .await
        .expect("open");
    let channel = opened.presentation.channel().clone();
    assert!(lagging.pinned_blocks().len() >= 2, "the read was repeated");
    lagging
        .payer
        .sign_voucher(&channel, 1_000)
        .await
        .expect("the deposit backs a voucher");
}

#[tokio::test]
async fn a_top_up_survives_an_endpoint_that_lacks_the_block_once() {
    if !require_anvil() {
        return;
    }
    let lagging = Lagging::spawn().await;
    let opened = lagging
        .payer
        .open(lagging.terms(), 1_000)
        .await
        .expect("open");
    let channel = opened.presentation.channel().clone();
    lagging.lack_the_block(1);
    let state = lagging.payer.top_up(&channel, 500).await.expect("top up");
    assert_eq!(
        state.on_chain.collateral, 1_500,
        "the answer has the increment"
    );
    lagging
        .payer
        .sign_voucher(&channel, 1_500)
        .await
        .expect("the backing includes the increment");
}

#[tokio::test]
async fn a_start_withdrawal_survives_an_endpoint_that_lacks_the_block_once() {
    if !require_anvil() {
        return;
    }
    let lagging = Lagging::spawn().await;
    let opened = lagging
        .payer
        .open(lagging.terms(), 1_000)
        .await
        .expect("open");
    let channel = opened.presentation.channel().clone();
    lagging.lack_the_block(1);
    let state = lagging
        .payer
        .start_withdrawal(&channel)
        .await
        .expect("start");
    assert_eq!(state.on_chain.status, BatchChannelStatus::Withdrawing);
    let refused = lagging
        .payer
        .sign_voucher(&channel, 1)
        .await
        .expect_err("a voucher above what is landed is refused");
    assert!(
        matches!(refused, BatchSettlementError::VoucherUnbacked { .. }),
        "{refused:?}"
    );
}

/// An endpoint that never has the block: the open fails within the bound,
/// says what happened, and opening the same record again over a healthy
/// endpoint adopts the channel without a second deposit.
#[tokio::test]
async fn an_open_whose_block_never_arrives_is_kept_and_adopted_on_repeat() {
    if !require_anvil() {
        return;
    }
    let lagging = Lagging::spawn().await;
    let before = lagging.payer_balance().await;
    let record = lagging
        .payer
        .prepare_open(lagging.terms(), 1_000)
        .await
        .expect("prepare");
    lagging.lack_the_block(NEVER);
    let started = std::time::Instant::now();
    let error = lagging
        .payer
        .open_prepared(&record)
        .await
        .expect_err("the result cannot be read");
    assert!(started.elapsed() < std::time::Duration::from_secs(30));
    let BatchSettlementError::Backend(message) = &error else {
        panic!("{error:?}");
    };
    assert!(message.contains("confirmed"), "{message}");
    assert!(
        message.contains("keeps the channel as its own"),
        "{message}"
    );
    assert!(
        message.contains("opening the same record again adopts it"),
        "{message}"
    );
    assert_eq!(lagging.payer_balance().await, before - 1_000);

    let healthy = node(&lagging.anvil.rpc_url, DEPLOYER_PRIVATE_KEY, lagging.token).await;
    let opened = healthy.open_prepared(&record).await.expect("adopt");
    assert_eq!(opened.presentation, record.presentation());
    assert_eq!(
        lagging.payer_balance().await,
        before - 1_000,
        "no second deposit"
    );
}

/// After `initiateWithdraw` confirms and its block never arrives, nothing is
/// recorded from an older reading: the backing stays at zero.
#[tokio::test]
async fn a_start_withdrawal_whose_block_never_arrives_backs_nothing() {
    if !require_anvil() {
        return;
    }
    let lagging = Lagging::spawn().await;
    let opened = lagging
        .payer
        .open(lagging.terms(), 1_000)
        .await
        .expect("open");
    let channel = opened.presentation.channel().clone();
    lagging.lack_the_block(NEVER);
    let error = lagging
        .payer
        .start_withdrawal(&channel)
        .await
        .expect_err("the result cannot be read");
    let BatchSettlementError::Backend(message) = &error else {
        panic!("{error:?}");
    };
    assert!(message.contains("confirmed"), "{message}");
    assert!(message.contains("could not be read"), "{message}");
    for amount in [1, 500, 1_000] {
        let refused = lagging
            .payer
            .sign_voucher(&channel, amount)
            .await
            .expect_err("nothing is backed");
        assert!(
            matches!(
                refused,
                BatchSettlementError::VoucherUnbacked { backed: 0, .. }
            ),
            "{refused:?}"
        );
    }
    assert!(matches!(
        opened.presentation,
        ChannelPresentation::Evm { .. }
    ));
}
