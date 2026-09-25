//! Issue #630: `SolanaSettlementBackend::connect`'s fail-closed identity
//! checks. `deploy` and the contract suite already prove program-reachable
//! and mint-owned-by-SPL-Token (issue #567); these are the "fuller" checks
//! `connect`'s own doc deferred to this later issue -- the configured
//! `decimals` must agree with the mint's own `decimals` field (the same
//! `#564` rule `EvmSettlementBackend::connect` already enforces for its
//! ERC-20's `decimals()`), and the configured `program_id` must actually
//! behave like the deployed payment-channel program, not merely be
//! executable (`verify_program_identity`, this issue's review finding 2).

use std::str::FromStr;

use connector_chain_rpc::{FakeRpc, RpcReply};
use connector_settlement_solana::SolanaSettlementBackend;
use solana_rpc_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signer;

use connector_settlement_solana::test_support::{
    fund, require_solana_test_validator, SolanaValidator, LOCAL_TEST_PROGRAM_ID,
};

/// A funded ed25519 seed [`SolanaSettlementBackend::connect`] can sign
/// transactions with -- `connect` submits one on a key's first start
/// (`ensure_own_ata_exists`, when the token account is missing), and refuses
/// an unfunded payer by name, so the identity it binds to needs real
/// lamports, exactly as a freshly generated production signer would on a
/// real cluster.
async fn funded_seed(rpc: &RpcClient, seed: [u8; 32]) -> [u8; 32] {
    let payer = solana_sdk::signer::keypair::keypair_from_seed(&seed).expect("derive keypair");
    fund(rpc, &payer.pubkey()).await;
    seed
}

#[tokio::test]
async fn connect_refuses_a_decimals_mismatch_naming_both_values() {
    if !require_solana_test_validator() {
        return;
    }

    let validator = SolanaValidator::spawn().await;
    let program_id = Pubkey::from_str(LOCAL_TEST_PROGRAM_ID).expect("valid local test program id");
    // `deploy` creates a fresh 6-decimal mint (see its own doc/body).
    let deployed = SolanaSettlementBackend::deploy(&validator.rpc_url, program_id)
        .await
        .expect("bind to the genesis-loaded payment-channel program");
    let token_mint = deployed.token_mint();

    let rpc =
        RpcClient::new_with_commitment(validator.rpc_url.clone(), CommitmentConfig::confirmed());
    let seed = funded_seed(&rpc, [3u8; 32]).await;

    let Err(error) = SolanaSettlementBackend::connect(
        &connector_settlement_solana::RpcTransport::direct(&validator.rpc_url)
            .expect("rpc transport"),
        &seed,
        program_id,
        token_mint,
        9,
    )
    .await
    else {
        panic!("a decimals the mint disagrees with must refuse to connect");
    };
    let message = error.to_string();
    assert!(
        message.contains("decimals is 9") && message.contains("decimals = 6"),
        "the failure must name both the configured and the on-chain decimals: {message}"
    );
}

#[tokio::test]
async fn connect_succeeds_when_decimals_agree() {
    if !require_solana_test_validator() {
        return;
    }

    let validator = SolanaValidator::spawn().await;
    let program_id = Pubkey::from_str(LOCAL_TEST_PROGRAM_ID).expect("valid local test program id");
    let deployed = SolanaSettlementBackend::deploy(&validator.rpc_url, program_id)
        .await
        .expect("bind to the genesis-loaded payment-channel program");
    let token_mint = deployed.token_mint();

    let rpc =
        RpcClient::new_with_commitment(validator.rpc_url.clone(), CommitmentConfig::confirmed());
    let seed = funded_seed(&rpc, [4u8; 32]).await;

    SolanaSettlementBackend::connect(
        &connector_settlement_solana::RpcTransport::direct(&validator.rpc_url)
            .expect("rpc transport"),
        &seed,
        program_id,
        token_mint,
        6,
    )
    .await
    .expect("decimals agree with the mint, connect should succeed");
}

/// Issue #630's review, finding 2: existing-and-executable is not
/// identity. A `program_id` naming a real, executable program that is not
/// the payment-channel program -- SPL Token itself here, executable on
/// every cluster including a fresh test validator -- must refuse to
/// connect, naming the configured program id, rather than pass the coarse
/// executability check and fail lazily at the first settle. The passing
/// twin is `connect_succeeds_when_decimals_agree` above: the same probe
/// runs there against the real program and lets connect through.
#[tokio::test]
async fn connect_refuses_a_program_id_naming_some_other_executable_program() {
    if !require_solana_test_validator() {
        return;
    }

    let validator = SolanaValidator::spawn().await;
    let program_id = Pubkey::from_str(LOCAL_TEST_PROGRAM_ID).expect("valid local test program id");
    let deployed = SolanaSettlementBackend::deploy(&validator.rpc_url, program_id)
        .await
        .expect("bind to the genesis-loaded payment-channel program");
    let token_mint = deployed.token_mint();

    let rpc =
        RpcClient::new_with_commitment(validator.rpc_url.clone(), CommitmentConfig::confirmed());
    let seed = funded_seed(&rpc, [6u8; 32]).await;

    // The canonical SPL Token program id -- deliberately spelled out
    // rather than taken from the `spl-token` crate, which is not a
    // dev-dependency of this crate's integration tests.
    let wrong_program_id = Pubkey::from_str("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA")
        .expect("the canonical SPL Token program id");
    let Err(error) = SolanaSettlementBackend::connect(
        &connector_settlement_solana::RpcTransport::direct(&validator.rpc_url)
            .expect("rpc transport"),
        &seed,
        wrong_program_id,
        token_mint,
        6,
    )
    .await
    else {
        panic!("a program_id naming some other executable program must refuse to connect");
    };
    let message = error.to_string();
    assert!(
        message.contains(&wrong_program_id.to_string()),
        "the failure must name the configured program id: {message}"
    );
}

/// Issue #1131: `connect` reads the chain's genesis hash to learn which
/// cluster it is on, and a `solana-test-validator` mints a fresh genesis on
/// every run -- so it matches no published cluster hash and can never match
/// one.
///
/// The load-bearing assertion is that `connect` still *succeeds*. Every
/// `local/` topology runs against exactly this validator, so a genesis read
/// that refused an unrecognised chain would take `make local-verify` and all
/// three CI topologies down; `cluster()` answering `None` is the same "this
/// node cannot say where it is, so it compares nothing" that
/// `SolanaSettlementConfig::cluster_hint` already answers for the
/// `solana-validator:8899` hostname those topologies configure.
#[tokio::test]
async fn a_test_validators_fresh_genesis_names_no_cluster_and_still_connects() {
    if !require_solana_test_validator() {
        return;
    }

    let validator = SolanaValidator::spawn().await;
    let program_id = Pubkey::from_str(LOCAL_TEST_PROGRAM_ID).expect("valid local test program id");
    let deployed = SolanaSettlementBackend::deploy(&validator.rpc_url, program_id)
        .await
        .expect("bind to the genesis-loaded payment-channel program");
    let token_mint = deployed.token_mint();

    let rpc =
        RpcClient::new_with_commitment(validator.rpc_url.clone(), CommitmentConfig::confirmed());
    let seed = funded_seed(&rpc, [7u8; 32]).await;

    let backend = SolanaSettlementBackend::connect(
        &connector_settlement_solana::RpcTransport::direct(&validator.rpc_url)
            .expect("rpc transport"),
        &seed,
        program_id,
        token_mint,
        6,
    )
    .await
    .expect("a chain this connector cannot name must still be connectable");
    assert_eq!(
        backend.cluster(),
        None,
        "a fresh test-validator genesis matches no published cluster hash, so this node \
         must record that it cannot name its cluster rather than guess one"
    );

    // And the genesis the validator actually reports really is one of the
    // unnameable ones -- otherwise the assertion above would pass for a
    // backend that never read the chain at all.
    let genesis_hash = rpc
        .get_genesis_hash()
        .await
        .expect("a running validator answers getGenesisHash");
    assert_eq!(
        connector_settlement_solana::cluster_for_genesis_hash(&genesis_hash),
        None,
        "the validator's own genesis hash {genesis_hash} must be one no public cluster published"
    );
}

#[tokio::test]
async fn connect_refuses_an_unreachable_rpc_endpoint() {
    // No validator spawned at all -- this must not hang or panic, just
    // report the RPC failure through `SettlementError::Backend`.
    let program_id = Pubkey::from_str(LOCAL_TEST_PROGRAM_ID).expect("valid local test program id");
    let seed = [5u8; 32];
    let result = SolanaSettlementBackend::connect(
        &connector_settlement_solana::RpcTransport::direct("http://127.0.0.1:1")
            .expect("rpc transport"),
        &seed,
        program_id,
        Pubkey::new_unique(),
        6,
    )
    .await;
    assert!(
        result.is_err(),
        "an unreachable RPC endpoint must refuse to connect"
    );
}

/// ADR 0073 decision 5: boot reads before it transacts. A key whose token
/// account already exists starts without sending anything, where it used to
/// submit the create on every start.
#[tokio::test]
async fn a_restart_reads_its_token_account_and_sends_no_transaction() {
    if !require_solana_test_validator() {
        return;
    }

    let validator = SolanaValidator::spawn().await;
    let program_id = Pubkey::from_str(LOCAL_TEST_PROGRAM_ID).expect("valid local test program id");
    let deployed = SolanaSettlementBackend::deploy(&validator.rpc_url, program_id)
        .await
        .expect("bind to the genesis-loaded payment-channel program");
    let token_mint = deployed.token_mint();
    let rpc =
        RpcClient::new_with_commitment(validator.rpc_url.clone(), CommitmentConfig::confirmed());
    let seed = funded_seed(&rpc, [8u8; 32]).await;
    let transport =
        connector_settlement_solana::RpcTransport::direct(&validator.rpc_url).expect("transport");

    // First start: the token account is missing, and is created.
    SolanaSettlementBackend::connect(&transport, &seed, program_id, token_mint, 6)
        .await
        .expect("first start");

    // Second start, watched: the traffic is the subject here.
    let watched = FakeRpc::spawn_in_front_of(&validator.rpc_url, |_| RpcReply::Forward).await;
    SolanaSettlementBackend::connect(
        &connector_settlement_solana::RpcTransport::direct(&watched.url()).expect("transport"),
        &seed,
        program_id,
        token_mint,
        6,
    )
    .await
    .expect("second start");
    assert_eq!(
        watched.count("sendTransaction"),
        0,
        "a token account that exists is read, not created again"
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

    let validator = SolanaValidator::spawn().await;
    let program_id = Pubkey::from_str(LOCAL_TEST_PROGRAM_ID).expect("valid local test program id");
    let deployed = SolanaSettlementBackend::deploy(&validator.rpc_url, program_id)
        .await
        .expect("bind to the genesis-loaded payment-channel program");
    let rpc =
        RpcClient::new_with_commitment(validator.rpc_url.clone(), CommitmentConfig::confirmed());
    let seed = funded_seed(&rpc, [9u8; 32]).await;

    let flaky = FakeRpc::spawn_in_front_of(&validator.rpc_url, |call| {
        if call.nth == 0 && call.method != "sendTransaction" {
            RpcReply::Drop
        } else {
            RpcReply::Forward
        }
    })
    .await;
    SolanaSettlementBackend::connect(
        &connector_settlement_solana::RpcTransport::direct(&flaky.url()).expect("transport"),
        &seed,
        program_id,
        deployed.token_mint(),
        6,
    )
    .await
    .expect("every boot read is retried, so one lost round trip each is survivable");
}
