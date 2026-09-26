//! ADR 0074 decision 9 (issue #1346), against a real chain: a client with
//! tokens and no SOL of its own to spare posts the payer-signed `open` a
//! stock x402 client builds -- a version-0 message, this node's sponsor key
//! as fee payer, a compute budget prefix and a uniqueness memo -- to a node
//! built from its config file, and the node co-signs it as fee payer and
//! `rent_payer`, submits it, and admits the channel. Every refusal that
//! needs the chain to decide it is shown here by name, and each leaves the
//! sponsor's balance exactly where it was: nothing was signed and sent.
//!
//! The refusals decided from the transaction alone are pinned, one by one,
//! beside the rules in `connector_settlement_solana::batch::sponsor`.
//!
//! Its own test binary, and one test: `solana-test-validator` binds fixed
//! ports, so one validator per binary.

mod support;

use std::io::Write;
use std::str::FromStr;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use base64::Engine as _;
use connector_domain::{Fulfill, Reject};
use connector_settlement::batch::{BatchChannelStatus, BatchSettlementBackend};
use connector_settlement::ChannelId;
use connector_settlement_solana::batch::sponsor::MEMO_PROGRAM_ID;
use connector_settlement_solana::batch::wire::{self, OpenChannel, PAYMENT_CHANNELS_PROGRAM_ID};
use connector_settlement_solana::test_support::{
    create_mint, fund, mint_to, require_solana_test_validator, send, BatchPayer, SolanaValidator,
    LOCAL_TEST_PROGRAM_ID,
};
use solana_rpc_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::instruction::Instruction;
use solana_sdk::message::{v0, VersionedMessage};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{Keypair, Signature, Signer};
use solana_sdk::transaction::VersionedTransaction;
use tower::ServiceExt;

use support::{paid_prepare, post_ilp, solana_voucher, spawn_recording_app};

const SPONSOR_PATH: &str = "/ilp/batch-settlement/solana/open";
const ROUTE: &str = "g.toon.sponsored";
const PRICE: u64 = 100;
const MIN_SPONSORED_DEPOSIT: u64 = 1_000;
const ONE_DAY: u32 = 86_400;

/// What a stock x402 client's `buildOpenPaymentChannelTransaction` returns:
/// base64 of a version-0 transaction whose fee payer is `sponsor`, carrying
/// `SetComputeUnitLimit`, `SetComputeUnitPrice`, the `open` and a hex memo,
/// signed by the payer alone.
async fn client_built(
    rpc: &RpcClient,
    payer: &BatchPayer,
    sponsor: &Pubkey,
    open: &OpenChannel,
) -> String {
    let program = Pubkey::from_str(PAYMENT_CHANNELS_PROGRAM_ID).expect("base58");
    let mut limit = vec![2];
    limit.extend_from_slice(&90_000u32.to_le_bytes());
    let mut price = vec![3];
    price.extend_from_slice(&1u64.to_le_bytes());
    let instructions = [
        Instruction::new_with_bytes(solana_sdk::compute_budget::id(), &limit, vec![]),
        Instruction::new_with_bytes(solana_sdk::compute_budget::id(), &price, vec![]),
        open.instruction(&program),
        Instruction::new_with_bytes(
            Pubkey::from_str(MEMO_PROGRAM_ID).expect("base58"),
            b"8f3a1c0e5b7d9f2a4c6e8a0b2d4f6a8c",
            vec![],
        ),
    ];
    let blockhash = rpc.get_latest_blockhash().await.expect("blockhash");
    let message = VersionedMessage::V0(
        v0::Message::try_compile(sponsor, &instructions, &[], blockhash).expect("compiles"),
    );
    let bytes = message.serialize();
    let transaction = VersionedTransaction {
        // Key 0 is the sponsor's empty slot; key 1 the payer's.
        signatures: vec![Signature::default(), payer.payer.sign_message(&bytes)],
        message,
    };
    base64::engine::general_purpose::STANDARD
        .encode(bincode::serialize(&transaction).expect("serializes"))
}

/// POST `transaction` to the sponsor endpoint.
async fn sponsor(app: &Router, transaction: &str) -> (StatusCode, serde_json::Value) {
    let response = app
        .clone()
        .oneshot(
            Request::post(SPONSOR_PATH)
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "transaction": transaction }).to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let bytes = hyper::body::to_bytes(response.into_body())
        .await
        .expect("body");
    (
        status,
        serde_json::from_slice(&bytes).expect("a JSON answer"),
    )
}

/// Prefund `channel` with its full rent from `funder`, so the `open` runs
/// on the v2.1.21 validator the Rust Workspace Gate pins, whose Rent sysvar
/// still carries an exemption threshold of 2 that `payment-channels`
/// ignores (see `BatchPayer::open`). The program tops up only a shortfall,
/// so on a validator with SIMD-0194's threshold of 1 this changes nothing
/// the sponsor pays but the channel's own rent. The sponsor does not do
/// this itself, and why is `connector_settlement_solana::batch::sponsor`'s
/// module doc.
async fn prefund_channel_rent(rpc: &RpcClient, funder: &Keypair, channel: &Pubkey) {
    let rent = rpc
        .get_minimum_balance_for_rent_exemption(wire::CHANNEL_ACCOUNT_LEN)
        .await
        .expect("rent");
    send(
        rpc,
        &[solana_sdk::system_instruction::transfer(
            &funder.pubkey(),
            channel,
            rent,
        )],
        funder,
        &[],
    )
    .await
    .expect("prefund the channel's rent");
}

#[tokio::test]
async fn the_sponsor_endpoint_opens_a_channel_and_refuses_by_name() {
    if !require_solana_test_validator() {
        return;
    }

    let validator = SolanaValidator::spawn().await;
    let rpc =
        RpcClient::new_with_commitment(validator.rpc_url.clone(), CommitmentConfig::confirmed());
    let authority = Keypair::new();
    fund(&rpc, &authority.pubkey()).await;
    let mint = create_mint(&rpc, &authority, 6).await;

    let sponsor_seed = [0x6b; 32];
    let sponsor_key =
        solana_sdk::signer::keypair::keypair_from_seed(&sponsor_seed).expect("a keypair");
    let sponsor_pubkey = sponsor_key.pubkey();
    fund(&rpc, &sponsor_pubkey).await;

    let program = Pubkey::from_str(PAYMENT_CHANNELS_PROGRAM_ID).expect("base58");
    let payer = BatchPayer::new(&validator.rpc_url, program).await;
    mint_to(&rpc, &authority, &mint, &payer.payer.pubkey(), 1_000_000).await;

    let (app_addr, recorded) = spawn_recording_app().await;
    let state_dir = tempfile::tempdir().expect("state dir");
    let mut signer_key = tempfile::NamedTempFile::new().expect("signer key");
    signer_key.write_all(&[0x07; 32]).expect("write");
    let mut settlement_key = tempfile::NamedTempFile::new().expect("settlement key");
    settlement_key.write_all(&sponsor_seed).expect("write");
    let config = support::load_config(&format!(
        r#"
client_edge_addr = "127.0.0.1:0"
state_dir = "{state_dir}"

[signer]
key_file = "{signer_key}"

[settlement.solana]
rpc_url = "{rpc_url}"
program_id = "{LOCAL_TEST_PROGRAM_ID}"
token_address = "{mint}"
decimals = 6

[settlement.solana.key]
key_file = "{settlement_key}"

[settlement.solana.batch_settlement]
min_sponsored_deposit = {MIN_SPONSORED_DEPOSIT}

[[routes]]
prefix = "{ROUTE}"
handler_url = "http://{app_addr}"
price = {PRICE}
"#,
        state_dir = state_dir.path().display(),
        signer_key = signer_key.path().display(),
        settlement_key = settlement_key.path().display(),
        rpc_url = validator.rpc_url,
    ));
    let runtime = connector_cli::build(&config).await.expect("build");
    let app = connector_cli::router(&runtime, &config).expect("router");

    // The settlement backend creates this node's receiving account when it
    // connects. Close it, so the node is left without one.
    let receiving =
        spl_associated_token_account::get_associated_token_address(&sponsor_pubkey, &mint);
    send(
        &rpc,
        &[spl_token::instruction::close_account(
            &spl_token::id(),
            &receiving,
            &sponsor_pubkey,
            &sponsor_pubkey,
            &[],
        )
        .expect("close_account")],
        &sponsor_key,
        &[],
    )
    .await
    .expect("close the receiving account");

    let balance = || async { rpc.get_balance(&sponsor_pubkey).await.expect("balance") };
    let untouched = balance().await;
    let expect_refused = |name: &'static str, expected_status: StatusCode| {
        move |(status, body): (StatusCode, serde_json::Value)| {
            assert_eq!(body["error"], name, "{body}");
            assert_eq!(status, expected_status, "{body}");
        }
    };

    // The node's receiving account does not exist: a payout to it would
    // forfeit to the program's treasury (Cantina 3.1.4).
    let open = payer
        .admissible_open(&sponsor_pubkey, &mint, MIN_SPONSORED_DEPOSIT, ONE_DAY)
        .await;
    expect_refused(
        "receiving_account_unusable",
        StatusCode::UNPROCESSABLE_ENTITY,
    )(
        sponsor(
            &app,
            &client_built(&rpc, &payer, &sponsor_pubkey, &open).await,
        )
        .await,
    );

    // Recreated, by anyone: an ATA is its owner's whoever pays for it.
    send(
        &rpc,
        &[
            spl_associated_token_account::instruction::create_associated_token_account_idempotent(
                &authority.pubkey(),
                &sponsor_pubkey,
                &mint,
                &spl_token::id(),
            ),
        ],
        &authority,
        &[],
    )
    .await
    .expect("create the receiving account");

    // A payer with no token account: its refund would forfeit.
    let tokenless = BatchPayer::new(&validator.rpc_url, program).await;
    let open = tokenless
        .admissible_open(&sponsor_pubkey, &mint, MIN_SPONSORED_DEPOSIT, ONE_DAY)
        .await;
    expect_refused(
        "payer_token_account_unusable",
        StatusCode::UNPROCESSABLE_ENTITY,
    )(
        sponsor(
            &app,
            &client_built(&rpc, &tokenless, &sponsor_pubkey, &open).await,
        )
        .await,
    );

    // A payer whose account cannot fund the deposit it names.
    let open = payer
        .admissible_open(&sponsor_pubkey, &mint, 1_000_001, ONE_DAY)
        .await;
    expect_refused(
        "payer_token_account_unusable",
        StatusCode::UNPROCESSABLE_ENTITY,
    )(
        sponsor(
            &app,
            &client_built(&rpc, &payer, &sponsor_pubkey, &open).await,
        )
        .await,
    );

    // Below the published minimums, end to end through the HTTP answer.
    let open = payer
        .admissible_open(&sponsor_pubkey, &mint, MIN_SPONSORED_DEPOSIT - 1, ONE_DAY)
        .await;
    expect_refused("deposit_below_minimum", StatusCode::UNPROCESSABLE_ENTITY)(
        sponsor(
            &app,
            &client_built(&rpc, &payer, &sponsor_pubkey, &open).await,
        )
        .await,
    );
    let open = payer
        .admissible_open(&sponsor_pubkey, &mint, MIN_SPONSORED_DEPOSIT, ONE_DAY - 1)
        .await;
    expect_refused(
        "grace_period_below_minimum",
        StatusCode::UNPROCESSABLE_ENTITY,
    )(
        sponsor(
            &app,
            &client_built(&rpc, &payer, &sponsor_pubkey, &open).await,
        )
        .await,
    );

    // A transaction every static rule and both accounts pass, which the
    // program itself refuses -- an `open_slot` in the future -- is caught by
    // simulating the co-signed bytes, never by sending them.
    let future = OpenChannel {
        open_slot: rpc.get_slot().await.expect("slot") + 100_000,
        ..payer
            .admissible_open(&sponsor_pubkey, &mint, MIN_SPONSORED_DEPOSIT, ONE_DAY)
            .await
    };
    prefund_channel_rent(&rpc, &authority, &future.channel(&program)).await;
    expect_refused("simulation_failed", StatusCode::UNPROCESSABLE_ENTITY)(
        sponsor(
            &app,
            &client_built(&rpc, &payer, &sponsor_pubkey, &future).await,
        )
        .await,
    );

    // An `open` of a channel nobody prefunded. Where the cluster's Rent
    // sysvar still carries a threshold of 2 -- the v2.1.21 validator the
    // Rust Workspace Gate pins -- `payment-channels` would leave it short
    // of rent, so it is refused by name before anything is signed, rather
    // than as `simulation_failed` (issue #1356). Where the threshold is 1,
    // the same `open` is sponsored, below.
    let threshold_is_one = cluster_rent_threshold_is_one(&rpc).await;
    // The gate's validator is what puts the refusal arm under test in CI.
    // A pin bump that moves it to threshold 1 must say so here, not
    // silently stop exercising the refusal end to end.
    assert!(
        std::env::var_os("CI").is_none() || !threshold_is_one,
        "CI's solana-test-validator (the v2.1.21 pin) is expected to carry a rent threshold of 2, \
         so the cluster_rent_threshold_unsupported arm below runs; if the pin moved, cover that \
         refusal some other way"
    );
    if !threshold_is_one {
        let unprefunded = payer
            .admissible_open(&sponsor_pubkey, &mint, MIN_SPONSORED_DEPOSIT, ONE_DAY)
            .await;
        expect_refused(
            "cluster_rent_threshold_unsupported",
            StatusCode::UNPROCESSABLE_ENTITY,
        )(
            sponsor(
                &app,
                &client_built(&rpc, &payer, &sponsor_pubkey, &unprefunded).await,
            )
            .await,
        );
        assert_eq!(
            rpc.get_balance(&unprefunded.channel(&program))
                .await
                .expect("balance"),
            0,
            "nothing was sent: the channel address holds nothing"
        );
    }

    assert_eq!(
        balance().await,
        untouched,
        "no refusal cost the sponsor a lamport: nothing was signed and sent"
    );

    // And one it co-signs, submits and admits.
    let open = payer
        .admissible_open(&sponsor_pubkey, &mint, MIN_SPONSORED_DEPOSIT, ONE_DAY)
        .await;
    let channel = open.channel(&program);
    prefund_channel_rent(&rpc, &authority, &channel).await;
    let transaction = client_built(&rpc, &payer, &sponsor_pubkey, &open).await;
    let (status, body) = sponsor(&app, &transaction).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["channelId"], channel.to_string());
    assert_eq!(body["payer"], payer.payer.pubkey().to_string());
    assert_eq!(body["deposit"], MIN_SPONSORED_DEPOSIT.to_string());
    let signature =
        Signature::from_str(body["transaction"].as_str().expect("a signature")).expect("base58");
    assert!(
        rpc.confirm_transaction(&signature).await.expect("status"),
        "the open landed"
    );
    assert!(
        balance().await < untouched,
        "the sponsor paid the fee and the escrow's rent"
    );

    let backend = runtime
        .batch_settlement_solana
        .clone()
        .expect("opted in on Solana");
    let state = backend
        .channel_state(&ChannelId(channel.to_string()))
        .await
        .expect("the sponsor admitted the channel it opened");
    assert_eq!(state.status, BatchChannelStatus::Open);
    assert_eq!(state.collateral, u128::from(MIN_SPONSORED_DEPOSIT));

    // The same bytes again cannot open anything twice.
    let (status, body) = sponsor(&app, &transaction).await;
    assert_ne!(status, StatusCode::OK, "{body}");

    // The channel it opened pays for a write.
    let receiver = runtime.signer.public_key().expect("the node's wrap key");
    let response = post_ilp(
        &app,
        &solana_voucher(&channel.to_string(), PRICE, &payer.sign(&channel, PRICE)),
        paid_prepare(ROUTE, &receiver),
    )
    .await;
    Fulfill::decode(&response).unwrap_or_else(|_| {
        panic!(
            "a voucher on the sponsored channel paid for the write: {:?}",
            Reject::decode(&response)
        )
    });
    assert_eq!(recorded.lock().unwrap().len(), 1);

    // Where SIMD-0194 has set the threshold to 1 -- mainnet-beta, devnet, a
    // v3+ validator -- the program's rent figure is the real one, and an
    // `open` of a channel nobody prefunded is sponsored as it stands.
    if threshold_is_one {
        let unprefunded = payer
            .admissible_open(&sponsor_pubkey, &mint, MIN_SPONSORED_DEPOSIT, ONE_DAY)
            .await;
        let (status, body) = sponsor(
            &app,
            &client_built(&rpc, &payer, &sponsor_pubkey, &unprefunded).await,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["channelId"], unprefunded.channel(&program).to_string());
    }
}

/// Whether the cluster's Rent sysvar carries SIMD-0194's exemption
/// threshold of 1, read the way the sponsor reads it.
async fn cluster_rent_threshold_is_one(rpc: &RpcClient) -> bool {
    let sysvar = rpc
        .get_account(&solana_sdk::sysvar::rent::id())
        .await
        .expect("the Rent sysvar");
    let rent: solana_sdk::rent::Rent = bincode::deserialize(&sysvar.data).expect("decodes");
    rent.exemption_threshold == 1.0
}
