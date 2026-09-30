//! The paying half of `EvmBatchSettlementBackend` (ADR 0075 decisions 2 and
//! 3, issue #1374) against x402's real `x402BatchSettlement`, its deposit
//! collectors and Permit2, Base Sepolia's own bytecode placed at their
//! canonical addresses on a disposable `anvil`
//! (`connector_settlement_evm::test_support::x402`).
//!
//! Two nodes, each its own settlement key and its own backend, pay each
//! other as a peering does: the port's paying suite runs here unmodified,
//! once over Circle's FiatToken v2.2 (a deposit through
//! `ERC3009DepositCollector`) and once over a token with no EIP-3009 (a
//! deposit through `Permit2DepositCollector`). The other tests are the
//! EVM-only facts the suite cannot state for both chains.

use std::sync::Arc;

use connector_chain_rpc::{FakeRpc, RpcReply};
use connector_settlement::batch::contract::{
    assert_upholds_the_paying_contract, PayingContractFixture,
};
use connector_settlement::batch::{
    BatchChannelStatus, BatchSettlementBackend, BatchSettlementError, BatchSettlementPayer,
    ChannelPresentation, EvmChannelConfig, EvmReceiverTerms, ReceiverTerms, VoucherSigner,
};
use connector_settlement_evm::test_support::x402::X402Chain;
use connector_settlement_evm::test_support::{
    require_anvil, Anvil, COUNTERPARTY_PRIVATE_KEY, DEPLOYER_PRIVATE_KEY,
};
use connector_settlement_evm::{DepositRoute, EvmBatchSettlementBackend, RpcTransport};
use ethers::signers::{LocalWallet, Signer};
use ethers::types::{Address, Signature, H256};

/// This binary's own base port for [`Anvil::spawn`], clear of the other
/// anvil binaries' ranges.
const ANVIL_BASE_PORT: u16 = 24_100;

/// ADR 0074's default minimum `withdrawDelay`, which the receiver publishes.
const ONE_DAY: u64 = 86_400;

/// What the payer holds of the token before the suite runs.
const FUNDED: u128 = 1_000_000;

/// Which token the two nodes settle in.
#[derive(Clone, Copy)]
enum Token {
    /// Circle's FiatToken v2.2: EIP-3009, so `ERC3009DepositCollector`.
    FiatToken,
    /// The mock ERC-20 with no EIP-3009: `Permit2DepositCollector`.
    Plain,
}

/// A payer node and a receiver node on one chain, in one token.
struct Peering {
    /// Held so the chain outlives every closure the suite runs.
    _anvil: Anvil,
    x402: Arc<X402Chain>,
    token: Address,
    payer: Arc<EvmBatchSettlementBackend>,
    receiver: Arc<EvmBatchSettlementBackend>,
}

impl Peering {
    async fn spawn(token: Token) -> Peering {
        let anvil = Anvil::spawn(ANVIL_BASE_PORT).await;
        let mut x402 = X402Chain::place(&anvil.rpc_url).await;
        let payer_address = LocalWallet::from_bytes(&hex32(DEPLOYER_PRIVATE_KEY))
            .expect("key")
            .address();
        let token = match token {
            Token::FiatToken => {
                let token = x402.deploy_fiat_token().await;
                x402.mint(token, payer_address, FUNDED).await;
                token
            }
            // A token with no EIP-3009, minted to the payer.
            Token::Plain => {
                connector_settlement_evm::test_support::deploy_plain_token(
                    &anvil.rpc_url,
                    DEPLOYER_PRIVATE_KEY,
                    FUNDED,
                )
                .await
            }
        };
        let payer = build_node(&anvil.rpc_url, DEPLOYER_PRIVATE_KEY, token).await;
        let receiver = build_node(&anvil.rpc_url, COUNTERPARTY_PRIVATE_KEY, token).await;
        Peering {
            _anvil: anvil,
            x402: Arc::new(x402),
            token,
            payer: Arc::new(payer),
            receiver: Arc::new(receiver),
        }
    }

    /// A node with settlement key `key` on this chain, built as a booting
    /// node builds it.
    async fn node(&self, key: &str) -> EvmBatchSettlementBackend {
        build_node(&self._anvil.rpc_url, key, self.token).await
    }

    /// What the receiver publishes in its self-description's
    /// `batchSettlements` entry.
    fn terms(&self) -> ReceiverTerms {
        ReceiverTerms::Evm(EvmReceiverTerms {
            receiver: self.receiver.own_address().to_fixed_bytes(),
            token: self.token.to_fixed_bytes(),
            min_withdraw_delay_secs: self.receiver.min_withdraw_delay_secs(),
        })
    }

    async fn payer_balance(&self) -> u128 {
        self.x402
            .balance_of(self.token, self.payer.own_address())
            .await
    }

    /// A real channel toward the receiver that the payer did not open: a
    /// client's.
    async fn clients_channel(&self) -> connector_settlement::ChannelId {
        let client = LocalWallet::from_bytes(&[0x61; 32]).expect("key");
        self.x402.fund_gas(client.address()).await;
        let config = EvmChannelConfig {
            payer: client.address().to_fixed_bytes(),
            payer_authorizer: client.address().to_fixed_bytes(),
            receiver: self.receiver.own_address().to_fixed_bytes(),
            receiver_authorizer: self.receiver.own_address().to_fixed_bytes(),
            token: self.token.to_fixed_bytes(),
            withdraw_delay: ONE_DAY,
            salt: [0x62; 32],
        };
        self.x402.channel_id(&config)
    }
}

async fn build_node(rpc_url: &str, key: &str, token: Address) -> EvmBatchSettlementBackend {
    EvmBatchSettlementBackend::connect(
        &RpcTransport::direct(rpc_url).expect("transport"),
        key,
        token,
        6,
        ONE_DAY,
    )
    .await
    .expect("a node's batch-settlement backend")
}

fn hex32(text: &str) -> [u8; 32] {
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[2 * i..2 * i + 2], 16).expect("hex");
    }
    out
}

async fn upholds_the_paying_contract(token: Token) {
    let peering = Arc::new(Peering::spawn(token).await);
    let not_outbound = peering.clients_channel().await;
    assert_upholds_the_paying_contract(|| async {
        PayingContractFixture {
            payer: Arc::clone(&peering.payer) as Arc<dyn BatchSettlementPayer>,
            receiver: Arc::clone(&peering.receiver) as Arc<dyn BatchSettlementBackend>,
            terms: peering.terms(),
            payer_balance: {
                let peering = Arc::clone(&peering);
                Box::new(move || {
                    let peering = Arc::clone(&peering);
                    Box::pin(async move { peering.payer_balance().await })
                })
            },
            payer_settlement_key: VoucherSigner::Evm(peering.payer.own_address().into()),
            let_delay_pass: {
                let peering = Arc::clone(&peering);
                Box::new(move || {
                    let peering = Arc::clone(&peering);
                    Box::pin(async move { peering.x402.advance_time(ONE_DAY).await })
                })
            },
            not_outbound,
            // A node restarting: the same settlement key over the same
            // chain, built afresh as a booting node builds it, remembering
            // nothing the earlier backend did.
            restart: {
                let peering = Arc::clone(&peering);
                Box::new(move || {
                    let peering = Arc::clone(&peering);
                    Box::pin(async move {
                        Arc::new(peering.node(DEPLOYER_PRIVATE_KEY).await)
                            as Arc<dyn BatchSettlementPayer>
                    })
                })
            },
        }
    })
    .await;
}

#[tokio::test]
async fn evm_batch_settlement_payer_upholds_the_paying_contract_over_erc3009() {
    if !require_anvil() {
        return;
    }
    upholds_the_paying_contract(Token::FiatToken).await;
}

#[tokio::test]
async fn evm_batch_settlement_payer_upholds_the_paying_contract_over_permit2() {
    if !require_anvil() {
        return;
    }
    upholds_the_paying_contract(Token::Plain).await;
}

/// ADR 0075 decision 8's hardest crash window: the opening deposit is in
/// flight from a process that died, and the restarted node sends the same
/// record again before it lands. Two nodes over the one key send it at
/// once, each seeing nothing on chain. Both answer with the one channel,
/// and the payer's account pays the opening deposit exactly once: the
/// authorisation is the config's own, and the token spends it once, so
/// whichever sending lands second reverts.
///
/// Each route is raced several times over one chain. The first race also
/// races Permit2's one-time `approve`, so each node sends two writes and
/// can take the nonce the other has just re-read (#1371); the later races
/// race the deposit alone. The deterministic form of that collision is
/// `send`'s own
/// `a_nonce_taken_again_after_the_reseed_is_reseeded_again_and_the_write_lands_once`.
#[tokio::test]
async fn an_open_sent_twice_at_once_deposits_once_for_both_token_routes() {
    if !require_anvil() {
        return;
    }
    // One race with the `approve` in it, then three of the deposit alone,
    // at well under a second each.
    const RACES: usize = 4;
    for token in [Token::FiatToken, Token::Plain] {
        let peering = Peering::spawn(token).await;
        for race in 0..RACES {
            let before = peering.payer_balance().await;
            let record = peering
                .payer
                .prepare_open(peering.terms(), 1_000)
                .await
                .expect("prepare");
            let crashed = peering.node(DEPLOYER_PRIVATE_KEY).await;
            let restarted = peering.node(DEPLOYER_PRIVATE_KEY).await;
            let (first, second) = tokio::join!(
                crashed.open_prepared(&record),
                restarted.open_prepared(&record)
            );
            let first = first.unwrap_or_else(|error| panic!("race {race}: first: {error:?}"));
            let second = second.unwrap_or_else(|error| panic!("race {race}: second: {error:?}"));
            assert_eq!(first.presentation, record.presentation());
            assert_eq!(second.presentation, record.presentation());
            assert_eq!(
                peering.payer_balance().await,
                before - 1_000,
                "race {race}: one record, one opening deposit"
            );
            assert_eq!(
                restarted
                    .outbound_state(record.channel())
                    .await
                    .expect("state")
                    .on_chain
                    .collateral,
                1_000
            );
        }
    }
}

/// ADR 0075 decision 3: the config this node builds names its settlement
/// address as both `payer` and `payerAuthorizer`, the counterparty in both
/// receiving seats, the shared token, the counterparty's minimum delay and
/// a fresh salt -- and the channel it names is the one the contract's own
/// `getChannelId` computes.
#[tokio::test]
async fn the_config_an_open_builds_names_the_settlement_key_as_payer_and_authorizer() {
    if !require_anvil() {
        return;
    }
    let peering = Peering::spawn(Token::FiatToken).await;
    let own = peering.payer.own_address().to_fixed_bytes();
    let counterparty = peering.receiver.own_address().to_fixed_bytes();

    let mut salts = Vec::new();
    for _ in 0..2 {
        let opened = peering
            .payer
            .open(peering.terms(), 1_000)
            .await
            .expect("open");
        let ChannelPresentation::Evm { channel, config } = opened.presentation else {
            panic!("an EVM open presents an EVM channel");
        };
        assert_eq!(config.payer, own);
        assert_eq!(config.payer_authorizer, own, "payerAuthorizer == payer");
        assert_eq!(config.receiver, counterparty);
        assert_eq!(config.receiver_authorizer, counterparty);
        assert_eq!(config.token, peering.token.to_fixed_bytes());
        assert_eq!(config.withdraw_delay, ONE_DAY);
        assert_eq!(opened.voucher_signer, VoucherSigner::Evm(own));
        assert_eq!(
            peering.x402.contract_channel_id(&config).await,
            connector_settlement_evm::test_support::x402::parse_channel(&channel),
            "the chain agrees on the channel's id"
        );
        salts.push(config.salt);
    }
    assert_ne!(salts[0], salts[1], "each open draws a fresh salt");
}

/// A counterparty whose minimum is below the contract's own floor still
/// gets a channel: this node raises the delay to 15 minutes, which is at
/// least what was asked and a delay the contract accepts.
#[tokio::test]
async fn a_minimum_below_the_contracts_floor_is_raised_to_it() {
    if !require_anvil() {
        return;
    }
    let peering = Peering::spawn(Token::FiatToken).await;
    let ReceiverTerms::Evm(terms) = peering.terms() else {
        unreachable!()
    };
    let opened = peering
        .payer
        .open(
            ReceiverTerms::Evm(EvmReceiverTerms {
                min_withdraw_delay_secs: 60,
                ..terms
            }),
            1_000,
        )
        .await
        .expect("open");
    let ChannelPresentation::Evm { config, .. } = opened.presentation else {
        panic!("an EVM presentation");
    };
    assert_eq!(config.withdraw_delay, 900);
}

/// ADR 0075 decision 3: a voucher this node signs is the contract's
/// `getVoucherDigest` signed by its settlement key, and this node's own
/// receiving half accepts it.
#[tokio::test]
async fn a_signed_voucher_is_the_contracts_digest_signed_by_the_settlement_key() {
    if !require_anvil() {
        return;
    }
    let peering = Peering::spawn(Token::FiatToken).await;
    let opened = peering
        .payer
        .open(peering.terms(), 1_000)
        .await
        .expect("open");
    let channel = opened.presentation.channel().clone();
    let voucher = peering
        .payer
        .sign_voucher(&channel, 250)
        .await
        .expect("sign");

    let digest = peering
        .x402
        .contract_voucher_digest(
            connector_settlement_evm::test_support::x402::parse_channel(&channel),
            250,
        )
        .await;
    let signature = Signature::try_from(voucher.signature.as_slice()).expect("65 bytes");
    assert_eq!(
        signature.recover(H256::from(digest)).expect("recover"),
        peering.payer.own_address(),
        "the settlement key signed the contract's own digest"
    );

    // This node's receiving half is the same code the counterparty runs.
    peering
        .payer
        .admit(opened.presentation.clone())
        .await
        .expect_err("a channel paying the counterparty is not this node's to admit");
    let state = peering
        .receiver
        .admit(opened.presentation)
        .await
        .expect("admitted");
    assert_eq!(state.voucher_signer, opened.voucher_signer);
    let landed = peering
        .receiver
        .land(&channel, voucher)
        .await
        .expect("the receiving half lands what the paying half signed");
    assert_eq!(landed.landed, 250);
}

/// A token with EIP-3009 is deposited through `ERC3009DepositCollector`
/// with no approval; one without goes through Permit2, approved once and
/// never again.
#[tokio::test]
async fn permit2_is_approved_once_and_erc3009_needs_no_approval() {
    if !require_anvil() {
        return;
    }
    let fiat = Peering::spawn(Token::FiatToken).await;
    assert_eq!(
        fiat.payer.deposit_route().await.expect("route"),
        DepositRoute::Erc3009
    );
    let before = fiat.x402.nonce(fiat.payer.own_address()).await;
    let opened = fiat.payer.open(fiat.terms(), 1_000).await.expect("open");
    fiat.payer
        .top_up(opened.presentation.channel(), 1)
        .await
        .expect("top up");
    assert_eq!(
        fiat.x402.nonce(fiat.payer.own_address()).await - before,
        2,
        "a deposit and a top-up, and nothing else"
    );

    let plain = Peering::spawn(Token::Plain).await;
    assert_eq!(
        plain.payer.deposit_route().await.expect("route"),
        DepositRoute::Permit2
    );
    let before = plain.x402.nonce(plain.payer.own_address()).await;
    let opened = plain.payer.open(plain.terms(), 1_000).await.expect("open");
    assert_eq!(
        plain.x402.nonce(plain.payer.own_address()).await - before,
        2,
        "the first deposit approves Permit2, then deposits"
    );
    plain
        .payer
        .top_up(opened.presentation.channel(), 1)
        .await
        .expect("top up");
    plain.payer.open(plain.terms(), 1_000).await.expect("open");
    assert_eq!(
        plain.x402.nonce(plain.payer.own_address()).await - before,
        4,
        "every later deposit is one transaction"
    );
}

/// A withdrawal with nothing landed returns the whole deposit to the payer
/// after the delay, and leaves an `Open` channel holding nothing.
#[tokio::test]
async fn a_withdrawal_with_nothing_landed_returns_the_whole_deposit() {
    if !require_anvil() {
        return;
    }
    let peering = Peering::spawn(Token::FiatToken).await;
    let funded = peering.payer_balance().await;
    let opened = peering
        .payer
        .open(peering.terms(), 1_000)
        .await
        .expect("open");
    let channel = opened.presentation.channel().clone();
    peering
        .payer
        .start_withdrawal(&channel)
        .await
        .expect("start");
    peering.x402.advance_time(ONE_DAY).await;
    let state = peering
        .payer
        .finish_withdrawal(&channel)
        .await
        .expect("finish");
    assert_eq!(peering.payer_balance().await, funded);
    assert_eq!(state.on_chain.landed, 0);
    assert_eq!(state.on_chain.collateral, 0);
    assert_eq!(
        peering.x402.channel(&channel).await,
        (0, 0),
        "nothing is left in escrow"
    );
}

/// An open whose deposit never lands records nothing and spends nothing:
/// a deposit above the payer's balance makes `receiveWithAuthorization`
/// revert, and a deposit of zero is refused before it is signed.
#[tokio::test]
async fn an_open_whose_deposit_does_not_land_records_nothing() {
    if !require_anvil() {
        return;
    }
    let peering = Peering::spawn(Token::FiatToken).await;
    let funded = peering.payer_balance().await;
    let nonce = peering.x402.nonce(peering.payer.own_address()).await;
    for deposit in [funded + 1, 0] {
        let err = peering
            .payer
            .open(peering.terms(), deposit)
            .await
            .expect_err("a deposit that cannot land opens nothing");
        assert!(matches!(err, BatchSettlementError::Backend(_)), "{err:?}");
    }
    assert_eq!(peering.payer_balance().await, funded);
    assert_eq!(
        peering.x402.nonce(peering.payer.own_address()).await,
        nonce,
        "nothing was sent"
    );
}

/// A deposit that lands though its confirmation says otherwise -- here a
/// receipt read that lies, reporting a revert -- is still this node's
/// channel: it is recorded before the deposit is sent and kept once the
/// chain shows a balance, so it can be read, signed on and withdrawn from
/// rather than stranded.
#[tokio::test]
async fn an_opening_deposit_that_lands_behind_a_lost_confirmation_is_kept() {
    if !require_anvil() {
        return;
    }
    let anvil = Anvil::spawn(ANVIL_BASE_PORT).await;
    let mut x402 = X402Chain::place(&anvil.rpc_url).await;
    let token = x402.deploy_fiat_token().await;
    let payer_own = LocalWallet::from_bytes(&hex32(DEPLOYER_PRIVATE_KEY))
        .expect("key")
        .address();
    x402.mint(token, payer_own, FUNDED).await;
    let receiver = build_node(&anvil.rpc_url, COUNTERPARTY_PRIVATE_KEY, token).await;

    let payer_address = format!("{payer_own:?}");
    let lying = FakeRpc::spawn_in_front_of(&anvil.rpc_url, move |call| {
        if call.method != "eth_getTransactionReceipt" {
            return RpcReply::Forward;
        }
        RpcReply::Result(serde_json::json!({
            "transactionHash": call.params[0],
            "transactionIndex": "0x0",
            "from": payer_address,
            "cumulativeGasUsed": "0x0",
            "logs": [],
            "logsBloom": format!("0x{}", "00".repeat(256)),
            "status": "0x0",
        }))
    })
    .await;
    let payer = EvmBatchSettlementBackend::connect(
        &RpcTransport::direct(&lying.url()).expect("transport"),
        DEPLOYER_PRIVATE_KEY,
        token,
        6,
        ONE_DAY,
    )
    .await
    .expect("the payer, over the lying endpoint");

    let opened = payer
        .open(
            ReceiverTerms::Evm(EvmReceiverTerms {
                receiver: receiver.own_address().to_fixed_bytes(),
                token: token.to_fixed_bytes(),
                min_withdraw_delay_secs: ONE_DAY,
            }),
            1_000,
        )
        .await
        .expect("the deposit landed, so the open did");
    assert!(
        lying.count("eth_getTransactionReceipt") > 0,
        "the deposit's confirmation was read, and lied about"
    );
    let channel = opened.presentation.channel().clone();
    let state = payer
        .outbound_state(&channel)
        .await
        .expect("its own channel");
    assert_eq!(state.on_chain.collateral, 1_000);
    payer
        .sign_voucher(&channel, 1_000)
        .await
        .expect("the landed deposit backs a voucher");
}

/// Issue #1413: a node that crashes with `initiateWithdraw` still in the
/// mempool, restarts, and re-reads the channel as not withdrawing yet. Its
/// own send is then re-seeded behind the in-flight one on the same key
/// (#1412), mines after it and reverts. That refusal must not surface as a
/// `Backend` error for a withdrawal that did happen: the port's contract
/// suite already says a second sequential `start_withdrawal` returns `Ok`
/// with the state it finds, so a raced one must too.
///
/// Modelled the same way `an_open_sent_twice_at_once_deposits_once_for_both_token_routes`
/// models #1371: two nodes on one settlement key, holding the same outbound
/// record, call `start_withdrawal` at once. Raced several times so a pass is
/// not one lucky interleaving.
#[tokio::test]
async fn concurrent_start_withdrawal_ends_with_one_withdrawal_pending() {
    if !require_anvil() {
        return;
    }
    const RACES: usize = 5;
    let peering = Peering::spawn(Token::FiatToken).await;
    for race in 0..RACES {
        let record = peering
            .payer
            .prepare_open(peering.terms(), 1_000)
            .await
            .expect("prepare");
        peering
            .payer
            .open_prepared(&record)
            .await
            .expect("open the channel two nodes will race a withdrawal on");
        let channel = record.channel().clone();

        // Two nodes on the payer's settlement key, each restored onto the
        // same on-chain channel as a restarted process restores it.
        let first = peering.node(DEPLOYER_PRIVATE_KEY).await;
        first
            .restore_outbound(&record, 0)
            .await
            .expect("restore onto the first node");
        let second = peering.node(DEPLOYER_PRIVATE_KEY).await;
        second
            .restore_outbound(&record, 0)
            .await
            .expect("restore onto the second node");

        let (first_result, second_result) = tokio::join!(
            first.start_withdrawal(&channel),
            second.start_withdrawal(&channel)
        );
        let first_state =
            first_result.unwrap_or_else(|error| panic!("race {race}: first: {error:?}"));
        let second_state =
            second_result.unwrap_or_else(|error| panic!("race {race}: second: {error:?}"));
        assert_eq!(
            first_state.on_chain.status,
            BatchChannelStatus::Withdrawing,
            "race {race}"
        );
        assert_eq!(
            second_state.on_chain.status,
            BatchChannelStatus::Withdrawing,
            "race {race}"
        );

        let (amount, initiated_at) = peering.x402.pending_withdrawal(&channel).await;
        assert_eq!(
            amount, 1_000,
            "race {race}: one withdrawal, for the whole deposit"
        );
        assert_ne!(
            initiated_at, 0,
            "race {race}: exactly one withdrawal is pending"
        );
    }
}

/// The same crash window as above, past the withdraw delay. Either the
/// loser's `finalizeWithdraw` is re-seeded behind the winner's, mines after
/// it and reverts against a channel with nothing left pending -- and the
/// port's contract suite already says a second sequential
/// `finish_withdrawal` there answers `NoWithdrawalPending`, so a raced one
/// must answer the same way rather than a spurious `Backend` error -- or
/// both nodes signed the identical transaction and both wait on its one
/// receipt, so both report the same state (#1433). Either way the payer is
/// paid back exactly once.
#[tokio::test]
async fn concurrent_finish_withdrawal_pays_the_winner_once() {
    if !require_anvil() {
        return;
    }
    const RACES: usize = 5;
    let peering = Peering::spawn(Token::FiatToken).await;
    for race in 0..RACES {
        let record = peering
            .payer
            .prepare_open(peering.terms(), 1_000)
            .await
            .expect("prepare");
        peering
            .payer
            .open_prepared(&record)
            .await
            .expect("open the channel two nodes will race a withdrawal on");
        let channel = record.channel().clone();
        peering
            .payer
            .start_withdrawal(&channel)
            .await
            .expect("start the withdrawal both nodes will race finishing");
        peering.x402.advance_time(ONE_DAY).await;

        let first = peering.node(DEPLOYER_PRIVATE_KEY).await;
        first
            .restore_outbound(&record, 0)
            .await
            .expect("restore onto the first node");
        let second = peering.node(DEPLOYER_PRIVATE_KEY).await;
        second
            .restore_outbound(&record, 0)
            .await
            .expect("restore onto the second node");

        let before = peering.payer_balance().await;
        let (first_result, second_result) = tokio::join!(
            first.finish_withdrawal(&channel),
            second.finish_withdrawal(&channel)
        );
        // Two nodes over one key can send the identical signed
        // `finalizeWithdraw` (same nonce, same bytes): the node answers the
        // second "already known", `send` counts that as a write sent, and
        // both wait on the one transaction, so both succeed. Otherwise the
        // loser's send was re-seeded behind the winner's and reverts, and it
        // must answer what a second sequential call would. Either way the
        // payer is paid back exactly once (asserted below).
        let winner = match (first_result, second_result) {
            (Ok(state), Ok(other)) => {
                assert_eq!(
                    state, other,
                    "race {race}: two winners on one transaction report one outcome"
                );
                state
            }
            (Ok(state), Err(error)) | (Err(error), Ok(state)) => {
                assert_eq!(
                    error,
                    BatchSettlementError::NoWithdrawalPending(channel.clone()),
                    "race {race}: the loser sees exactly what a second sequential call would"
                );
                state
            }
            other => panic!("race {race}: expected at least one winner, got {other:?}"),
        };
        assert_eq!(
            winner.on_chain.status,
            BatchChannelStatus::Open,
            "race {race}"
        );
        assert_eq!(winner.on_chain.collateral, 0, "race {race}");
        assert_eq!(
            peering.payer_balance().await - before,
            1_000,
            "race {race}: the payer's balance rises exactly once, by balance - totalClaimed"
        );
    }
}

/// The re-read after a failed send is not a licence to swallow every
/// failure: a refusal for the write's own reasons, with the chain never
/// touched, must still surface as an error on both `start_withdrawal` and
/// `finish_withdrawal`. Modelled on `send.rs`'s own fault injection
/// (ADR 0007): a `FakeRpc` in front of a real anvil refuses every
/// `eth_sendRawTransaction` outright, so nothing this node sends ever
/// lands and the chain never reaches either write's target state.
#[tokio::test]
async fn a_send_refused_for_its_own_reasons_still_errors() {
    if !require_anvil() {
        return;
    }
    let peering = Peering::spawn(Token::FiatToken).await;
    let record = peering
        .payer
        .prepare_open(peering.terms(), 1_000)
        .await
        .expect("prepare");
    peering.payer.open_prepared(&record).await.expect("open");
    let channel = record.channel().clone();

    let refused = FakeRpc::spawn_in_front_of(&peering._anvil.rpc_url, |call| {
        if call.method == "eth_sendRawTransaction" {
            return RpcReply::Error {
                code: -32000,
                message: "execution reverted: refused for the test".to_string(),
            };
        }
        RpcReply::Forward
    })
    .await;
    let faulty = build_node(&refused.url(), DEPLOYER_PRIVATE_KEY, peering.token).await;
    faulty
        .restore_outbound(&record, 0)
        .await
        .expect("restore onto the faulty node");

    let error = faulty
        .start_withdrawal(&channel)
        .await
        .expect_err("a refusal that never touches the chain is not a race won");
    assert!(
        matches!(error, BatchSettlementError::Backend(_)),
        "{error:?}"
    );
    assert_eq!(
        peering.x402.pending_withdrawal(&channel).await,
        (0, 0),
        "nothing was ever initiated"
    );

    // A healthy node starts the withdrawal for real, so `finish_withdrawal`
    // on the faulty node has a real pending withdrawal to fail finishing.
    peering
        .payer
        .start_withdrawal(&channel)
        .await
        .expect("start for real, off the faulty endpoint");
    peering.x402.advance_time(ONE_DAY).await;

    let error = faulty
        .finish_withdrawal(&channel)
        .await
        .expect_err("a refusal that never touches the chain is not a race won");
    assert!(
        matches!(error, BatchSettlementError::Backend(_)),
        "{error:?}"
    );
    assert_ne!(
        peering.x402.pending_withdrawal(&channel).await.1,
        0,
        "the withdrawal this node started is still pending -- the faulty send never landed"
    );
}
