//! `EvmBatchWatcher` (ADR 0074 decision 5, issue #1344) against x402's real
//! `x402BatchSettlement` on a disposable `anvil`: a payer's
//! `initiateWithdraw` is answered by a `claim` of the latest voucher before
//! the delay ends, and the sweep claims many channels in one transaction and
//! then settles.
//!
//! The watcher is driven one step at a time here, so each assertion is about
//! one step; `connector-cli`'s end-to-end test runs it as the node does.

use std::sync::{Arc, RwLock};

use connector_settlement::batch::{
    BatchChannelStatus, BatchSettlementBackend, ChannelPresentation, EvmChannelConfig, HeldVoucher,
    HeldVouchers, Voucher,
};
use connector_settlement_evm::test_support::x402::{batch_settlement_address, X402Chain};
use connector_settlement_evm::test_support::{require_anvil, Anvil, DEPLOYER_PRIVATE_KEY};
use connector_settlement_evm::{
    Claimed, EvmBatchSettlementBackend, EvmBatchWatcher, EvmSettlementBackend,
};
use ethers::signers::{LocalWallet, Signer};
use ethers::types::Address;

/// This binary's own base port for [`Anvil::spawn`], clear of every other
/// anvil binary's range.
const ANVIL_BASE_PORT: u16 = 22_500;

const ONE_DAY: u64 = 86_400;

struct Chain {
    _anvil: Anvil,
    x402: X402Chain,
    backend: Arc<EvmBatchSettlementBackend>,
    token: Address,
    payer: LocalWallet,
    session: LocalWallet,
    held: Arc<RwLock<Vec<HeldVoucher>>>,
    next_salt: u64,
}

impl Chain {
    async fn spawn() -> Chain {
        let anvil = Anvil::spawn(ANVIL_BASE_PORT).await;
        let mut x402 = X402Chain::place(&anvil.rpc_url).await;
        let token = x402.deploy_fiat_token().await;
        let settlement = EvmSettlementBackend::deploy(&anvil.rpc_url, DEPLOYER_PRIVATE_KEY, token)
            .await
            .expect("this node's settlement backend");
        let backend = settlement
            .batch_settlement(batch_settlement_address(), ONE_DAY)
            .await
            .expect("bound to x402BatchSettlement");
        let payer = LocalWallet::from_bytes(&[0x71; 32]).expect("key");
        let session = LocalWallet::from_bytes(&[0x72; 32]).expect("key");
        x402.fund_gas(payer.address()).await;
        x402.fund_gas(session.address()).await;
        x402.mint(token, payer.address(), 1_000_000).await;
        Chain {
            _anvil: anvil,
            x402,
            backend: Arc::new(backend),
            token,
            payer,
            session,
            held: Arc::new(RwLock::new(Vec::new())),
            next_salt: 1,
        }
    }

    fn node(&self) -> Address {
        self.backend.own_address()
    }

    /// A channel paying this node, opened with `deposit`.
    async fn open(&mut self, deposit: u128) -> EvmChannelConfig {
        let mut salt = [0u8; 32];
        salt[24..].copy_from_slice(&self.next_salt.to_be_bytes());
        self.next_salt += 1;
        let config = EvmChannelConfig {
            payer: self.payer.address().to_fixed_bytes(),
            payer_authorizer: self.session.address().to_fixed_bytes(),
            receiver: self.node().to_fixed_bytes(),
            receiver_authorizer: self.node().to_fixed_bytes(),
            token: self.token.to_fixed_bytes(),
            withdraw_delay: ONE_DAY,
            salt,
        };
        self.x402
            .deposit(&self.payer, &config, deposit, self.next_salt)
            .await;
        self.next_salt += 1;
        config
    }

    /// The client pays: its session key signs a voucher for `amount`, and
    /// this node now holds it, as the claim gate would after accepting it.
    fn pay(&self, config: &EvmChannelConfig, amount: u128) {
        let channel = self.x402.channel_id(config);
        let voucher = Voucher {
            cumulative_amount: amount,
            signature: self.x402.sign_voucher(&self.session, &channel, amount),
        };
        let mut held = self.held.write().unwrap();
        held.retain(|entry| entry.presentation.channel() != &channel);
        held.push(HeldVoucher {
            presentation: ChannelPresentation::Evm {
                channel,
                config: config.clone(),
            },
            voucher,
        });
    }

    fn watcher(&self) -> EvmBatchWatcher {
        EvmBatchWatcher::new(
            Arc::clone(&self.backend),
            Arc::clone(&self.held) as Arc<dyn HeldVouchers>,
        )
    }
}

/// ADR 0074 decision 5's EVM race: a payer initiates a withdrawal of
/// everything the chain says is unclaimed, which includes what it already
/// paid this node in a voucher. The watcher sees the `WithdrawInitiated` and
/// claims that voucher before the delay passes, so `finalizeWithdraw` can
/// take only what was never paid for. A channel this node holds no voucher
/// on is left alone.
#[tokio::test]
async fn a_withdrawal_is_answered_by_claiming_the_latest_voucher_before_it_can_finalize() {
    if !require_anvil() {
        return;
    }
    let mut chain = Chain::spawn().await;
    let paid = chain.open(1_000).await;
    let unpaid = chain.open(1_000).await;
    chain.pay(&paid, 100);
    chain.pay(&paid, 300);
    let paid_id = chain.x402.channel_id(&paid);
    let unpaid_id = chain.x402.channel_id(&unpaid);

    let mut watcher = chain.watcher();
    assert!(
        watcher.watch_once().await.expect("first watch").is_empty(),
        "the first watch only starts the cursor"
    );
    let node_nonce = chain.x402.nonce(chain.node()).await;

    // The payer tries to take back everything the chain has not recorded.
    chain
        .x402
        .initiate_withdraw(&chain.session, &paid, 1_000)
        .await;
    chain
        .x402
        .initiate_withdraw(&chain.session, &unpaid, 1_000)
        .await;

    let claimed = watcher.watch_once().await.expect("watch");
    assert_eq!(
        claimed,
        vec![Claimed {
            channel: paid_id.clone(),
            total_claimed: 300,
        }],
        "the latest voucher on the withdrawing channel, and nothing on the one it holds none on"
    );
    assert_eq!(chain.x402.channel(&paid_id).await, (1_000, 300));
    assert_eq!(chain.x402.channel(&unpaid_id).await, (1_000, 0));
    assert_eq!(
        chain.x402.nonce(chain.node()).await,
        node_nonce + 1,
        "one claim, sent at once"
    );
    let state = chain.backend.channel_state(&paid_id).await.expect("read");
    assert_eq!(state.status, BatchChannelStatus::Withdrawing);
    assert_eq!(state.landed, 300);
    assert_eq!(
        state.collateral, 0,
        "re-read: the withdrawal leaves nothing to back a new voucher"
    );

    // The watch is idempotent: no new log, nothing sent.
    assert!(watcher.watch_once().await.expect("watch").is_empty());
    assert_eq!(chain.x402.nonce(chain.node()).await, node_nonce + 1);

    // The delay passes and the payer finalizes: it gets only what it never
    // paid over, because the voucher was claimed in time.
    chain.x402.advance_time(ONE_DAY).await;
    let before = chain
        .x402
        .balance_of(chain.token, chain.payer.address())
        .await;
    chain.x402.finalize_withdraw(&chain.session, &paid).await;
    let refunded = chain
        .x402
        .balance_of(chain.token, chain.payer.address())
        .await
        - before;
    assert_eq!(refunded, 700, "the claimed 300 stayed in the channel");
    assert_eq!(chain.x402.channel(&paid_id).await, (300, 300));
}

/// The sweep claims every held voucher the chain has not recorded -- many
/// channels, one `claim` -- skips one already recorded, and then settles
/// everything claimed to this node's address in one `settle`.
#[tokio::test]
async fn the_sweep_claims_every_channel_in_one_transaction_and_then_settles() {
    if !require_anvil() {
        return;
    }
    let mut chain = Chain::spawn().await;
    let first = chain.open(1_000).await;
    let second = chain.open(1_000).await;
    let third = chain.open(1_000).await;
    chain.pay(&first, 100);
    chain.pay(&second, 200);
    chain.pay(&third, 50);
    // The third channel's voucher is already landed.
    let third_id = chain.x402.channel_id(&third);
    let held = chain.held.read().unwrap().clone();
    let third_voucher = held
        .iter()
        .find(|entry| entry.presentation.channel() == &third_id)
        .expect("held")
        .clone();
    chain
        .backend
        .admit(third_voucher.presentation.clone())
        .await
        .expect("admit");
    chain
        .backend
        .land(&third_id, third_voucher.voucher)
        .await
        .expect("land");

    let node_nonce = chain.x402.nonce(chain.node()).await;
    let node_balance = chain.x402.balance_of(chain.token, chain.node()).await;
    let report = chain.watcher().sweep_once().await.expect("sweep");
    let mut claimed = report.claimed.clone();
    claimed.sort_by(|a, b| a.channel.0.cmp(&b.channel.0));
    let mut expected = vec![
        Claimed {
            channel: chain.x402.channel_id(&first),
            total_claimed: 100,
        },
        Claimed {
            channel: chain.x402.channel_id(&second),
            total_claimed: 200,
        },
    ];
    expected.sort_by(|a, b| a.channel.0.cmp(&b.channel.0));
    assert_eq!(claimed, expected);
    assert_eq!(
        report.settled, 350,
        "everything claimed, the third included"
    );
    assert_eq!(
        chain.x402.nonce(chain.node()).await,
        node_nonce + 2,
        "one claim for both channels, then one settle"
    );
    assert_eq!(
        chain.x402.balance_of(chain.token, chain.node()).await - node_balance,
        350
    );

    // Nothing left to do: a second sweep sends nothing.
    let report = chain.watcher().sweep_once().await.expect("sweep");
    assert!(report.claimed.is_empty());
    assert_eq!(report.settled, 0);
    assert_eq!(chain.x402.nonce(chain.node()).await, node_nonce + 2);
}
