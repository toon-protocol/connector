//! ADR 0074 decision 5 on EVM, through a node built from a config file
//! (issue #1344): a client pays over an x402 `batch-settlement` channel, then
//! initiates a withdrawal of everything the chain says is unclaimed -- which
//! includes what it already paid in vouchers. The running node's own
//! watcher, started by `router` and nothing else, claims the latest voucher
//! before the withdrawal delay ends, so `finalizeWithdraw` returns only what
//! was never paid for. The gate then re-reads what the withdrawal left and
//! refuses a voucher it no longer backs.
//!
//! Nothing here calls `land`, a watcher or a sweep: the test pays, withdraws
//! and waits, as a client would.

mod support;

use std::io::Write;
use std::time::Duration;

use connector_domain::{Fulfill, Reject};
use connector_settlement::batch::EvmChannelConfig;
use connector_settlement_evm::test_support::x402::X402Chain;
use connector_settlement_evm::test_support::{require_anvil, Anvil, DEPLOYER_PRIVATE_KEY};
use connector_settlement_evm::EvmSettlementBackend;
use ethers::signers::{LocalWallet, Signer};

use support::{evm_voucher, paid_prepare, post_ilp, spawn_recording_app};

/// This binary's own base port for [`Anvil::spawn`], clear of every other
/// anvil binary's range.
const ANVIL_BASE_PORT: u16 = 22_700;

const ROUTE: &str = "g.toon.batch";
const PRICE: u64 = 100;
const DEPOSIT: u128 = 1_000;
const ONE_DAY: u64 = 86_400;

#[tokio::test(flavor = "multi_thread")]
async fn a_withdrawal_begun_by_the_payer_is_answered_by_the_node_claiming_its_latest_voucher() {
    if !require_anvil() {
        return;
    }
    let anvil = Anvil::spawn(ANVIL_BASE_PORT).await;
    let mut x402 = X402Chain::place(&anvil.rpc_url).await;
    let token = x402.deploy_fiat_token().await;
    let settlement = EvmSettlementBackend::deploy(&anvil.rpc_url, DEPLOYER_PRIVATE_KEY, token)
        .await
        .expect("a TokenNetwork over the FiatToken");
    let registry = settlement.registry_address();
    let node = settlement.own_address().to_fixed_bytes();
    drop(settlement);

    let payer = LocalWallet::from_bytes(&[0x81; 32]).expect("key");
    let session = LocalWallet::from_bytes(&[0x82; 32]).expect("key");
    x402.fund_gas(payer.address()).await;
    x402.fund_gas(session.address()).await;
    x402.mint(token, payer.address(), 1_000_000).await;

    let (app_addr, _recorded) = spawn_recording_app().await;
    let state_dir = tempfile::tempdir().expect("state dir");
    let mut signer_key = tempfile::NamedTempFile::new().expect("signer key");
    signer_key.write_all(&[0x07; 32]).expect("write");
    let mut settlement_key = tempfile::NamedTempFile::new().expect("settlement key");
    settlement_key
        .write_all(DEPLOYER_PRIVATE_KEY.as_bytes())
        .expect("write");
    let config = support::load_config(&format!(
        r#"
client_edge_addr = "127.0.0.1:0"
state_dir = "{state_dir}"

[signer]
key_file = "{signer_key}"

[settlement.evm]
rpc_url = "{rpc_url}"
contract_address = "{registry:?}"
token_address = "{token:?}"
decimals = 6

[settlement.evm.key]
key_file = "{settlement_key}"

[settlement.evm.batch_settlement]
asset_eip712_name = "USDC"
asset_eip712_version = "2"

[[routes]]
prefix = "{ROUTE}"
handler_url = "http://{app_addr}"
price = {PRICE}
"#,
        state_dir = state_dir.path().display(),
        signer_key = signer_key.path().display(),
        settlement_key = settlement_key.path().display(),
        rpc_url = anvil.rpc_url,
    ));
    let runtime = connector_cli::build(&config).await.expect("build");
    let app = connector_cli::router(&runtime, &config).expect("router");
    let receiver = runtime.signer.public_key().expect("the node's wrap key");

    let mut salt = [0u8; 32];
    salt[31] = 1;
    let channel_config = EvmChannelConfig {
        payer: payer.address().to_fixed_bytes(),
        payer_authorizer: session.address().to_fixed_bytes(),
        receiver: node,
        receiver_authorizer: node,
        token: token.to_fixed_bytes(),
        withdraw_delay: ONE_DAY,
        salt,
    };
    x402.deposit(&payer, &channel_config, DEPOSIT, 1).await;
    let channel = x402.channel_id(&channel_config);

    // Two paid writes: the node now holds a voucher for 200, and has landed
    // nothing.
    for (amount, config) in [(PRICE, Some(&channel_config)), (2 * PRICE, None)] {
        let signature = x402.sign_voucher(&session, &channel, u128::from(amount));
        let response = post_ilp(
            &app,
            &evm_voucher(&channel, amount, &signature, config),
            paid_prepare(ROUTE, &receiver),
        )
        .await;
        Fulfill::decode(&response)
            .unwrap_or_else(|_| panic!("paid for {amount}: {:?}", Reject::decode(&response)));
    }
    assert_eq!(x402.channel(&channel).await, (DEPOSIT, 0));

    // The payer withdraws everything the chain says is unclaimed.
    x402.initiate_withdraw(&session, &channel_config, DEPOSIT)
        .await;

    // The node answers on its own, well inside the day's delay: the chain's
    // clock never moves while this waits.
    let mut claimed = false;
    for _ in 0..200 {
        if x402.channel(&channel).await == (DEPOSIT, u128::from(2 * PRICE)) {
            claimed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        claimed,
        "the running node claimed its latest voucher after the payer began to withdraw"
    );

    // What the withdrawal left backs no new voucher, and the gate knows it:
    // it reads collateral from the chain, never from a cached deposit.
    let signature = x402.sign_voucher(&session, &channel, u128::from(3 * PRICE));
    let refused = Reject::decode(
        &post_ilp(
            &app,
            &evm_voucher(&channel, 3 * PRICE, &signature, None),
            paid_prepare(ROUTE, &receiver),
        )
        .await,
    )
    .expect("a voucher the withdrawal leaves unbacked is refused");
    assert!(
        refused.message.contains("more than the 200"),
        "refused as unbacked above the 200 claimed: {refused:?}"
    );

    // The delay passes; the payer takes back only what it never paid over.
    x402.advance_time(ONE_DAY).await;
    let before = x402.balance_of(token, payer.address()).await;
    x402.finalize_withdraw(&session, &channel_config).await;
    assert_eq!(
        x402.balance_of(token, payer.address()).await - before,
        DEPOSIT - u128::from(2 * PRICE)
    );
}
