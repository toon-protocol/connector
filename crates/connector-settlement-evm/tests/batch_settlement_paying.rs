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
    BatchSettlementBackend, BatchSettlementError, BatchSettlementPayer, ChannelPresentation,
    EvmChannelConfig, EvmReceiverTerms, ReceiverTerms, VoucherSigner,
};
use connector_settlement_evm::test_support::x402::X402Chain;
use connector_settlement_evm::test_support::{
    require_anvil, Anvil, COUNTERPARTY_PRIVATE_KEY, DEPLOYER_PRIVATE_KEY,
};
use connector_settlement_evm::{
    DepositRoute, EvmBatchSettlementBackend, EvmSettlementBackend, RpcTransport,
};
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
            Token::Plain => EvmSettlementBackend::deploy_mock_token(
                &anvil.rpc_url,
                DEPLOYER_PRIVATE_KEY,
                FUNDED,
            )
            .await
            .expect("a token with no EIP-3009, minted to the payer"),
        };
        let node = |key: &'static str| {
            let rpc_url = anvil.rpc_url.clone();
            async move {
                EvmSettlementBackend::deploy(&rpc_url, key, token)
                    .await
                    .expect("a node's settlement backend")
                    .batch_settlement(ONE_DAY)
                    .await
                    .expect("its batch-settlement backend")
            }
        };
        let payer = node(DEPLOYER_PRIVATE_KEY).await;
        let receiver = node(COUNTERPARTY_PRIVATE_KEY).await;
        Peering {
            _anvil: anvil,
            x402: Arc::new(x402),
            token,
            payer: Arc::new(payer),
            receiver: Arc::new(receiver),
        }
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
    let settlement = EvmSettlementBackend::deploy(&anvil.rpc_url, DEPLOYER_PRIVATE_KEY, token)
        .await
        .expect("the payer's settlement backend");
    x402.mint(token, settlement.own_address(), FUNDED).await;
    let receiver = EvmSettlementBackend::deploy(&anvil.rpc_url, COUNTERPARTY_PRIVATE_KEY, token)
        .await
        .expect("the receiver's settlement backend")
        .batch_settlement(ONE_DAY)
        .await
        .expect("its batch-settlement backend");

    let payer_address = format!("{:?}", settlement.own_address());
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
    let payer = EvmSettlementBackend::connect(
        &RpcTransport::direct(&lying.url()).expect("transport"),
        DEPLOYER_PRIVATE_KEY,
        settlement.registry_address(),
        token,
        6,
    )
    .await
    .expect("the payer, over the lying endpoint")
    .batch_settlement(ONE_DAY)
    .await
    .expect("its batch-settlement backend");

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
