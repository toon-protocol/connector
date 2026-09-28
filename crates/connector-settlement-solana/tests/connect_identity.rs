//! `SolanaBatchSettlement::connect`'s fail-closed boot checks (ADR 0075
//! decision 1, issue #1385; the `#564` decimals rule, issue #630; the
//! genesis-hash network, issue #1131; ADR 0073 decision 5's retried reads):
//! a chain without `payment-channels` is refused by name, a mint must be
//! the SPL Token program's and agree with the configured `decimals`, and
//! the network is read off the chain's own genesis hash.

use connector_chain_rpc::{FakeRpc, RpcReply};
use connector_settlement::batch::BatchSettlementError;
use connector_settlement_solana::batch::SolanaBatchSettlement;
use connector_settlement_solana::RpcTransport;
use solana_rpc_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{Keypair, Signer};

use connector_settlement_solana::test_support::{
    create_mint, fund, require_solana_test_validator, SolanaValidator,
};

const ONE_DAY: u64 = 86_400;

/// A funded ed25519 seed: `connect` refuses a key holding no lamports by
/// name, so the identity it binds to needs real lamports, exactly as a
/// freshly generated production signer would on a real cluster.
async fn funded_seed(rpc: &RpcClient, seed: [u8; 32]) -> [u8; 32] {
    let payer = solana_sdk::signer::keypair::keypair_from_seed(&seed).expect("derive keypair");
    fund(rpc, &payer.pubkey()).await;
    seed
}

async fn connect(
    rpc_url: &str,
    seed: &[u8; 32],
    mint: Pubkey,
    decimals: u8,
) -> Result<SolanaBatchSettlement, BatchSettlementError> {
    SolanaBatchSettlement::connect(
        &RpcTransport::direct(rpc_url).expect("rpc transport"),
        seed,
        mint,
        decimals,
        ONE_DAY,
        1,
    )
    .await
}

/// A validator, an RPC client on it, and a 6-decimal SPL mint.
async fn chain() -> (SolanaValidator, RpcClient, Pubkey) {
    let validator = SolanaValidator::spawn().await;
    let rpc =
        RpcClient::new_with_commitment(validator.rpc_url.clone(), CommitmentConfig::confirmed());
    let authority = Keypair::new();
    fund(&rpc, &authority.pubkey()).await;
    let mint = create_mint(&rpc, &authority, 6).await;
    (validator, rpc, mint)
}

/// ADR 0075 decision 1: a chain `payment-channels` has not deployed to is
/// refused by name, not bound to admit nothing and say nothing.
#[tokio::test]
async fn connect_refuses_a_chain_without_payment_channels_by_name() {
    if !require_solana_test_validator() {
        return;
    }
    let validator = SolanaValidator::spawn_without_payment_channels().await;
    let rpc =
        RpcClient::new_with_commitment(validator.rpc_url.clone(), CommitmentConfig::confirmed());
    let seed = funded_seed(&rpc, [4u8; 32]).await;

    let Err(error) = connect(&validator.rpc_url, &seed, Pubkey::new_unique(), 6).await else {
        panic!("bound on a chain without payment-channels");
    };
    assert!(
        matches!(&error, BatchSettlementError::NotDeployed(what) if what.contains("payment-channels")),
        "{error:?}"
    );
}

#[tokio::test]
async fn connect_refuses_a_decimals_mismatch_naming_both_values() {
    if !require_solana_test_validator() {
        return;
    }
    let (validator, rpc, mint) = chain().await;
    let seed = funded_seed(&rpc, [5u8; 32]).await;

    let Err(error) = connect(&validator.rpc_url, &seed, mint, 9).await else {
        panic!("bound over a decimals the mint disagrees with");
    };
    let message = error.to_string();
    assert!(
        message.contains('9') && message.contains('6'),
        "the refusal names both values: {message}"
    );
}

/// Token-2022 stays refused (ADR 0075 decision 1): a mint any program but
/// SPL Token owns is refused at boot.
#[tokio::test]
async fn connect_refuses_a_mint_the_spl_token_program_does_not_own() {
    if !require_solana_test_validator() {
        return;
    }
    let validator = SolanaValidator::spawn().await;
    let rpc =
        RpcClient::new_with_commitment(validator.rpc_url.clone(), CommitmentConfig::confirmed());
    let seed = funded_seed(&rpc, [6u8; 32]).await;
    // A system-owned account: funded, so it exists, and not a mint.
    let not_a_mint = Keypair::new().pubkey();
    fund(&rpc, &not_a_mint).await;

    let Err(error) = connect(&validator.rpc_url, &seed, not_a_mint, 6).await else {
        panic!("bound over a mint SPL Token does not own");
    };
    assert!(
        error.to_string().contains("SPL Token"),
        "the refusal names the owner it wanted: {error}"
    );
}

/// A fresh node has its receiving account -- the sponsor key's token
/// account for the mint -- once it has connected, so a channel can be
/// opened toward it from the first request; and a restart that finds it
/// sends nothing (ADR 0073 decision 5).
#[tokio::test]
async fn connect_creates_the_receiving_account_once() {
    if !require_solana_test_validator() {
        return;
    }
    let (validator, rpc, mint) = chain().await;
    let seed = funded_seed(&rpc, [8u8; 32]).await;
    let owner = solana_sdk::signer::keypair::keypair_from_seed(&seed)
        .expect("derive keypair")
        .pubkey();
    let receiving = spl_associated_token_account::get_associated_token_address(&owner, &mint);
    assert!(
        rpc.get_account(&receiving).await.is_err(),
        "none before boot"
    );

    connect(&validator.rpc_url, &seed, mint, 6)
        .await
        .expect("first boot");
    assert!(
        rpc.get_account(&receiving).await.is_ok(),
        "a fresh node has its receiving account"
    );

    let watched = FakeRpc::spawn_in_front_of(&validator.rpc_url, |_| RpcReply::Forward).await;
    connect(&watched.url(), &seed, mint, 6)
        .await
        .expect("second boot");
    assert_eq!(
        watched.count("sendTransaction"),
        0,
        "a receiving account that exists is read, not created again"
    );
}

/// Issue #1131 and ADR 0074 decision 8: a `solana-test-validator` mints a
/// fresh genesis on every run, so it names no public cluster -- and still
/// connects, with a CAIP-2 network of its own, which every `local/`
/// topology depends on.
#[tokio::test]
async fn a_test_validators_fresh_genesis_names_no_cluster_and_still_connects() {
    if !require_solana_test_validator() {
        return;
    }
    let (validator, rpc, mint) = chain().await;
    let seed = funded_seed(&rpc, [7u8; 32]).await;

    let backend = connect(&validator.rpc_url, &seed, mint, 6)
        .await
        .expect("an unnamed chain is recorded as unnamed, never refused");
    assert_eq!(backend.cluster(), None);
    let genesis_hash = rpc.get_genesis_hash().await.expect("genesis hash");
    assert_eq!(
        backend.caip2_network(),
        connector_settlement_solana::caip2_solana_network(&genesis_hash)
    );
    assert_eq!(backend.token_program(), spl_token::id());
}

#[tokio::test]
async fn connect_refuses_an_unreachable_rpc_endpoint() {
    // No validator spawned at all -- this must not hang or panic, just
    // report the RPC failure.
    let result = connect("http://127.0.0.1:1", &[5u8; 32], Pubkey::new_unique(), 6).await;
    assert!(
        result.is_err(),
        "an unreachable RPC endpoint must refuse to connect"
    );
}

/// ADR 0073 decision 5: a boot read that fails is retried before it fails
/// the node. The first attempt at every read is lost here, and the node
/// still starts.
#[tokio::test]
async fn a_boot_that_loses_a_round_trip_on_every_read_still_starts() {
    if !require_solana_test_validator() {
        return;
    }
    let (validator, rpc, mint) = chain().await;
    let seed = funded_seed(&rpc, [9u8; 32]).await;

    let flaky = FakeRpc::spawn_in_front_of(&validator.rpc_url, |call| {
        if call.nth == 0 {
            RpcReply::Drop
        } else {
            RpcReply::Forward
        }
    })
    .await;
    connect(&flaky.url(), &seed, mint, 6)
        .await
        .expect("every boot read is retried, so one lost round trip each is survivable");
}
