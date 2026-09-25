//! x402's batch-settlement contracts on a disposable `anvil` (ADR 0074,
//! issue #1342), placed exactly as toon-protocol/infra's
//! `sandbox/scripts/seed-x402.sh` places them, from the committed bytecode
//! in `contracts/x402/` (`contracts/x402/PROVENANCE.md` says where each file
//! came from).
//!
//! - `x402BatchSettlement` and `ERC3009DepositCollector` go to their
//!   canonical CREATE2 addresses by `anvil_setCode`. Both are Base Sepolia's
//!   runtime code, and neither needs storage: the contract's only
//!   constructor state is OpenZeppelin `EIP712`'s, which rebuilds its
//!   separator when `block.chainid` differs from the one it cached, and the
//!   collector's one immutable is the contract's canonical address.
//! - USDC is Circle's FiatToken v2.2, **deployed** from Base Sepolia's own
//!   creation code (implementation, then proxy) and initialised, because a
//!   FiatToken's initialisers write the state that makes it work. It is what
//!   gives a deposit ERC-3009's `receiveWithAuthorization`, so a payer
//!   deposits with no gas of its own, through a relayer, as on Base. It links
//!   Circle's `SignatureChecker` library at `0xbA3b…7DA6`, which is placed
//!   there first: the library's call guard compares its own address to that
//!   one.
//!
//! This is the **client's** side of a channel, which is why it is a fixture
//! and not the backend: the backend never opens, funds or withdraws.

use std::sync::Arc;

use connector_settlement::batch::EvmChannelConfig;
use connector_settlement::ChannelId;
use connector_signer::{
    evm_batch_channel_id, evm_voucher_digest, BatchSettlementDomain, X402_BATCH_SETTLEMENT_ADDRESS,
};
use ethers::abi::{encode, Token};
use ethers::contract::abigen;
use ethers::middleware::SignerMiddleware;
use ethers::providers::{Http, Middleware, Provider};
use ethers::signers::{LocalWallet, Signer};
use ethers::types::transaction::eip2718::TypedTransaction;
use ethers::types::{Address, Bytes, TransactionReceipt, TransactionRequest, H256, U256};
use ethers::utils::keccak256;

use crate::batch_settlement::{chain_config, signer_config};
use crate::bindings::x402_batch_settlement::X402BatchSettlement;

abigen!(
    FiatToken,
    r#"[
        function initialize(string tokenName, string tokenSymbol, string tokenCurrency, uint8 tokenDecimals, address newMasterMinter, address newPauser, address newBlacklister, address newOwner)
        function initializeV2(string newName)
        function initializeV2_1(address lostAndFound)
        function initializeV2_2(address[] accountsToBlacklist, string newSymbol)
        function configureMinter(address minter, uint256 minterAllowedAmount) returns (bool)
        function mint(address to, uint256 amount) returns (bool)
        function DOMAIN_SEPARATOR() view returns (bytes32)
        function balanceOf(address account) view returns (uint256)
        function version() view returns (string)
    ]"#
);

/// Circle's `SignatureChecker` library, at the Base Sepolia address FiatToken
/// v2.2's creation code links: `0xbA3b60c21e28C41df4bABd90f228e1D368627DA6`.
pub fn signature_checker_address() -> Address {
    "0xbA3b60c21e28C41df4bABd90f228e1D368627DA6"
        .parse()
        .expect("a literal address")
}

/// `ERC3009DepositCollector` at `0x4020806089470a89826cB9fB1f4059150b550004`.
pub fn erc3009_deposit_collector_address() -> Address {
    "0x4020806089470a89826cB9fB1f4059150b550004"
        .parse()
        .expect("a literal address")
}

/// `x402BatchSettlement` at its canonical address.
pub fn batch_settlement_address() -> Address {
    Address::from(X402_BATCH_SETTLEMENT_ADDRESS)
}

const BATCH_SETTLEMENT_RUNTIME: &str =
    include_str!("../../contracts/x402/x402BatchSettlement.runtime.hex");
const ERC3009_DEPOSIT_COLLECTOR_RUNTIME: &str =
    include_str!("../../contracts/x402/ERC3009DepositCollector.runtime.hex");
const SIGNATURE_CHECKER_RUNTIME: &str =
    include_str!("../../contracts/x402/SignatureChecker.runtime.hex");
const FIAT_TOKEN_V2_2_CREATION: &str =
    include_str!("../../contracts/x402/FiatTokenV2_2.creation.hex");
const FIAT_TOKEN_PROXY_CREATION: &str =
    include_str!("../../contracts/x402/FiatTokenProxy.creation.hex");

/// EIP-3009's `ReceiveWithAuthorization` type, the struct a FiatToken
/// deposit authorisation is signed over.
const RECEIVE_WITH_AUTHORIZATION_TYPE: &[u8] = b"ReceiveWithAuthorization(address from,address to,uint256 value,uint256 validAfter,uint256 validBefore,bytes32 nonce)";

/// The fixture's own keys, each funded by `anvil_setBalance` so none spends
/// a genesis account's nonce. Deterministic, and never anything but test
/// keys on a disposable chain.
const TOKEN_ADMIN_KEY: [u8; 32] = [0x41; 32];
const TOKEN_OWNER_KEY: [u8; 32] = [0x42; 32];
const RELAYER_KEY: [u8; 32] = [0x43; 32];

type Client = SignerMiddleware<Arc<Provider<Http>>, LocalWallet>;

/// x402's contracts and an ERC-3009 USDC on one `anvil`, and the client-side
/// transactions a payer makes against them.
pub struct X402Chain {
    provider: Arc<Provider<Http>>,
    chain_id: u64,
    /// Proxy admin of every FiatToken this fixture deploys. A transparent
    /// proxy refuses every token call from its admin, so the token's roles
    /// are [`owner`](Self::owner)'s.
    admin: Client,
    /// Owner, master minter, minter, pauser and blacklister of every token.
    owner: Client,
    /// Sends deposits on a payer's behalf, as an x402 facilitator does: an
    /// ERC-3009 deposit is the payer's signature and someone else's gas.
    relayer: Client,
    implementation: Option<Address>,
}

impl X402Chain {
    /// Place x402's contracts on the chain at `rpc_url` and fund the
    /// fixture's own keys. Panics on any failure: this is a test fixture,
    /// and a chain it could not set up is not one a test can say anything
    /// about.
    pub async fn place(rpc_url: &str) -> X402Chain {
        // ethers polls a pending transaction every 7s by default; anvil
        // mines on send, so a fixture transaction is waited on at 50ms.
        let provider = Arc::new(
            Provider::<Http>::try_from(rpc_url)
                .expect("build provider")
                .interval(std::time::Duration::from_millis(50)),
        );
        let chain_id = provider.get_chainid().await.expect("chain id").as_u64();
        let client = |key: [u8; 32]| {
            let wallet = LocalWallet::from_bytes(&key)
                .expect("a valid key")
                .with_chain_id(chain_id);
            SignerMiddleware::new(Arc::clone(&provider), wallet)
        };
        let chain = X402Chain {
            admin: client(TOKEN_ADMIN_KEY),
            owner: client(TOKEN_OWNER_KEY),
            relayer: client(RELAYER_KEY),
            provider,
            chain_id,
            implementation: None,
        };
        for address in [
            chain.admin.address(),
            chain.owner.address(),
            chain.relayer.address(),
        ] {
            chain.fund_gas(address).await;
        }
        chain
            .set_code(signature_checker_address(), SIGNATURE_CHECKER_RUNTIME)
            .await;
        chain
            .set_code(batch_settlement_address(), BATCH_SETTLEMENT_RUNTIME)
            .await;
        chain
            .set_code(
                erc3009_deposit_collector_address(),
                ERC3009_DEPOSIT_COLLECTOR_RUNTIME,
            )
            .await;
        chain
    }

    /// The chain's id, as the chain reports it.
    pub fn chain_id(&self) -> u64 {
        self.chain_id
    }

    /// The EIP-712 domain of `x402BatchSettlement` on this chain.
    pub fn domain(&self) -> BatchSettlementDomain {
        BatchSettlementDomain::x402(self.chain_id)
    }

    /// Give `address` 100 ETH for gas, spending nobody's nonce.
    pub async fn fund_gas(&self, address: Address) {
        let _: serde_json::Value = self
            .provider
            .request(
                "anvil_setBalance",
                (address, U256::from(100u64) * U256::exp10(18)),
            )
            .await
            .expect("anvil_setBalance");
    }

    /// Put `code` (hex, with or without `0x`) at `address`.
    pub async fn set_code(&self, address: Address, code: &str) {
        let code = code.trim();
        let code = if code.starts_with("0x") {
            code.to_string()
        } else {
            format!("0x{code}")
        };
        let _: serde_json::Value = self
            .provider
            .request("anvil_setCode", (address, code))
            .await
            .expect("anvil_setCode");
    }

    /// Deploy and initialise a fresh Circle FiatToken v2.2 behind its own
    /// proxy, 6 decimals, named as Base Sepolia's USDC is ("USDC", version
    /// "2") so ERC-3009 signs over the same domain shape. Each call is a
    /// distinct token; the implementation is deployed once and shared.
    pub async fn deploy_fiat_token(&mut self) -> Address {
        let implementation = match self.implementation {
            Some(implementation) => implementation,
            None => {
                let implementation = deploy(&self.admin, FIAT_TOKEN_V2_2_CREATION, &[]).await;
                self.implementation = Some(implementation);
                implementation
            }
        };
        let proxy = deploy(
            &self.admin,
            FIAT_TOKEN_PROXY_CREATION,
            &encode(&[Token::Address(implementation)]),
        )
        .await;
        let owner = self.owner.address();
        let token = FiatToken::new(proxy, Arc::new(self.owner.clone()));
        send(
            &self.owner,
            token
                .initialize(
                    "USDC".into(),
                    "USDC".into(),
                    "USD".into(),
                    6,
                    owner,
                    owner,
                    owner,
                    owner,
                )
                .tx,
        )
        .await;
        send(&self.owner, token.initialize_v2("USDC".into()).tx).await;
        send(&self.owner, token.initialize_v2_1(owner).tx).await;
        send(
            &self.owner,
            token.initialize_v2_2(Vec::new(), "USDC".into()).tx,
        )
        .await;
        send(&self.owner, token.configure_minter(owner, U256::MAX).tx).await;
        proxy
    }

    /// Mint `amount` of `token` to `to`.
    pub async fn mint(&self, token: Address, to: Address, amount: u128) {
        let contract = FiatToken::new(token, Arc::new(self.owner.clone()));
        send(&self.owner, contract.mint(to, U256::from(amount)).tx).await;
    }

    /// `token`'s balance of `owner`.
    pub async fn balance_of(&self, token: Address, owner: Address) -> u128 {
        FiatToken::new(token, Arc::clone(&self.provider))
            .balance_of(owner)
            .call()
            .await
            .expect("balanceOf")
            .as_u128()
    }

    /// The channel `config` names on this chain, as the backend spells it.
    pub fn channel_id(&self, config: &EvmChannelConfig) -> ChannelId {
        crate::channel_id::format_channel_id(evm_batch_channel_id(
            &self.domain(),
            &signer_config(config),
        ))
    }

    /// The payer deposits `amount` into `config`'s channel through
    /// `ERC3009DepositCollector`: `payer` signs a `receiveWithAuthorization`
    /// to the collector, and the relayer sends `deposit` and pays its gas.
    /// The first deposit creates the channel. `collector_salt` must differ
    /// between deposits into one channel, since it makes the ERC-3009 nonce.
    pub async fn deposit(
        &self,
        payer: &LocalWallet,
        config: &EvmChannelConfig,
        amount: u128,
        collector_salt: u64,
    ) {
        let channel_id = evm_batch_channel_id(&self.domain(), &signer_config(config));
        let salt = U256::from(collector_salt);
        let nonce = keccak256(encode(&[
            Token::FixedBytes(channel_id.to_vec()),
            Token::Uint(salt),
        ]));
        let valid_after = U256::zero();
        let valid_before = U256::MAX;
        let collector = erc3009_deposit_collector_address();
        let token = FiatToken::new(Address::from(config.token), Arc::clone(&self.provider));
        let separator = token
            .domain_separator()
            .call()
            .await
            .expect("DOMAIN_SEPARATOR");
        let struct_hash = keccak256(encode(&[
            Token::FixedBytes(keccak256(RECEIVE_WITH_AUTHORIZATION_TYPE).to_vec()),
            Token::Address(payer.address()),
            Token::Address(collector),
            Token::Uint(U256::from(amount)),
            Token::Uint(valid_after),
            Token::Uint(valid_before),
            Token::FixedBytes(nonce.to_vec()),
        ]));
        let mut preimage = vec![0x19, 0x01];
        preimage.extend_from_slice(&separator);
        preimage.extend_from_slice(&struct_hash);
        let signature = payer
            .sign_hash(H256::from(keccak256(preimage)))
            .expect("sign the authorization")
            .to_vec();
        let collector_data = encode(&[
            Token::Uint(valid_after),
            Token::Uint(valid_before),
            Token::Uint(salt),
            Token::Bytes(signature),
        ]);
        let contract =
            X402BatchSettlement::new(batch_settlement_address(), Arc::new(self.relayer.clone()));
        send(
            &self.relayer,
            contract
                .deposit(
                    chain_config(config),
                    amount,
                    collector,
                    Bytes::from(collector_data),
                )
                .tx,
        )
        .await;
    }

    /// `signer` -- the channel's payer or payerAuthorizer, which must hold
    /// gas -- starts a timed withdrawal of `amount`.
    pub async fn initiate_withdraw(
        &self,
        signer: &LocalWallet,
        config: &EvmChannelConfig,
        amount: u128,
    ) {
        let client = SignerMiddleware::new(
            Arc::clone(&self.provider),
            signer.clone().with_chain_id(self.chain_id),
        );
        let contract =
            X402BatchSettlement::new(batch_settlement_address(), Arc::new(client.clone()));
        send(
            &client,
            contract.initiate_withdraw(chain_config(config), amount).tx,
        )
        .await;
    }

    /// `signer` -- payer or payerAuthorizer -- completes the pending
    /// withdrawal, once its delay has passed.
    pub async fn finalize_withdraw(&self, signer: &LocalWallet, config: &EvmChannelConfig) {
        let client = SignerMiddleware::new(
            Arc::clone(&self.provider),
            signer.clone().with_chain_id(self.chain_id),
        );
        let contract =
            X402BatchSettlement::new(batch_settlement_address(), Arc::new(client.clone()));
        send(&client, contract.finalize_withdraw(chain_config(config)).tx).await;
    }

    /// Move the chain's clock forward `seconds` and mine a block, so a
    /// withdrawal delay passes without the test waiting for it.
    pub async fn advance_time(&self, seconds: u64) {
        let _: serde_json::Value = self
            .provider
            .request("evm_increaseTime", [seconds])
            .await
            .expect("evm_increaseTime");
        let _: serde_json::Value = self
            .provider
            .request("evm_mine", ())
            .await
            .expect("evm_mine");
    }

    /// `address`'s transaction count: how a test tells that nothing was sent.
    pub async fn nonce(&self, address: Address) -> u64 {
        self.provider
            .get_transaction_count(address, None)
            .await
            .expect("nonce")
            .as_u64()
    }

    /// `channels(id)`: `(balance, totalClaimed)`.
    pub async fn channel(&self, channel: &ChannelId) -> (u128, u128) {
        let id = crate::channel_id::parse_channel_id(channel).expect("a channel id");
        X402BatchSettlement::new(batch_settlement_address(), Arc::clone(&self.provider))
            .channels(id)
            .call()
            .await
            .expect("channels")
    }

    /// `signer`'s voucher for `cumulative_amount` on `channel`: 65 bytes,
    /// `r ‖ s ‖ v` with `v` of 27 or 28, as a wallet signs one.
    pub fn sign_voucher(
        &self,
        signer: &LocalWallet,
        channel: &ChannelId,
        cumulative_amount: u128,
    ) -> Vec<u8> {
        let id = crate::channel_id::parse_channel_id(channel).expect("a channel id");
        let digest = evm_voucher_digest(&self.domain(), &id, cumulative_amount);
        signer
            .sign_hash(H256::from(digest))
            .expect("sign the voucher")
            .to_vec()
    }
}

async fn deploy(client: &Client, creation: &str, arguments: &[u8]) -> Address {
    let creation = creation.trim();
    let mut code = decode_hex(creation.strip_prefix("0x").unwrap_or(creation));
    code.extend_from_slice(arguments);
    let receipt = send(client, TransactionRequest::new().data(code).into()).await;
    receipt
        .contract_address
        .expect("a deployment names its contract")
}

async fn send(client: &Client, transaction: TypedTransaction) -> TransactionReceipt {
    let receipt = client
        .send_transaction(transaction, None)
        .await
        .expect("send")
        .await
        .expect("mined")
        .expect("a receipt");
    assert_eq!(
        receipt.status,
        Some(1u64.into()),
        "fixture transaction {:#x} reverted",
        receipt.transaction_hash
    );
    receipt
}

fn decode_hex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).expect("committed bytecode is hex"))
        .collect()
}
