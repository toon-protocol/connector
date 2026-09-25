//! `EvmBatchSettlementBackend` (ADR 0074, issue #1342) against x402's real
//! `x402BatchSettlement`, Base Sepolia's own bytecode placed at its canonical
//! address on a disposable `anvil`, with Circle's FiatToken v2.2 as the
//! ERC-3009 USDC a payer deposits through `ERC3009DepositCollector`. See
//! `connector_settlement_evm::test_support::x402` for how that chain is
//! stood up.
//!
//! The port's contract suite runs here unmodified; the other tests are the
//! EVM-only rules the suite cannot state for both chains.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use connector_settlement::batch::contract::{
    assert_upholds_the_contract, BatchContractFixture, BoxFuture, ChannelTerms, OpenedChannel,
};
use connector_settlement::batch::{
    AdmissionRefusal, BatchChannelStatus, BatchSettlementBackend, BatchSettlementError,
    ChannelPresentation, EvmChannelConfig, Voucher, VoucherSigner,
};
use connector_settlement::ChannelId;
use connector_settlement_evm::test_support::x402::X402Chain;
use connector_settlement_evm::test_support::{require_anvil, Anvil, DEPLOYER_PRIVATE_KEY};
use connector_settlement_evm::{EvmBatchSettlementBackend, EvmSettlementBackend};
use ethers::signers::{LocalWallet, Signer};
use ethers::types::Address;

/// This binary's own base port for [`Anvil::spawn`]: clear of the other
/// anvil binaries' ranges and of `SolanaValidator`'s, from 19_900.
const ANVIL_BASE_PORT: u16 = 22_100;

/// ADR 0074's default minimum `withdrawDelay`.
const ONE_DAY: u64 = 86_400;

/// Somebody who is not this node, for the seats a channel must give it.
const SOMEONE_ELSE: [u8; 20] = [0xbb; 20];

/// Everything a test needs: the chain, this node's backend, and the client
/// who opens channels on it.
struct Chain {
    /// Held so the chain outlives every closure the suite runs.
    _anvil: Anvil,
    x402: Arc<X402Chain>,
    backend: Arc<EvmBatchSettlementBackend>,
    /// The token this node settles in.
    token: Address,
    /// A token it does not.
    other_token: Address,
    /// The client's funding key.
    payer: LocalWallet,
    /// The client's session key: the `payerAuthorizer` that signs vouchers
    /// (ADR 0074 decision 6).
    session: LocalWallet,
    next_salt: AtomicU64,
    configs: Mutex<HashMap<ChannelId, EvmChannelConfig>>,
}

impl Chain {
    async fn spawn() -> Chain {
        let anvil = Anvil::spawn(ANVIL_BASE_PORT).await;
        let mut x402 = X402Chain::place(&anvil.rpc_url).await;
        let token = x402.deploy_fiat_token().await;
        let other_token = x402.deploy_fiat_token().await;
        let settlement = EvmSettlementBackend::deploy(&anvil.rpc_url, DEPLOYER_PRIVATE_KEY, token)
            .await
            .expect("this node's settlement backend, over the FiatToken");
        let backend = settlement
            .batch_settlement(ONE_DAY)
            .await
            .expect("the batch-settlement backend binds to x402BatchSettlement");

        let payer = LocalWallet::from_bytes(&[0x51; 32]).expect("key");
        let session = LocalWallet::from_bytes(&[0x52; 32]).expect("key");
        // The payer deposits gaslessly; only a withdrawal costs it gas.
        x402.fund_gas(payer.address()).await;
        x402.fund_gas(session.address()).await;
        for token in [token, other_token] {
            x402.mint(token, payer.address(), 1_000_000_000).await;
        }
        Chain {
            _anvil: anvil,
            x402: Arc::new(x402),
            backend: Arc::new(backend),
            token,
            other_token,
            payer,
            session,
            next_salt: AtomicU64::new(1),
            configs: Mutex::new(HashMap::new()),
        }
    }

    fn node(&self) -> [u8; 20] {
        self.backend.own_address().to_fixed_bytes()
    }

    fn salt(&self) -> u64 {
        self.next_salt.fetch_add(1, Ordering::SeqCst)
    }

    /// An admissible config, as the client would build one to pay this node.
    fn config(&self) -> EvmChannelConfig {
        let mut salt = [0u8; 32];
        salt[24..].copy_from_slice(&self.salt().to_be_bytes());
        EvmChannelConfig {
            payer: self.payer.address().to_fixed_bytes(),
            payer_authorizer: self.session.address().to_fixed_bytes(),
            receiver: self.node(),
            receiver_authorizer: self.node(),
            token: self.token.to_fixed_bytes(),
            withdraw_delay: ONE_DAY,
            salt,
        }
    }

    fn config_for(&self, terms: ChannelTerms) -> EvmChannelConfig {
        let base = self.config();
        let seat = if terms.pays_this_node {
            self.node()
        } else {
            SOMEONE_ELSE
        };
        EvmChannelConfig {
            receiver: seat,
            receiver_authorizer: seat,
            token: if terms.in_settled_token {
                self.token.to_fixed_bytes()
            } else {
                self.other_token.to_fixed_bytes()
            },
            withdraw_delay: terms.delay_secs,
            ..base
        }
    }

    /// The payer's first deposit, which creates the channel.
    async fn open(&self, config: EvmChannelConfig, deposit: u128) -> ChannelPresentation {
        self.x402
            .deposit(&self.payer, &config, deposit, self.salt())
            .await;
        let channel = self.x402.channel_id(&config);
        self.configs
            .lock()
            .unwrap()
            .insert(channel.clone(), config.clone());
        ChannelPresentation::Evm { channel, config }
    }

    fn config_of(&self, channel: &ChannelId) -> EvmChannelConfig {
        self.configs.lock().unwrap()[channel].clone()
    }

    fn voucher(&self, signer: &LocalWallet, channel: &ChannelId, amount: u128) -> Voucher {
        Voucher {
            cumulative_amount: amount,
            signature: self.x402.sign_voucher(signer, channel, amount),
        }
    }
}

fn refusal(result: Result<impl std::fmt::Debug, BatchSettlementError>) -> AdmissionRefusal {
    match result {
        Err(BatchSettlementError::NotAdmissible { refusal, .. }) => refusal,
        other => panic!("expected an admission refusal, got {other:?}"),
    }
}

#[tokio::test]
async fn evm_batch_settlement_backend_upholds_the_contract() {
    if !require_anvil() {
        return;
    }
    let chain = Arc::new(Chain::spawn().await);

    assert_upholds_the_contract(|| async {
        let unopened = {
            let config = chain.config();
            ChannelPresentation::Evm {
                channel: chain.x402.channel_id(&config),
                config,
            }
        };
        BatchContractFixture {
            backend: Arc::clone(&chain.backend) as Arc<dyn BatchSettlementBackend>,
            minimum_delay_secs: ONE_DAY,
            open: {
                let chain = Arc::clone(&chain);
                Box::new(move |terms: ChannelTerms| {
                    let chain = Arc::clone(&chain);
                    let opened: BoxFuture<'static, OpenedChannel> = Box::pin(async move {
                        let presentation = chain.open(chain.config_for(terms), terms.deposit).await;
                        OpenedChannel {
                            presentation,
                            voucher_signer: VoucherSigner::Evm(chain.session.address().into()),
                        }
                    });
                    opened
                })
            },
            deposit: {
                let chain = Arc::clone(&chain);
                Box::new(move |channel: &ChannelId, amount: u128| {
                    let chain = Arc::clone(&chain);
                    let channel = channel.clone();
                    let deposited: BoxFuture<'static, ()> = Box::pin(async move {
                        let config = chain.config_of(&channel);
                        chain
                            .x402
                            .deposit(&chain.payer, &config, amount, chain.salt())
                            .await;
                    });
                    deposited
                })
            },
            begin_exit: {
                let chain = Arc::clone(&chain);
                Box::new(move |channel: &ChannelId| {
                    let chain = Arc::clone(&chain);
                    let channel = channel.clone();
                    let exiting: BoxFuture<'static, ()> = Box::pin(async move {
                        let (balance, claimed) = chain.x402.channel(&channel).await;
                        let config = chain.config_of(&channel);
                        chain
                            .x402
                            .initiate_withdraw(&chain.session, &config, balance - claimed)
                            .await;
                    });
                    exiting
                })
            },
            sign: {
                let chain = Arc::clone(&chain);
                Box::new(move |channel: &ChannelId, amount: u128| {
                    chain.x402.sign_voucher(&chain.session, channel, amount)
                })
            },
            unopened,
        }
    })
    .await;
}

/// ADR 0074 decision 2, as amended on 2026-09-25: a channel must name a
/// nonzero `payerAuthorizer`, whatever its payer is. With none, the contract
/// checks a voucher against `payer` through `SignatureChecker`, which asks
/// ERC-1271 of any payer with code -- and an EOA payer can gain code later,
/// by an EIP-7702 delegation, turning every ECDSA voucher this node already
/// accepted into one the contract no longer checks by ECDSA. So an EOA payer
/// with no `payerAuthorizer` is refused as surely as a contract wallet is,
/// and a payer with code that names one is admitted.
///
/// A channel already held is another matter (ADR 0074 decision 5): one
/// accepted before the rule is restored for landing, never re-judged.
#[tokio::test]
async fn a_channel_must_name_a_payer_authorizer() {
    if !require_anvil() {
        return;
    }
    let chain = Chain::spawn().await;
    let unauthorised = |chain: &Chain| EvmChannelConfig {
        payer_authorizer: [0u8; 20],
        ..chain.config()
    };

    // An EOA payer that would sign for itself is refused by the rule's name.
    let eoa = chain.open(unauthorised(&chain), 1_000).await;
    let channel = eoa.channel().clone();
    assert_eq!(
        refusal(chain.backend.admit(eoa.clone()).await),
        AdmissionRefusal::NoPayerAuthorizer,
        "an EOA payer with no payerAuthorizer"
    );

    // Held from before the rule, it is restored all the same, and the
    // payer's own voucher still lands.
    let state = chain.backend.restore(eoa).await.expect("restored");
    assert_eq!(
        state.voucher_signer,
        VoucherSigner::Evm(chain.payer.address().into())
    );
    let state = chain
        .backend
        .land(&channel, chain.voucher(&chain.payer, &channel, 250))
        .await
        .expect("a held voucher lands on a restored channel");
    assert_eq!(state.landed, 250);

    // A payer with code -- an EIP-7702 delegation designator, then an
    // ordinary contract -- is refused without a payerAuthorizer and
    // admitted with one. Its channels are opened while it is still an EOA
    // -- a FiatToken would ask a payer with code for ERC-1271 on the
    // deposit too -- and it gains the code before this node looks.
    for code in [
        format!("0xef0100{}", "11".repeat(20)),
        "0x6080604052".to_string(),
    ] {
        chain.x402.set_code(chain.payer.address(), "0x").await;
        let wallet = chain.open(unauthorised(&chain), 1_000).await;
        let authorised = chain.open(chain.config(), 1_000).await;
        chain.x402.set_code(chain.payer.address(), &code).await;
        assert_eq!(
            refusal(chain.backend.admit(wallet).await),
            AdmissionRefusal::NoPayerAuthorizer,
            "a payer with code {code} and no payerAuthorizer"
        );

        chain
            .backend
            .admit(authorised)
            .await
            .expect("a payer with code that names a payerAuthorizer is admitted");
    }
}

/// ADR 0074 decision 5: `receiverAuthorizer` can refund to the payer what
/// this node has earned and not yet claimed, so a channel that delegates it
/// is refused even when `receiver` is this node -- and each seat is named.
#[tokio::test]
async fn each_seat_that_must_be_this_nodes_is_refused_by_name() {
    if !require_anvil() {
        return;
    }
    let chain = Chain::spawn().await;

    let delegated = chain
        .open(
            EvmChannelConfig {
                receiver_authorizer: SOMEONE_ELSE,
                ..chain.config()
            },
            1_000,
        )
        .await;
    assert_eq!(
        refusal(chain.backend.admit(delegated).await),
        AdmissionRefusal::NotPayableToThisNode {
            field: "receiverAuthorizer"
        }
    );

    let elsewhere = chain
        .open(
            EvmChannelConfig {
                receiver: SOMEONE_ELSE,
                ..chain.config()
            },
            1_000,
        )
        .await;
    assert_eq!(
        refusal(chain.backend.admit(elsewhere).await),
        AdmissionRefusal::NotPayableToThisNode { field: "receiver" }
    );
}

/// ADR 0074 decision 2: the id is recomputed from the presented config, off
/// chain, and a config that hashes elsewhere is refused before anything is
/// read. A Solana presentation is not this backend's.
#[tokio::test]
async fn a_presentation_that_does_not_name_itself_is_refused() {
    if !require_anvil() {
        return;
    }
    let chain = Chain::spawn().await;
    let first = chain.open(chain.config(), 1_000).await;
    let second = chain.open(chain.config(), 1_000).await;
    let (
        ChannelPresentation::Evm { channel, .. },
        ChannelPresentation::Evm {
            channel: derived,
            config,
        },
    ) = (first, second)
    else {
        unreachable!("an EVM presentation");
    };

    let err = chain
        .backend
        .admit(ChannelPresentation::Evm {
            channel: channel.clone(),
            config,
        })
        .await
        .unwrap_err();
    assert_eq!(
        err,
        BatchSettlementError::ChannelIdMismatch {
            presented: channel.clone(),
            derived,
        }
    );
    assert_eq!(
        chain.backend.channel_state(&channel).await.unwrap_err(),
        BatchSettlementError::ChannelNotAdmitted(channel)
    );

    let err = chain
        .backend
        .admit(ChannelPresentation::Solana {
            channel: ChannelId("CHNLxYvVA28MJP9PrFuDXccuoGXAx7jBacfLEkahyGsX".into()),
        })
        .await
        .unwrap_err();
    assert_eq!(
        err,
        BatchSettlementError::WrongChain {
            presented: "solana",
            backend: "evm",
        }
    );
}

/// A voucher the contract would revert on is refused before it is sent, so
/// no gas is spent on it: signed by the wrong key, or not 65 bytes.
#[tokio::test]
async fn a_voucher_the_contract_would_refuse_is_never_submitted() {
    if !require_anvil() {
        return;
    }
    let chain = Chain::spawn().await;
    let presentation = chain.open(chain.config(), 1_000).await;
    let channel = presentation.channel().clone();
    chain.backend.admit(presentation).await.expect("admit");
    let nonce_before = nonce(&chain).await;

    // The payer's funding key is not the signer when a session key is named.
    let by_the_payer = chain.voucher(&chain.payer, &channel, 100);
    let truncated = Voucher {
        cumulative_amount: 100,
        signature: chain.voucher(&chain.session, &channel, 100).signature[..64].to_vec(),
    };
    for voucher in [by_the_payer, truncated] {
        let err = chain.backend.land(&channel, voucher).await.unwrap_err();
        assert!(
            matches!(err, BatchSettlementError::InvalidVoucherSignature(_)),
            "{err:?}"
        );
    }
    assert_eq!(chain.x402.channel(&channel).await, (1_000, 0));
    assert_eq!(nonce(&chain).await, nonce_before, "nothing was sent");
}

/// EVM has no terminal state (the port's `BatchChannelStatus` table): a
/// payer who finalises a withdrawal of everything unclaimed leaves an `Open`
/// channel holding only what was landed, which backs nothing new -- and the
/// value landed before the withdrawal stays landed.
#[tokio::test]
async fn a_finalised_withdrawal_leaves_an_open_channel_backing_nothing_new() {
    if !require_anvil() {
        return;
    }
    let chain = Chain::spawn().await;
    let presentation = chain.open(chain.config(), 1_000).await;
    let channel = presentation.channel().clone();
    chain.backend.admit(presentation).await.expect("admit");
    chain
        .backend
        .land(&channel, chain.voucher(&chain.session, &channel, 300))
        .await
        .expect("land");

    let config = chain.config_of(&channel);
    chain
        .x402
        .initiate_withdraw(&chain.session, &config, 700)
        .await;
    chain.x402.advance_time(ONE_DAY).await;
    chain.x402.finalize_withdraw(&chain.session, &config).await;

    let state = chain.backend.channel_state(&channel).await.expect("state");
    assert_eq!(state.status, BatchChannelStatus::Open);
    assert_eq!(state.landed, 300);
    assert_eq!(state.collateral, 0);
    let err = chain
        .backend
        .land(&channel, chain.voucher(&chain.session, &channel, 301))
        .await
        .unwrap_err();
    assert_eq!(
        err,
        BatchSettlementError::VoucherExceedsDeposit {
            amount: 301,
            deposited: 300,
        }
    );
}

/// The backend refuses to bind on a chain where no `x402BatchSettlement`
/// answers at its fixed address: here, one x402 was never placed on.
#[tokio::test]
async fn binding_where_x402_batch_settlement_is_not_deployed_is_refused() {
    if !require_anvil() {
        return;
    }
    let anvil = Anvil::spawn(ANVIL_BASE_PORT).await;
    let token = EvmSettlementBackend::deploy_mock_token(&anvil.rpc_url, DEPLOYER_PRIVATE_KEY, 0)
        .await
        .expect("a token");
    let settlement = EvmSettlementBackend::deploy(&anvil.rpc_url, DEPLOYER_PRIVATE_KEY, token)
        .await
        .expect("this node's settlement backend");
    let Err(err) = settlement.batch_settlement(ONE_DAY).await else {
        panic!("bound where no x402BatchSettlement is deployed");
    };
    assert!(
        matches!(&err, BatchSettlementError::Backend(message) if message.contains("no x402BatchSettlement")),
        "{err:?}"
    );
}

async fn nonce(chain: &Chain) -> u64 {
    chain.x402.nonce(chain.backend.own_address()).await
}
