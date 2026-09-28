//! The Solana paying half of the batch-settlement port (ADR 0075 decisions 2
//! and 3, issue #1375) against solana-foundation's own `payment-channels`
//! binary and mainnet-beta's p-token, both in a disposable validator's
//! genesis (`SolanaValidator::spawn`).
//!
//! Two nodes, each a `SolanaBatchSettlement` built by `connect` from its own
//! settlement key, as a node boots: the payer, and a counterparty that serves
//! **this crate's real sponsor endpoint** (`connector_cli::sponsor_router`)
//! on a real socket. Every channel the payer opens reaches the chain only
//! through that endpoint, which co-signs as fee payer and `rent_payer` and
//! keeps the `payee` seat (ADR 0074 decisions 5 and 9) -- never through a
//! co-signature the test makes itself.
//!
//! Here:
//!
//! - the paying half's contract suite, unmodified (ADR 0007);
//! - a deposit below the counterparty's `min_sponsored_deposit`, which its
//!   sponsor refuses and the payer reports by the sponsor's own name for it;
//! - the close path end to end: `request_close`, the counterparty's own
//!   Closing watcher landing the payer's voucher with `settle_and_seal`, the
//!   payer's `distribute`, and the channel left Distributed in the
//!   counterparty's sweep, whose `reclaim` brings its rent float home
//!   (`connector-settlement-solana`'s `batch_watch.rs` shows that last step);
//! - a close the counterparty does not answer: once the grace period has run,
//!   the payer seals the channel itself and takes back its whole deposit.

use std::net::TcpListener;
use std::str::FromStr;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use connector_settlement::batch::contract::{
    assert_upholds_the_paying_contract, PayingContractFixture,
};
use connector_settlement::batch::{
    BatchChannelStatus, BatchSettlementBackend, BatchSettlementError, BatchSettlementPayer,
    ChannelPresentation, HeldVoucher, HeldVouchers, ReceiverTerms, SolanaReceiverTerms,
    VoucherSigner,
};
use connector_settlement::ChannelId;
use connector_settlement_solana::batch::wire::{self, PAYMENT_CHANNELS_PROGRAM_ID};
use connector_settlement_solana::batch::{SolanaBatchSettlement, SolanaBatchWatcher, Step};
use connector_settlement_solana::test_support::{
    create_mint, fund, mint_to, require_solana_test_validator, BatchPayer, SolanaValidator,
};
use connector_settlement_solana::RpcTransport;
use solana_rpc_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{Keypair, Signer};

/// The counterparty's minimum `grace_period` for the suite: ADR 0074's
/// default.
const ONE_DAY: u64 = 86_400;
/// The counterparty's `min_sponsored_deposit`: the most the suite allows.
const MIN_SPONSORED_DEPOSIT: u64 = 1_000;
const STARTING_TOKENS: u64 = 1_000_000;

fn seed_of(keypair: &Keypair) -> [u8; 32] {
    keypair.to_bytes()[..32]
        .try_into()
        .expect("a Keypair's first 32 bytes are its seed")
}

async fn token_balance(rpc: &RpcClient, owner: &Pubkey, mint: &Pubkey) -> u64 {
    let ata = spl_associated_token_account::get_associated_token_address(owner, mint);
    match rpc.get_token_account_balance(&ata).await {
        Ok(balance) => balance.amount.parse().expect("an amount"),
        Err(_) => 0,
    }
}

async fn read(rpc: &RpcClient, channel: &Pubkey) -> Option<wire::ChannelAccount> {
    rpc.get_account_with_commitment(channel, CommitmentConfig::confirmed())
        .await
        .expect("read")
        .value
        .and_then(|account| wire::ChannelAccount::parse(&account.data))
}

fn address(channel: &ChannelId) -> Pubkey {
    Pubkey::from_str(&channel.0).expect("a Solana channel id is its address")
}

/// Serve `backend`'s sponsor endpoint on a socket of its own, and return the
/// endpoint's absolute URL: what the counterparty's greeting publishes as
/// `sponsorEndpoint`, resolved against its URL.
fn serve_sponsor(backend: Arc<SolanaBatchSettlement>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind the sponsor endpoint");
    let addr = listener.local_addr().expect("its address");
    let router = connector_cli::sponsor_router(Some(backend));
    tokio::spawn(async move {
        axum::Server::from_tcp(listener)
            .expect("serve from the listener")
            .serve(router.into_make_service())
            .await
            .expect("the sponsor endpoint serves");
    });
    format!("http://{addr}{}", connector_cli::SPONSOR_PATH)
}

/// A validator, a mint, a paying node holding [`STARTING_TOKENS`], and a
/// counterparty node serving its sponsor endpoint.
struct World {
    _validator: SolanaValidator,
    rpc: RpcClient,
    rpc_url: String,
    mint: Pubkey,
    mint_authority: Keypair,
    payer_key: Pubkey,
    /// The paying node's settlement key, for a restart to connect again.
    payer_seed: [u8; 32],
    payer: Arc<SolanaBatchSettlement>,
    receiver: Arc<SolanaBatchSettlement>,
    terms: ReceiverTerms,
}

impl World {
    /// The counterparty admits and sponsors a `grace_period` of at least
    /// `min_grace_period_secs`.
    async fn new(min_grace_period_secs: u64) -> World {
        let validator = SolanaValidator::spawn().await;
        let rpc_url = validator.rpc_url.clone();
        let rpc = RpcClient::new_with_commitment(rpc_url.clone(), CommitmentConfig::confirmed());
        let mint_authority = Keypair::new();
        fund(&rpc, &mint_authority.pubkey()).await;
        let mint = create_mint(&rpc, &mint_authority, 6).await;

        // Each node's `[settlement.solana]` key, holding SOL for its own
        // fees. The payer's also holds the tokens it deposits; the
        // counterparty's holds a receiving account, which its sponsor checks
        // exists before it floats a channel's rent.
        let payer_key = Keypair::new();
        let receiver_key = Keypair::new();
        for key in [&payer_key, &receiver_key] {
            fund(&rpc, &key.pubkey()).await;
        }
        mint_to(
            &rpc,
            &mint_authority,
            &mint,
            &payer_key.pubkey(),
            STARTING_TOKENS,
        )
        .await;
        mint_to(&rpc, &mint_authority, &mint, &receiver_key.pubkey(), 0).await;

        let transport = RpcTransport::direct(&rpc_url).expect("rpc transport");
        let payer =
            SolanaBatchSettlement::connect(&transport, &seed_of(&payer_key), mint, ONE_DAY, 1)
                .await
                .expect("connect the paying node");
        let receiver = Arc::new(
            SolanaBatchSettlement::connect(
                &transport,
                &seed_of(&receiver_key),
                mint,
                min_grace_period_secs,
                MIN_SPONSORED_DEPOSIT,
            )
            .await
            .expect("connect the counterparty"),
        );
        let sponsor_endpoint = serve_sponsor(Arc::clone(&receiver));
        // The counterparty's `batchSettlements` entry for Solana, as its
        // self-description publishes it (ADR 0075 decision 10).
        let terms = ReceiverTerms::Solana(SolanaReceiverTerms {
            sponsor: receiver.sponsor().to_bytes(),
            receiver: receiver.receiver().to_bytes(),
            mint: receiver.mint().to_bytes(),
            min_grace_period_secs: receiver.min_grace_period_secs(),
            min_deposit: u128::from(receiver.min_sponsored_deposit()),
            sponsor_endpoint,
        });
        World {
            _validator: validator,
            rpc,
            rpc_url,
            mint,
            mint_authority,
            payer_key: payer_key.pubkey(),
            payer_seed: seed_of(&payer_key),
            payer: Arc::new(payer),
            receiver,
            terms,
        }
    }

    async fn payer_tokens(&self) -> u64 {
        token_balance(&self.rpc, &self.payer_key, &self.mint).await
    }

    async fn receiver_tokens(&self) -> u64 {
        token_balance(&self.rpc, &self.receiver.receiver(), &self.mint).await
    }
}

#[tokio::test]
async fn solana_batch_settlement_upholds_the_paying_contract() {
    if !require_solana_test_validator() {
        return;
    }
    let world = World::new(ONE_DAY).await;

    // A client's channel toward the counterparty: real, and not the payer's.
    let client = BatchPayer::new(
        &world.rpc_url,
        Pubkey::from_str(PAYMENT_CHANNELS_PROGRAM_ID).expect("base58"),
    )
    .await;
    mint_to(
        &world.rpc,
        &world.mint_authority,
        &world.mint,
        &client.payer.pubkey(),
        STARTING_TOKENS,
    )
    .await;
    // Its rent is paid by a key of the test's rather than through the
    // counterparty's endpoint: all the suite asks of it is that it is real
    // and that the payer did not open it.
    let rent_payer = Keypair::new();
    fund(&world.rpc, &rent_payer.pubkey()).await;
    let open = wire::OpenChannel {
        rent_payer: rent_payer.pubkey(),
        ..client
            .admissible_open(
                &world.receiver.sponsor(),
                &world.mint,
                1_000,
                ONE_DAY as u32,
            )
            .await
    };
    let not_outbound = match client.open(&open, &rent_payer).await {
        Ok(channel) => ChannelId(channel.to_string()),
        Err(error) => panic!("a client's open: {error}"),
    };

    let payer_balance = {
        let rpc_url = world.rpc_url.clone();
        let (owner, mint) = (world.payer_key, world.mint);
        Box::new(move || {
            let rpc_url = rpc_url.clone();
            let boxed: connector_settlement::batch::contract::BoxFuture<'static, u128> =
                Box::pin(async move {
                    let rpc =
                        RpcClient::new_with_commitment(rpc_url, CommitmentConfig::confirmed());
                    u128::from(token_balance(&rpc, &owner, &mint).await)
                });
            boxed
        })
    };
    let fixture = PayingContractFixture {
        payer: Arc::clone(&world.payer) as Arc<dyn BatchSettlementPayer>,
        receiver: Arc::clone(&world.receiver) as Arc<dyn BatchSettlementBackend>,
        terms: world.terms.clone(),
        payer_balance,
        payer_settlement_key: VoucherSigner::Solana(world.payer_key.to_bytes()),
        // The suite finishes a close only after the receiver has sealed it,
        // which is due at once; a validator's clock cannot be moved anyway.
        let_delay_pass: Box::new(|| Box::pin(async {})),
        not_outbound,
        // A node restarting: `connect` again from the same settlement key,
        // as a booting node does, remembering nothing.
        restart: {
            let rpc_url = world.rpc_url.clone();
            let (seed, mint) = (world.payer_seed, world.mint);
            Box::new(move || {
                let rpc_url = rpc_url.clone();
                Box::pin(async move {
                    let transport = RpcTransport::direct(&rpc_url).expect("rpc transport");
                    Arc::new(
                        SolanaBatchSettlement::connect(&transport, &seed, mint, ONE_DAY, 1)
                            .await
                            .expect("reconnect the paying node"),
                    ) as Arc<dyn BatchSettlementPayer>
                })
            })
        },
    };
    assert_upholds_the_paying_contract(|| async move { fixture }).await;
}

/// ADR 0074 decision 5's bound on the rent a sponsor floats, seen from the
/// paying side: the counterparty's sponsor refuses an opening deposit below
/// its `min_sponsored_deposit`, by name, and the payer reports that name.
/// Nothing is opened and nothing is spent.
#[tokio::test]
async fn a_deposit_below_the_counterpartys_minimum_is_refused_by_its_sponsor_by_name() {
    if !require_solana_test_validator() {
        return;
    }
    let world = World::new(ONE_DAY).await;
    let before = world.payer_tokens().await;
    let lamports_before = world
        .rpc
        .get_balance(&world.payer_key)
        .await
        .expect("the payer's SOL");

    let refused = world
        .payer
        .open(world.terms.clone(), u128::from(MIN_SPONSORED_DEPOSIT - 1))
        .await
        .unwrap_err();
    let BatchSettlementError::OpenRefused(reason) = &refused else {
        panic!("expected the sponsor's refusal, got {refused:?}");
    };
    assert!(
        reason.starts_with("deposit_below_minimum: "),
        "the payer surfaces the sponsor's own name for the refusal: {reason}"
    );
    assert_eq!(world.payer_tokens().await, before, "nothing was deposited");
    assert_eq!(
        world
            .rpc
            .get_balance(&world.payer_key)
            .await
            .expect("the payer's SOL"),
        lamports_before,
        "nothing was sent at all: not even a channel's rent, on a cluster that would need it"
    );
    assert!(
        world
            .receiver
            .sponsored_channels()
            .await
            .expect("read the counterparty's channels")
            .is_empty(),
        "nothing was opened"
    );

    // At the minimum, the same open goes through.
    world
        .payer
        .open(world.terms.clone(), u128::from(MIN_SPONSORED_DEPOSIT))
        .await
        .expect("an open at the minimum is sponsored");
}

/// The Solana close path, with the counterparty doing what a running node
/// does rather than what the suite asks of it directly: its own Closing
/// watcher lands the voucher the payer signed, the payer distributes, and the
/// channel waits Distributed in the counterparty's sweep for `reclaim`.
#[tokio::test]
async fn a_close_is_sealed_by_the_counterpartys_watcher_and_distributed_by_the_payer() {
    if !require_solana_test_validator() {
        return;
    }
    let world = World::new(ONE_DAY).await;
    let opened = world
        .payer
        .open(world.terms.clone(), 1_000)
        .await
        .expect("open");
    let channel = opened.presentation.channel().clone();
    let voucher = world.payer.sign_voucher(&channel, 250).await.expect("sign");

    // The counterparty holds the voucher, as its claim gate would.
    let held: Arc<RwLock<Vec<HeldVoucher>>> = Arc::new(RwLock::new(vec![HeldVoucher {
        presentation: ChannelPresentation::Solana {
            channel: channel.clone(),
        },
        voucher,
    }]));
    let watcher =
        SolanaBatchWatcher::new(Arc::clone(&world.receiver), held as Arc<dyn HeldVouchers>);

    let state = world
        .payer
        .start_withdrawal(&channel)
        .await
        .expect("request_close");
    assert_eq!(state.on_chain.status, BatchChannelStatus::Closing);

    let taken = watcher.tick(false).await.expect("the counterparty's pass");
    assert!(
        taken.contains(&(channel.clone(), Step::SettleAndSeal { amount: 250 })),
        "the counterparty lands the payer's voucher inside the grace period: {taken:?}"
    );

    let (payer_before, receiver_before) =
        (world.payer_tokens().await, world.receiver_tokens().await);
    let state = world
        .payer
        .finish_withdrawal(&channel)
        .await
        .expect("distribute the sealed channel");
    assert_eq!(state.on_chain.landed, 250);
    assert_eq!(state.signed, 250);
    assert_eq!(world.payer_tokens().await - payer_before, 750, "the refund");
    assert_eq!(
        world.receiver_tokens().await - receiver_before,
        250,
        "the counterparty's share, paid to its receiving account"
    );

    // What is left is the counterparty's rent float, in its own sweep.
    let account = read(&world.rpc, &address(&channel))
        .await
        .expect("inside its slot window the channel stays allocated");
    assert_eq!(account.status, wire::ChannelStatus::Distributed);
    assert_eq!(account.rent_payer, world.receiver.sponsor());
    let sponsored = world
        .receiver
        .sponsored_channels()
        .await
        .expect("the counterparty's channels");
    assert!(
        sponsored
            .iter()
            .any(|found| found.address == address(&channel)),
        "the counterparty's sweep finds the channel to reclaim"
    );
    let taken = watcher.tick(false).await.expect("the counterparty's pass");
    assert!(
        !taken.iter().any(|(id, _)| *id == channel),
        "nothing more is done to it until its slot window passes: {taken:?}"
    );
    assert_eq!(
        world
            .payer
            .outbound_state(&channel)
            .await
            .expect("state")
            .on_chain
            .status,
        BatchChannelStatus::Sealed
    );
}

/// A counterparty that does not answer a close: once the grace period has
/// run, the payer cranks `seal` itself and takes back everything unlanded.
/// A grace period of one second, which this counterparty admits, lets the
/// test wait it out.
#[tokio::test]
async fn once_the_grace_period_has_run_the_payer_seals_and_takes_back_its_deposit() {
    if !require_solana_test_validator() {
        return;
    }
    let world = World::new(1).await;
    let opened = world
        .payer
        .open(world.terms.clone(), 1_000)
        .await
        .expect("open");
    let channel = opened.presentation.channel().clone();
    world
        .payer
        .sign_voucher(&channel, 400)
        .await
        .expect("a voucher the counterparty never lands");
    let before = world.payer_tokens().await;
    world
        .payer
        .start_withdrawal(&channel)
        .await
        .expect("request_close");

    let mut finished = None;
    for _ in 0..60 {
        match world.payer.finish_withdrawal(&channel).await {
            Ok(state) => {
                finished = Some(state);
                break;
            }
            Err(BatchSettlementError::WithdrawalNotDue { .. }) => {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            Err(other) => panic!("finishing the withdrawal: {other:?}"),
        }
    }
    let state = finished.expect("the grace period ran out within the test");
    assert_eq!(state.on_chain.landed, 0, "nothing was landed in time");
    assert_eq!(
        world.payer_tokens().await - before,
        1_000,
        "the whole deposit comes back"
    );
    assert_eq!(
        world.payer.finish_withdrawal(&channel).await.unwrap_err(),
        BatchSettlementError::NoWithdrawalPending(channel.clone())
    );
}
