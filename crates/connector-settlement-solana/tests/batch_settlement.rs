//! `SolanaBatchSettlement` against solana-foundation's own `payment-channels`
//! binary -- the one deployed on mainnet-beta, loaded into a disposable
//! validator's genesis at its canonical id (`test_support::payment_channels_fixture`)
//! -- held to the batch-settlement port's contract suite unmodified (ADR
//! 0007, ADR 0074 decision 9), plus what the suite cannot ask of both chains:
//! the Solana-only admission rules on a real `open`, a voucher the program
//! would refuse, and a sealed channel (issue #1343).
//!
//! Everything the node does not do itself, the test does as the client
//! would: open (co-signed by the sponsor key, which pays), top up, request
//! close and sign vouchers with a session key. The backend is built by
//! `connect`, the constructor a node boots with.

use std::str::FromStr;
use std::sync::Arc;

use connector_settlement::batch::contract::{
    assert_upholds_the_contract, BatchContractFixture, ChannelTerms, OpenedChannel,
};
use connector_settlement::batch::{
    AdmissionRefusal, BatchChannelStatus, BatchSettlementBackend, BatchSettlementError,
    ChannelPresentation, Voucher, VoucherSigner,
};
use connector_settlement::ChannelId;
use connector_settlement_solana::batch::wire::{
    self, DistributionEntry, PAYMENT_CHANNELS_PROGRAM_ID,
};
use connector_settlement_solana::batch::SolanaBatchSettlement;
use connector_settlement_solana::test_support::{
    create_mint, fund, mint_to, payment_channels_fixture, require_solana_test_validator,
    BatchPayer, SolanaValidator, PAYMENT_CHANNELS_FIXTURE_SHA256,
};
use connector_settlement_solana::RpcTransport;
use solana_rpc_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{Keypair, Signer};

/// ADR 0074's default minimum `grace_period`, and comfortably above the
/// suite's 901-second floor.
const ONE_DAY: u64 = 86_400;
const STARTING_TOKENS: u64 = 1_000_000_000;

fn program_id() -> Pubkey {
    Pubkey::from_str(PAYMENT_CHANNELS_PROGRAM_ID).expect("base58 program id")
}

fn seed_of(keypair: &Keypair) -> [u8; 32] {
    keypair.to_bytes()[..32]
        .try_into()
        .expect("a Keypair's first 32 bytes are its seed")
}

/// A validator, a sponsor-keyed backend under test, a payer holding tokens
/// in both the settled mint and another, and the mint authority.
struct World {
    _validator: SolanaValidator,
    rpc: RpcClient,
    sponsor: Arc<Keypair>,
    settled_mint: Pubkey,
    other_mint: Pubkey,
    payer: Arc<BatchPayer>,
    backend: Arc<SolanaBatchSettlement>,
}

async fn world() -> World {
    let validator = SolanaValidator::spawn().await;
    let rpc =
        RpcClient::new_with_commitment(validator.rpc_url.clone(), CommitmentConfig::confirmed());

    let authority = Keypair::new();
    fund(&rpc, &authority.pubkey()).await;
    let settled_mint = create_mint(&rpc, &authority, 6).await;
    let other_mint = create_mint(&rpc, &authority, 6).await;

    // The node's `[settlement.solana]` key: sponsor, payee and receiver. It
    // needs SOL for the rent and fees it fronts, and nothing else.
    let sponsor = Keypair::new();
    fund(&rpc, &sponsor.pubkey()).await;

    let payer = BatchPayer::new(&validator.rpc_url, program_id()).await;
    for mint in [&settled_mint, &other_mint] {
        mint_to(
            &rpc,
            &authority,
            mint,
            &payer.payer.pubkey(),
            STARTING_TOKENS,
        )
        .await;
    }

    let backend = SolanaBatchSettlement::connect(
        &RpcTransport::direct(&validator.rpc_url).expect("rpc transport"),
        &seed_of(&sponsor),
        settled_mint,
        ONE_DAY,
        1,
    )
    .await
    .expect("connect to the genesis-loaded payment-channels program");
    assert_eq!(backend.sponsor(), sponsor.pubkey());

    World {
        _validator: validator,
        rpc,
        sponsor: Arc::new(sponsor),
        settled_mint,
        other_mint,
        payer: Arc::new(payer),
        backend: Arc::new(backend),
    }
}

fn presentation(channel: &Pubkey) -> ChannelPresentation {
    ChannelPresentation::Solana {
        channel: ChannelId(channel.to_string()),
    }
}

fn address(channel: &ChannelId) -> Pubkey {
    Pubkey::from_str(&channel.0).expect("a Solana channel id is its address")
}

#[tokio::test]
async fn solana_batch_settlement_upholds_the_contract() {
    if !require_solana_test_validator() {
        return;
    }
    let world = world().await;

    let open = {
        let payer = Arc::clone(&world.payer);
        let sponsor = Arc::clone(&world.sponsor);
        let (settled_mint, other_mint) = (world.settled_mint, world.other_mint);
        Box::new(move |terms: ChannelTerms| {
            let payer = Arc::clone(&payer);
            let sponsor = Arc::clone(&sponsor);
            let boxed: connector_settlement::batch::contract::BoxFuture<'static, OpenedChannel> =
                Box::pin(async move {
                    let mint = if terms.in_settled_token {
                        settled_mint
                    } else {
                        other_mint
                    };
                    let mut open = payer
                        .admissible_open(
                            &sponsor.pubkey(),
                            &mint,
                            u64::try_from(terms.deposit).expect("the suite's deposits fit a u64"),
                            u32::try_from(terms.delay_secs).expect("a grace period fits a u32"),
                        )
                        .await;
                    if !terms.pays_this_node {
                        open.payee = Pubkey::new_unique();
                    }
                    let channel = payer.open(&open, &sponsor).await.expect("open");
                    OpenedChannel {
                        presentation: presentation(&channel),
                        voucher_signer: VoucherSigner::Solana(payer.session.pubkey().to_bytes()),
                    }
                });
            boxed
        })
    };
    let deposit = {
        let payer = Arc::clone(&world.payer);
        let mint = world.settled_mint;
        Box::new(move |channel: &ChannelId, amount: u128| {
            let payer = Arc::clone(&payer);
            let channel = address(channel);
            let boxed: connector_settlement::batch::contract::BoxFuture<'static, ()> =
                Box::pin(async move {
                    payer
                        .top_up(&channel, &mint, u64::try_from(amount).expect("fits a u64"))
                        .await
                        .expect("top_up");
                });
            boxed
        })
    };
    let begin_exit = {
        let payer = Arc::clone(&world.payer);
        Box::new(move |channel: &ChannelId| {
            let payer = Arc::clone(&payer);
            let channel = address(channel);
            let boxed: connector_settlement::batch::contract::BoxFuture<'static, ()> =
                Box::pin(async move {
                    payer.request_close(&channel).await.expect("request_close");
                });
            boxed
        })
    };
    let sign = {
        let payer = Arc::clone(&world.payer);
        Box::new(move |channel: &ChannelId, amount: u128| {
            payer
                .sign(
                    &address(channel),
                    u64::try_from(amount).expect("fits a u64"),
                )
                .to_vec()
        })
    };
    let fixture = BatchContractFixture {
        backend: Arc::clone(&world.backend) as Arc<dyn BatchSettlementBackend>,
        minimum_delay_secs: ONE_DAY,
        open,
        deposit,
        begin_exit,
        sign,
        unopened: presentation(&Pubkey::new_unique()),
    };

    assert_upholds_the_contract(|| async move { fixture }).await;
}

/// What the suite cannot ask of both chains, on real channels:
///
/// - the two seats only Solana has -- a third-party `rent_payer`, and a
///   distribution that sends anything anywhere but this node's receiver --
///   are refused by name;
/// - an account that is not a `payment-channels` channel is not found;
/// - a voucher not signed by the channel's `authorized_signer` is refused
///   before anything is sent, so it costs the sponsor nothing;
/// - a channel already closing is not admitted, and once sealed nothing
///   lands on it.
#[tokio::test]
async fn solana_only_rules_hold_against_the_deployed_program() {
    if !require_solana_test_validator() {
        return;
    }
    let world = world().await;
    let sponsor = world.sponsor.pubkey();
    let payer = &world.payer;
    let backend = &world.backend;

    // A third party sponsors: it pays the rent and holds the rent seat.
    let third_party = Keypair::new();
    fund(&world.rpc, &third_party.pubkey()).await;
    let open = wire::OpenChannel {
        rent_payer: third_party.pubkey(),
        ..payer
            .admissible_open(&sponsor, &world.settled_mint, 1_000, ONE_DAY as u32)
            .await
    };
    let channel = payer.open(&open, &third_party).await.expect("open");
    assert_eq!(
        backend.admit(presentation(&channel)).await,
        Err(BatchSettlementError::NotAdmissible {
            channel: ChannelId(channel.to_string()),
            refusal: AdmissionRefusal::NotPayableToThisNode {
                field: "rent_payer"
            },
        })
    );

    // Everything is paid to this node except one basis point.
    let open = wire::OpenChannel {
        recipients: vec![
            DistributionEntry {
                recipient: sponsor,
                bps: 9_999,
            },
            DistributionEntry {
                recipient: Pubkey::new_unique(),
                bps: 1,
            },
        ],
        ..payer
            .admissible_open(&sponsor, &world.settled_mint, 1_000, ONE_DAY as u32)
            .await
    };
    let channel = payer.open(&open, &world.sponsor).await.expect("open");
    assert_eq!(
        backend.admit(presentation(&channel)).await,
        Err(BatchSettlementError::NotAdmissible {
            channel: ChannelId(channel.to_string()),
            refusal: AdmissionRefusal::NotPayableToThisNode {
                field: "distribution_hash"
            },
        })
    );

    // A real account that is not this program's channel: the mint.
    let err = backend
        .admit(presentation(&world.settled_mint))
        .await
        .unwrap_err();
    assert_eq!(
        err,
        BatchSettlementError::ChannelNotFound(ChannelId(world.settled_mint.to_string()))
    );
    // And a string that names no address at all.
    let err = backend
        .admit(ChannelPresentation::Solana {
            channel: ChannelId("not-base58!".to_string()),
        })
        .await
        .unwrap_err();
    assert!(matches!(err, BatchSettlementError::ChannelNotFound(_)));

    // An admitted channel, and a voucher for it signed by the wrong key.
    let open = payer
        .admissible_open(&sponsor, &world.settled_mint, 1_000, ONE_DAY as u32)
        .await;
    let channel = payer.open(&open, &world.sponsor).await.expect("open");
    let id = ChannelId(channel.to_string());
    backend.admit(presentation(&channel)).await.expect("admit");
    let impostor = Keypair::new();
    let forged = impostor
        .sign_message(&connector_signer::solana_voucher_message(
            &channel.to_bytes(),
            100,
            0,
        ))
        .as_ref()
        .to_vec();
    let lamports_before = world
        .rpc
        .get_balance(&sponsor)
        .await
        .expect("sponsor balance");
    let err = backend
        .land(
            &id,
            Voucher {
                cumulative_amount: 100,
                signature: forged,
            },
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, BatchSettlementError::InvalidVoucherSignature(_)),
        "{err:?}"
    );
    let err = backend
        .land(
            &id,
            Voucher {
                cumulative_amount: 100,
                signature: vec![0; 65],
            },
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, BatchSettlementError::InvalidVoucherSignature(_)),
        "{err:?}"
    );
    assert_eq!(
        world.rpc.get_balance(&sponsor).await.expect("balance"),
        lamports_before,
        "a refused voucher is refused before a fee is spent"
    );

    // The payer asks to close. A closing channel backs no new voucher, so
    // it is not admitted afresh...
    payer.request_close(&channel).await.expect("request_close");
    assert_eq!(
        backend.admit(presentation(&channel)).await,
        Err(BatchSettlementError::NotAdmissible {
            channel: id.clone(),
            refusal: AdmissionRefusal::NotOpen,
        })
    );
    // ...but the voucher already held still lands, with `settle_and_seal`,
    // and that seals the channel: nothing lands after it.
    let state = backend
        .land(
            &id,
            Voucher {
                cumulative_amount: 250,
                signature: payer.sign(&channel, 250).to_vec(),
            },
        )
        .await
        .expect("settle_and_seal while closing");
    assert_eq!(state.status, BatchChannelStatus::Sealed);
    assert_eq!((state.landed, state.collateral), (250, 0));
    assert_eq!(
        backend
            .land(
                &id,
                Voucher {
                    cumulative_amount: 300,
                    signature: payer.sign(&channel, 300).to_vec(),
                },
            )
            .await,
        Err(BatchSettlementError::ChannelSealed(id.clone()))
    );
    assert_eq!(
        backend.channel_state(&id).await.expect("state").status,
        BatchChannelStatus::Sealed
    );
}

/// The committed binary is the one its provenance names: a replaced
/// fixture changes this constant in the same reviewed diff, or fails here.
#[test]
fn the_payment_channels_fixture_is_the_dumped_mainnet_binary() {
    let bytes = std::fs::read(payment_channels_fixture()).expect("the committed fixture");
    let digest = solana_sdk::hash::hash(&bytes).to_bytes();
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    assert_eq!(bytes.len(), 66_240);
    assert_eq!(hex, PAYMENT_CHANNELS_FIXTURE_SHA256);
}
