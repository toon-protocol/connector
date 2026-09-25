//! Chain-specific behavior the port's own contract suite deliberately
//! doesn't exercise, since it is specific to *how* a real backend talks to
//! a chain rather than to the port's channel-lifecycle rules: real gas
//! estimation, and nonce safety under concurrent calls against the same
//! backend (every `SettlementBackend` method takes `&self`, so nothing at
//! the port level stops two calls racing).

mod support;

use std::sync::Arc;

use chrono::Duration;
use connector_settlement::{Claim, SettlementBackend};
use connector_settlement_evm::EvmSettlementBackend;
use connector_signer::{derive_evm_address, evm_balance_proof_digest, EvmBalanceProof};
use ethers::signers::Signer as EvmSigner;
use libsecp256k1::{PublicKey, SecretKey};

use support::{
    channel_id_bytes, require_anvil, sign_evm, Anvil, ANVIL_CHAIN_ID, DEPLOYER_PRIVATE_KEY,
};

/// `open`, `fund`, `redeem` and `close` are all real transactions against
/// a real chain with no manually-specified gas limit anywhere in
/// `EvmSettlementBackend` -- every one of them only succeeds if ethers'
/// automatic `eth_estimateGas` round trip against the deployed contract
/// produced a workable limit. A wrong or missing estimate would surface
/// here as an "out of gas" revert or a `SettlementError::Backend` from a
/// failed `eth_estimateGas` call, not a hang.
#[tokio::test]
async fn every_channel_operation_estimates_its_own_gas_and_succeeds() {
    if !require_anvil() {
        return;
    }

    let anvil = Anvil::spawn().await;
    let token =
        EvmSettlementBackend::deploy_mock_token(&anvil.rpc_url, DEPLOYER_PRIVATE_KEY, 1_000_000)
            .await
            .expect("deploy mock USDC");
    let backend = EvmSettlementBackend::deploy(&anvil.rpc_url, DEPLOYER_PRIVATE_KEY, token)
        .await
        .expect("deploy a TokenNetwork through a fresh registry");
    let token_network_address = backend.address().to_fixed_bytes();

    let counterparty_secret = SecretKey::parse(&[3u8; 32]).expect("valid secret key");
    let counterparty_public = PublicKey::from_secret_key(&counterparty_secret);
    let counterparty = derive_evm_address(&counterparty_public.serialize()).to_vec();

    let channel = backend
        .open(counterparty, Duration::seconds(3600))
        .await
        .expect("open");
    // `fund` is a self-deposit (issue #1118): it credits this backend's
    // own participant slot, not the counterparty's.
    let state = backend.fund(&channel, 1_000).await.expect("fund");
    assert_eq!(state.own_deposited, 1_000);
    assert_eq!(state.counterparty_deposited, 0);

    // What the claim below is drawn from is the counterparty's own
    // deposit, which the fixture-only delegate deposit stands in for here
    // -- on a real deployment the counterparty makes it themselves.
    let state = backend
        .fund_counterparty(&channel, 1_000)
        .await
        .expect("fund the counterparty's side");
    assert_eq!(state.counterparty_deposited, 1_000);

    let proof = EvmBalanceProof {
        channel_id: channel_id_bytes(&channel.0),
        nonce: 1,
        transferred_amount: 400,
        locked_amount: 0,
        locks_root: [0u8; 32],
        chain_id: ANVIL_CHAIN_ID,
        token_network_address,
    };
    let state = backend
        .redeem(
            &channel,
            Claim {
                nonce: 1,
                cumulative_amount: 400,
                signature: sign_evm(&counterparty_secret, &evm_balance_proof_digest(&proof)),
            },
        )
        .await
        .expect("redeem");
    assert_eq!(state.redeemed, 400);

    let state = backend.close(&channel).await.expect("close");
    assert_eq!(state.status, connector_settlement::ChannelStatus::Closed);
}

/// Two channels, funded concurrently from the *same* signer through the
/// *same* `EvmSettlementBackend` (shared behind one `Arc`, called via
/// `&self` from two tasks with no `.await` between them) -- a real race
/// for "what is my next nonce", which a naive client (each call
/// independently asking the node for its pending nonce) can lose: both
/// reads can observe the same pending count before either transaction is
/// broadcast, so both would submit the same nonce and one would be
/// rejected or silently replace the other. This only both land, each with
/// their own funded amount, because `EvmSettlementBackend`'s sender holds
/// one lock across choosing a nonce, signing and sending, and counts nonces
/// locally from one `pending` read rather than re-deriving each one from a
/// racy on-chain read (ADR 0073).
#[tokio::test]
async fn concurrent_calls_from_the_same_signer_do_not_conflict_on_nonce() {
    if !require_anvil() {
        return;
    }

    let anvil = Anvil::spawn().await;
    let token =
        EvmSettlementBackend::deploy_mock_token(&anvil.rpc_url, DEPLOYER_PRIVATE_KEY, 1_000_000)
            .await
            .expect("deploy mock USDC");
    let backend = Arc::new(
        EvmSettlementBackend::deploy(&anvil.rpc_url, DEPLOYER_PRIVATE_KEY, token)
            .await
            .expect("deploy a TokenNetwork through a fresh registry"),
    );

    let peer_one = ethers::signers::LocalWallet::new(&mut ethers::core::rand::thread_rng())
        .address()
        .as_bytes()
        .to_vec();
    let peer_two = ethers::signers::LocalWallet::new(&mut ethers::core::rand::thread_rng())
        .address()
        .as_bytes()
        .to_vec();

    let first = backend
        .open(peer_one, Duration::seconds(3600))
        .await
        .expect("open first channel");
    let second = backend
        .open(peer_two, Duration::seconds(3600))
        .await
        .expect("open second channel");

    let backend_a = Arc::clone(&backend);
    let first_a = first.clone();
    let backend_b = Arc::clone(&backend);
    let second_b = second.clone();

    let (result_a, result_b) = tokio::join!(
        async move { backend_a.fund(&first_a, 111).await },
        async move { backend_b.fund(&second_b, 222).await },
    );

    let state_a = result_a.expect("concurrent fund of the first channel");
    let state_b = result_b.expect("concurrent fund of the second channel");
    assert_eq!(state_a.own_deposited, 111);
    assert_eq!(state_b.own_deposited, 222);

    // A third, immediately-following call proves the nonce sequence is
    // still consistent afterward too -- a manager that had desynced from
    // the two concurrent sends above would misfire here, not just during
    // the race itself.
    let state_a_again = backend.fund(&first, 50).await.expect("fund again");
    assert_eq!(state_a_again.own_deposited, 161);
}
