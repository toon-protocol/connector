//! Issue #631's security review, finding 1 (mint binding): the deployed
//! `packages/solana-program` lets any payer open a channel with ANY SPL
//! mint -- its channel PDA is seeded per (pair, mint) -- and the Ed25519
//! balance proof does not cover the mint
//! (`connector_signer::solana_balance_proof_message` signs channel
//! account, nonce and amount alone). So the ONE place a chain-resolved,
//! undeclared channel is bound to the mint this node actually settles in
//! is `SolanaSettlementBackend::channel_counterparty`: a channel on any
//! other mint must resolve to `Ok(None)` (unknown channel), or a claim on
//! a channel funded with a worthless token would buy USDC-priced writes.
//!
//! Driven against a real validator: the wrong-mint channel here is
//! genuinely opened and funded on chain, and a validly-signed claim for it
//! exists -- the refusal under test is purely the resolution's mint check,
//! not a missing channel or a bad signature.

use std::str::FromStr;

use chrono::Duration;
use connector_settlement::SettlementBackend;
use connector_settlement_solana::SolanaSettlementBackend;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{Keypair, Signer};

use connector_settlement_solana::test_support::{
    require_solana_test_validator, SolanaValidator, LOCAL_TEST_PROGRAM_ID,
};

#[tokio::test]
async fn a_funded_channel_on_a_different_mint_resolves_as_unknown() {
    if !require_solana_test_validator() {
        return;
    }

    let validator = SolanaValidator::spawn().await;
    let program_id = Pubkey::from_str(LOCAL_TEST_PROGRAM_ID).expect("valid local test program id");

    // The attacker's shape: a real, open, funded channel -- on a mint that
    // is NOT the one this node settles in. `deploy` builds it end to end:
    // fresh mint, this identity as participant, held counterparty key.
    let opener = SolanaSettlementBackend::deploy(&validator.rpc_url, program_id)
        .await
        .expect("bind to the genesis-loaded payment-channel program");
    let junk_mint = opener.token_mint();
    let counterparty = opener
        .test_counterparty_pubkey()
        .expect("deploy() holds a counterparty key");
    let channel = opener
        .open(counterparty.clone(), Duration::hours(1))
        .await
        .expect("open a channel on the junk mint");
    opener
        .test_fund_counterparty(&channel, 1_000)
        .await
        .expect("fund the junk-mint channel with a real on-chain deposit");
    // A validly-signed claim on that channel genuinely exists -- the
    // refusal below is not for want of one.
    assert!(
        opener.test_sign_claim(&channel, 1, 100).is_some(),
        "the held counterparty key signs a real balance proof for this channel"
    );
    let channel_pubkey = Pubkey::from_str(&channel.0).expect("a channel id is a base58 pubkey");

    // The node under test: the SAME on-chain identity, settling in a
    // DIFFERENT mint (any other real SPL mint -- a second `deploy`'s).
    let other = SolanaSettlementBackend::deploy(&validator.rpc_url, program_id)
        .await
        .expect("deploy a second backend for its fresh mint");
    let configured_mint = other.token_mint();
    assert_ne!(junk_mint, configured_mint);
    let node = SolanaSettlementBackend::connect(
        &connector_settlement_solana::RpcTransport::direct(&validator.rpc_url)
            .expect("rpc transport"),
        &opener.test_payer_seed(),
        program_id,
        configured_mint,
        6,
    )
    .await
    .expect("connect under the opener's identity, bound to the configured mint");

    assert_eq!(
        node.channel_counterparty(channel_pubkey)
            .await
            .expect("the lookup itself succeeds"),
        None,
        "a channel on any mint but the configured one is an unknown channel, \
         however real and well-funded it is"
    );

    // Control: the identical channel resolves fine for a backend actually
    // configured with its mint -- proving the refusal above was the mint
    // binding and nothing else.
    let same_mint_node = SolanaSettlementBackend::connect(
        &connector_settlement_solana::RpcTransport::direct(&validator.rpc_url)
            .expect("rpc transport"),
        &opener.test_payer_seed(),
        program_id,
        junk_mint,
        6,
    )
    .await
    .expect("connect under the opener's identity, bound to the channel's own mint");
    assert_eq!(
        same_mint_node
            .channel_counterparty(channel_pubkey)
            .await
            .expect("the lookup itself succeeds"),
        Some(Pubkey::try_from(counterparty.as_slice()).expect("32-byte pubkey")),
        "the same channel resolves to its counterparty when the mint matches"
    );

    // The remaining chain-level refusals (finding 2): an account that
    // exists but is owned by the system program, not the payment-channel
    // program...
    assert_eq!(
        node.channel_counterparty(node.own_pubkey())
            .await
            .expect("the lookup itself succeeds"),
        None,
        "a real account the program does not own is an unknown channel"
    );
    // ...and an address nothing lives at.
    assert_eq!(
        node.channel_counterparty(Keypair::new().pubkey())
            .await
            .expect("the lookup itself succeeds"),
        None,
        "an address nothing was ever opened at is an unknown channel"
    );
}
