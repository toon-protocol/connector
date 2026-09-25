//! `SolanaSettlementBackend` held to the `SettlementBackend` port's
//! contract suite, unmodified, against a real `packages/solana-program`
//! instance loaded into a real (if disposable) validator's genesis -- see
//! `connector_settlement_solana::test_support` for how that validator is stood up. The suite
//! requiring no changes for the deployed program's own, very different
//! wire (SPL-token PDAs and an Ed25519-precompile balance proof, in place
//! of the old crate's native-SOL, unverified one) is the measure of
//! success issue #567 itself names for the port being chain-agnostic.
//!
//! **The backend under test is built by [`SolanaSettlementBackend::connect`]**
//! -- the constructor every real node uses -- not by the test-only
//! `deploy`. Until issue #1118 it was the other way round, and that is
//! precisely how a `fund` that could never work in production stayed green
//! here for so long: the suite exercised `fund` in six places with no
//! Solana exemption, and passed only because `deploy` privately holds the
//! counterparty's signing key. A contract suite that passes only under a
//! test-only constructor is not holding the implementation to the
//! contract, so this one no longer uses it.
//!
//! What the test itself supplies, rather than the backend, is everything a
//! real deployment supplies from outside the node: SOL for fees, tokens in
//! the node's own associated token account for it to collateralise with,
//! and a counterparty who holds their own key -- signing their own claims
//! and making their own deposits. A `deploy`-built backend appears here in
//! exactly one role, as the mock mint's authority, i.e. the faucet; the
//! backend under test never sees it.

use std::str::FromStr;
use std::sync::Arc;

use chrono::Duration;
use connector_settlement::contract::{assert_upholds_the_contract, ContractFixture};
use connector_settlement::{ChannelId, SettlementBackend};
use connector_settlement_solana::wire;
use connector_settlement_solana::SolanaSettlementBackend;
use solana_rpc_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::instruction::Instruction;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{Keypair, Signer};
use solana_sdk::transaction::Transaction;

use connector_settlement_solana::test_support::{
    fund, require_solana_test_validator, SolanaValidator, LOCAL_TEST_PROGRAM_ID,
};

/// How much mock USDC the node under test, and each counterparty, starts
/// with. Comfortably above everything the suite deposits (1,250 of the
/// node's own collateral, 1,050 of the counterparty's).
const STARTING_TOKENS: u64 = 1_000_000_000;

/// The counterparty's own `Deposit`, signed by the counterparty's own key
/// and paid out of the counterparty's own associated token account --
/// which is the only way the deployed program will ever credit their side
/// (`processor.rs:309-311`, `:356-360`: credit is by signer, and there is
/// no participant parameter to name anyone else). This is the external
/// actor a real deployment has -- `rig`, `toon-client`, a peer node -- and
/// what [`ContractFixture::fund_counterparty`] asks every fixture for.
async fn deposit_as_counterparty(
    rpc: &RpcClient,
    program_id: &Pubkey,
    token_mint: &Pubkey,
    counterparty: &Keypair,
    channel: &ChannelId,
    amount: u64,
) {
    let channel_pda = Pubkey::from_str(&channel.0).expect("a channel id is its PDA in base58");
    let (vault, _bump) = wire::vault_pda(&channel_pda, program_id);
    let depositor_token_account = spl_associated_token_account::get_associated_token_address(
        &counterparty.pubkey(),
        token_mint,
    );
    let instruction = Instruction::new_with_bytes(
        *program_id,
        &wire::pack_deposit(amount),
        wire::Accounts::deposit(
            &counterparty.pubkey(),
            &depositor_token_account,
            &vault,
            &channel_pda,
        ),
    );
    let recent_blockhash = rpc.get_latest_blockhash().await.expect("latest blockhash");
    let transaction = Transaction::new_signed_with_payer(
        &[instruction],
        Some(&counterparty.pubkey()),
        &[counterparty],
        recent_blockhash,
    );
    rpc.send_and_confirm_transaction(&transaction)
        .await
        .expect("the counterparty deposits on their own side");
}

#[tokio::test]
async fn solana_settlement_backend_upholds_the_contract() {
    if !require_solana_test_validator() {
        return;
    }

    let validator = SolanaValidator::spawn().await;
    let rpc_url = validator.rpc_url.clone();
    let program_id = Pubkey::from_str(LOCAL_TEST_PROGRAM_ID).expect("valid local test program id");

    assert_upholds_the_contract(|| async move {
        let rpc = RpcClient::new_with_commitment(rpc_url.clone(), CommitmentConfig::confirmed());

        // The only thing taken from a `deploy`-built backend: a mock
        // 6-decimal SPL mint and the authority to hand out tokens in it.
        // On a real cluster this is USDC and a faucet, neither of which is
        // the node's business.
        let faucet = SolanaSettlementBackend::deploy(&rpc_url, program_id)
            .await
            .expect("create a mock mint to settle in");
        let token_mint = faucet.token_mint();

        // The node under test, in exactly the shape a real one boots in:
        // a `[settlement.solana] key` seed, SOL for fees, and `connect`.
        let node = Keypair::new();
        let node_seed: [u8; 32] = node.to_bytes()[..32]
            .try_into()
            .expect("a Keypair's first 32 bytes are its seed");
        fund(&rpc, &node.pubkey()).await;
        let backend = SolanaSettlementBackend::connect(
            &connector_settlement_solana::RpcTransport::direct(&rpc_url).expect("rpc transport"),
            &node_seed,
            program_id,
            token_mint,
            6,
        )
        .await
        .expect("connect to the genesis-loaded payment-channel program");
        // The node's own collateral. `fund` is a self-deposit (issue
        // #1118), so without tokens of its own here there is nothing for
        // it to put behind its own claims -- which is the whole point, and
        // the local gap issue #1118 names for `local/keys.sh`.
        faucet
            .test_mint_tokens_to(&node.pubkey(), STARTING_TOKENS)
            .await
            .expect("give the node its own collateral to deposit");

        // Two counterparties whose keys this *test* holds, not the
        // backend: they sign their own claims and make their own deposits.
        // Two, because the deployed program holds one live channel per
        // (pair, mint) and `counterparty`'s is still Closed in its
        // challenge window when `instant_counterparty`'s opens.
        let counterparty_key = Keypair::new();
        let instant_counterparty_key = Keypair::new();
        for keypair in [&counterparty_key, &instant_counterparty_key] {
            fund(&rpc, &keypair.pubkey()).await;
            faucet
                .test_mint_tokens_to(&keypair.pubkey(), STARTING_TOKENS)
                .await
                .expect("give the counterparty tokens to deposit");
        }
        let counterparty = counterparty_key.pubkey().to_bytes().to_vec();
        let instant_counterparty = instant_counterparty_key.pubkey().to_bytes().to_vec();
        // `other_counterparty` is only ever opened against, never funded
        // or redeemed from, so it needs no key and no tokens.
        let other_counterparty = Keypair::new().pubkey().to_bytes().to_vec();

        // The counterparty signs the balance proof the deployed program's
        // Ed25519 precompile check recovers -- from the test's own copy of
        // their key, not from anything the backend holds.
        let sign_counterparty = Keypair::from_bytes(&counterparty_key.to_bytes())
            .expect("clone the counterparty keypair");
        let sign = move |channel: &ChannelId, nonce: u64, cumulative_amount: u128| {
            let channel_pda =
                Pubkey::from_str(&channel.0).expect("a channel id is its PDA in base58");
            let units = u64::try_from(cumulative_amount).expect("the suite's amounts fit a u64");
            let message = wire::balance_proof_message(&program_id, &channel_pda, nonce, units);
            sign_counterparty.sign_message(&message).as_ref().to_vec()
        };

        // Which of the two held identities is a given channel's
        // counterparty is re-derived from the channel PDA, exactly as the
        // old `deploy`-held version did: the PDA is a pure function of
        // (node, counterparty, mint).
        let deposit_rpc_url = rpc_url.clone();
        let depositors: Vec<Keypair> = [&counterparty_key, &instant_counterparty_key]
            .iter()
            .map(|keypair| Keypair::from_bytes(&keypair.to_bytes()).expect("clone a keypair"))
            .collect();
        let node_pubkey = node.pubkey();
        let depositors = Arc::new(depositors);
        let fund_counterparty = move |channel: &ChannelId, amount: u128| {
            let rpc_url = deposit_rpc_url.clone();
            let depositors = Arc::clone(&depositors);
            let channel = channel.clone();
            let boxed: connector_settlement::contract::BoxFuture<'static, ()> =
                Box::pin(async move {
                    let channel_pda =
                        Pubkey::from_str(&channel.0).expect("a channel id is its PDA in base58");
                    let depositor = depositors
                        .iter()
                        .find(|keypair| {
                            wire::channel_pda(
                                &node_pubkey,
                                &keypair.pubkey(),
                                &token_mint,
                                &program_id,
                            )
                            .0 == channel_pda
                        })
                        .expect("one of the held counterparties is this channel's");
                    let rpc =
                        RpcClient::new_with_commitment(rpc_url, CommitmentConfig::confirmed());
                    deposit_as_counterparty(
                        &rpc,
                        &program_id,
                        &token_mint,
                        depositor,
                        &channel,
                        u64::try_from(amount).expect("the suite's amounts fit a u64"),
                    )
                    .await;
                });
            boxed
        };

        ContractFixture {
            backend: Arc::new(backend) as Arc<dyn SettlementBackend>,
            counterparty,
            other_counterparty,
            instant_counterparty,
            sign: Box::new(sign),
            fund_counterparty: Box::new(fund_counterparty),
            // No protocol-level minimum challenge period is enforced by
            // the deployed program (unlike `TokenNetwork`'s one-hour
            // `MIN_SETTLEMENT_TIMEOUT`), so a zero-length one is already
            // due the instant the channel closes -- no advancing needed.
            instant_settlement_timeout: Duration::zero(),
            advance_past_instant_settlement_timeout: Box::new(|| Box::pin(async {})),
        }
    })
    .await;
}
