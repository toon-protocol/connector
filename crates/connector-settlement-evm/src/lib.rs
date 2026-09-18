//! EVM settlement backend (issue #576, ADR 0001, ADR 0002): a real,
//! chain-backed [`SettlementBackend`] driven against
//! `packages/contracts/src/TokenNetwork.sol` -- the two-sided, EIP-712
//! payment-channel contract the live TypeScript fleet already settles
//! through, reached through its `TokenNetworkRegistry` factory -- with no
//! `lockedAmount`/`locksRoot` value in use (ADR 0004; both are still
//! hashed as zero, since the deployed contract's signed struct includes
//! them).
//!
//! `contracts/SettlementChannel.sol` -- this crate's own throwaway,
//! signature-unverified channel contract (issue #459; quarantined against
//! accidental deployment by issue #568) -- is gone. Nothing in this crate
//! constructs, deploys or calls it anymore.
//!
//! Unlike [`connector_settlement::InMemorySettlementBackend`], this
//! backend holds no local channel state of its own: every
//! [`SettlementBackend`] method reads the chain fresh before deciding what
//! the port's rules require, and mutates the chain via a real transaction
//! only once that check passes. `TokenNetwork` tracks a `ParticipantState`
//! (deposit, nonce, transferred amount) per side of a channel rather than
//! one shared balance, so this backend has to know which side it is: see
//! [`EvmSettlementBackend::read_state`] for how a single
//! [`connector_settlement::ChannelState`] is derived from that two-sided
//! shape.

mod bindings;
mod channel_id;
mod channel_index;
pub mod channel_index_sync;
// Also compiled for this crate's own `#[cfg(test)]` unit tests (none left
// after issue #576 removed the #568 constructor-guard tests, which were
// `SettlementChannel`-specific -- kept available the same way regardless,
// matching `connector-operator`'s own `test_support` precedent).
#[cfg(any(test, feature = "test-util"))]
pub mod test_support;

pub use channel_id::{derive_channel_id, sort_participants};
pub use channel_index::{
    ChannelIndexEvent, ChannelIndexLookup, EvmChannelIndex, EvmChannelIndexError,
    IndexedChannelStatus, IndexedContract, OrderedChannelIndexEvent, RejectedSnapshot,
};
pub use channel_index_sync::{ChannelIndexSyncError, EvmChannelIndexSyncer, DEFAULT_POLL_INTERVAL};

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Duration;
use ethers::middleware::nonce_manager::NonceManagerMiddleware;
use ethers::middleware::{Middleware, SignerMiddleware};
use ethers::providers::{Http, JsonRpcClient, PendingTransaction, Provider, ProviderError};
use ethers::signers::{LocalWallet, Signer as EvmSigner};
use ethers::types::{Address, BlockNumber, Bytes, TransactionReceipt, U256};

use connector_settlement::{
    ChannelId, ChannelState, ChannelStatus, Claim, SettlementBackend, SettlementError,
};

use channel_id::{format_channel_id, parse_channel_id};

use bindings::token_network::{
    BalanceProof, ChannelOpenedFilter, TokenNetwork as TokenNetworkContract,
};
use bindings::token_network_registry::TokenNetworkRegistry as TokenNetworkRegistryContract;
use bindings::{Erc20 as Erc20Contract, MockErc20 as MockErc20Contract};

/// The signing client every contract call is made through, wrapped in a
/// [`NonceManagerMiddleware`] so that concurrent calls against the same
/// backend (every [`SettlementBackend`] method takes `&self`, so nothing
/// stops two calls racing) assign themselves distinct, correctly ordered
/// nonces instead of both reading the same "pending" nonce from the node
/// and conflicting when both land.
type EvmClient = NonceManagerMiddleware<SignerMiddleware<Provider<Http>, LocalWallet>>;

/// A [`SettlementBackend`] backed by a real `TokenNetwork` contract
/// instance on an EVM chain, resolved through a `TokenNetworkRegistry`
/// (issue #566), settling in whatever ERC-20 that `TokenNetwork` was
/// created for.
pub struct EvmSettlementBackend {
    contract: TokenNetworkContract<EvmClient>,
    token: Erc20Contract<EvmClient>,
    registry_address: Address,
    /// The chain id `build_client` read from the RPC endpoint at connect
    /// time -- half of the EIP-712 domain a claim against this backend's
    /// `TokenNetwork` is signed under (OpenZeppelin's `EIP712` derives the
    /// domain separator from `block.chainid` and `address(this)`). Read
    /// once rather than per call: a chain does not renumber itself, and a
    /// node pointed at a different chain is a restart.
    chain_id: u64,
    /// This backend's own signing address -- every channel it opens names
    /// this address as one of the two on-chain participants, so
    /// [`read_state`](Self::read_state) can tell which `ParticipantState`
    /// is "self" and which is the counterparty's.
    own_address: Address,
    /// Serializes [`fund`](SettlementBackend::fund): `setTotalDeposit`
    /// takes the counterparty's *new total* deposit, not an increment, so
    /// computing that total requires a read-then-write this backend's own
    /// `&self` concurrency (every method takes `&self`, so nothing stops
    /// two calls racing) would otherwise race on -- two concurrent `fund`
    /// calls could both read the same stale total, both submit the same
    /// higher total, and the second transaction would move zero real
    /// tokens despite appearing to succeed (issue #576's "two concurrent
    /// `fund` calls ... do not lose a deposit" AC). The old
    /// `SettlementChannel.sol` this backend replaces took an increment
    /// server-side and needed no such lock.
    deposit_lock: tokio::sync::Mutex<()>,
}

impl EvmSettlementBackend {
    /// Bind to the `TokenNetwork` that `registry_address`'s
    /// `TokenNetworkRegistry.getTokenNetwork(token_address)` resolves to,
    /// signing every transaction with `private_key` (a hex-encoded
    /// secp256k1 key, `0x`-prefix optional). Refuses -- naming both
    /// addresses -- if the registry has no `TokenNetwork` registered for
    /// `token_address` (the zero address, issue #576's AC): a
    /// `TokenNetworkRegistry` is a factory keyed by token, and there is no
    /// single "the" channel contract to fall back to guessing at.
    ///
    /// `expected_decimals` is the scale the operator wrote down
    /// (`[settlement] decimals`). Nothing here scales by it -- every amount
    /// this backend moves is already in the token's own base units, and
    /// `docs/usdc-cross-chain-settlement.md`'s "6 decimals everywhere" is
    /// what makes that safe across chains -- so it is checked rather than
    /// applied: `connect` reads the token's own `decimals()` and refuses,
    /// naming both values, when they disagree (issue #564, ADR 0009). That
    /// is exactly the startup assertion
    /// `docs/usdc-cross-chain-settlement.md` asks for, and the check that
    /// turns a stale `decimals = 18` from a line with no effect into a
    /// refusal to start.
    pub async fn connect(
        rpc_url: &str,
        private_key: &str,
        registry_address: Address,
        token_address: Address,
        expected_decimals: u8,
    ) -> Result<Self, SettlementError> {
        let (client, own_address, chain_id) = build_client(rpc_url, private_key).await?;
        let client = Arc::new(client);
        let registry = TokenNetworkRegistryContract::new(registry_address, client.clone());
        let token_network_address = registry
            .get_token_network(token_address)
            .call()
            .await
            .map_err(backend_error)?;
        if token_network_address.is_zero() {
            return Err(SettlementError::Backend(format!(
                "registry {registry_address:?} has no TokenNetwork registered for token \
                 {token_address:?}"
            )));
        }
        let contract = TokenNetworkContract::new(token_network_address, client.clone());
        let token = Erc20Contract::new(token_address, client);
        let on_chain_decimals = token.decimals().call().await.map_err(backend_error)?;
        if on_chain_decimals != expected_decimals {
            return Err(SettlementError::Backend(format!(
                "[settlement] decimals is {expected_decimals}, but token {token_address:?} \
                 reports decimals() = {on_chain_decimals}"
            )));
        }
        Ok(Self {
            contract,
            token,
            registry_address,
            chain_id,
            own_address,
            deposit_lock: tokio::sync::Mutex::new(()),
        })
    }

    /// Deploy a fresh `TokenNetworkRegistry` and create a `TokenNetwork`
    /// for `token_address` through it, signed and paid for by
    /// `private_key`, then bind to the result exactly as
    /// [`connect`](Self::connect) would. Used by this crate's own tests
    /// and by local `anvil` tooling -- it exercises the same
    /// registry-resolution path a production [`connect`](Self::connect)
    /// call does, rather than deploying a `TokenNetwork` directly and
    /// side-stepping the registry.
    pub async fn deploy(
        rpc_url: &str,
        private_key: &str,
        token_address: Address,
    ) -> Result<Self, SettlementError> {
        let (client, own_address, chain_id) = build_client(rpc_url, private_key).await?;
        let client = Arc::new(client);
        let registry = TokenNetworkRegistryContract::deploy(client.clone(), ())
            .map_err(backend_error)?
            .send()
            .await
            .map_err(backend_error)?;
        let registry_address = registry.address();

        let call = registry.create_token_network(token_address);
        let pending = call.send().await.map_err(backend_error)?;
        confirm(pending).await?;

        let token_network_address = registry
            .get_token_network(token_address)
            .call()
            .await
            .map_err(backend_error)?;
        let contract = TokenNetworkContract::new(token_network_address, client.clone());
        let token = Erc20Contract::new(token_address, client);
        Ok(Self {
            contract,
            token,
            registry_address,
            chain_id,
            own_address,
            deposit_lock: tokio::sync::Mutex::new(()),
        })
    }

    /// Deploy a fresh, mintable mock ERC-20 (`contracts/MockERC20.sol`)
    /// and mint `mint_to_deployer` of it to `private_key`'s own address,
    /// returning the token's address. Never used against a real chain --
    /// this exists so a disposable test or devnet chain, which starts with
    /// no token deployed at all, has something real for
    /// [`EvmSettlementBackend::deploy`] to point at; a production
    /// deployment always names an already-deployed token address instead.
    pub async fn deploy_mock_token(
        rpc_url: &str,
        private_key: &str,
        mint_to_deployer: u128,
    ) -> Result<Address, SettlementError> {
        let (client, deployer, _chain_id) = build_client(rpc_url, private_key).await?;
        let client = Arc::new(client);
        let contract = MockErc20Contract::deploy(
            client,
            ("USD Coin (mock)".to_string(), "USDC".to_string(), 6u8),
        )
        .map_err(backend_error)?
        .send()
        .await
        .map_err(backend_error)?;
        let call = contract.mint(deployer, U256::from(mint_to_deployer));
        let pending = call.send().await.map_err(backend_error)?;
        confirm(pending).await?;
        Ok(contract.address())
    }

    /// The address this backend's `TokenNetwork` is deployed at -- the
    /// contract every channel operation is actually sent to.
    pub fn address(&self) -> Address {
        self.contract.address()
    }

    /// The `TokenNetworkRegistry` address this backend's `TokenNetwork`
    /// was resolved through -- what a `[settlement] contract_address`
    /// config value names (issue #576: the operator-facing address is the
    /// stable registry, not whichever `TokenNetwork` it currently resolves
    /// to).
    pub fn registry_address(&self) -> Address {
        self.registry_address
    }

    /// The chain id this backend's RPC endpoint reported at connect time.
    /// With [`address`](Self::address) this is the complete EIP-712 domain
    /// a `BalanceProof` on any of this `TokenNetwork`'s channels must be
    /// signed under -- `TokenNetwork` inherits OpenZeppelin's
    /// `EIP712("TokenNetwork", "1")`, whose `_hashTypedDataV4` builds its
    /// domain separator from `block.chainid` and `address(this)`. Issue
    /// #556's open question about where a channel's signing domain comes
    /// from is answered here: it is a property of the deployed contract,
    /// so it is read from the deployment rather than written down twice.
    pub fn chain_id(&self) -> u64 {
        self.chain_id
    }

    /// This backend's own signing address -- the on-chain participant a
    /// buyer opens a channel WITH. Issue #617: the x402 terms carry this so
    /// an unaffiliated buyer can learn the counterparty by asking (ADR
    /// 0022) instead of from an announce this connector never makes.
    pub fn own_address(&self) -> Address {
        self.own_address
    }

    /// Who this backend's counterparty is on `channel_id`, as the chain
    /// itself holds it -- the client edge's own channel record (issue
    /// #556), so that a claim can be verified against the party who
    /// actually opened the channel rather than against whoever the claim
    /// declares.
    ///
    /// `Ok(None)`, rather than an error, for every "this is not a channel
    /// this backend can be paid on":
    ///
    /// - nothing was ever opened at `channel_id`;
    /// - the channel has already `Settled`, so a claim against it can
    ///   never be redeemed (`TokenNetwork.claimFromChannel` requires
    ///   `Opened` or `Closed`, `TokenNetwork.sol:273`) and honouring one
    ///   would be giving work away. A merely `Closed` channel still
    ///   redeems during its challenge period (issue #574), so it still
    ///   answers;
    /// - neither participant is this backend's own signing address, i.e.
    ///   it is somebody else's channel.
    ///
    /// `Err` is reserved for a lookup that genuinely failed -- an
    /// unreachable endpoint, a malformed response -- so a caller can tell
    /// "there is no such channel" apart from "I could not find out".
    ///
    /// One `eth_call`: `TokenNetwork.channels(id)` carries the state and
    /// both participants, so this deliberately does not go through
    /// [`read_state`](Self::read_state), whose three reads exist to report
    /// deposits and claimed amounts nothing here needs.
    pub async fn channel_counterparty(
        &self,
        channel_id: [u8; 32],
    ) -> Result<Option<Address>, SettlementError> {
        let (_settlement_timeout, state, _closed_at, _opened_at, participant1, participant2) =
            self.fetch_channel(channel_id).await?;
        if state == CHANNEL_STATE_NONEXISTENT {
            return Ok(None);
        }
        if status_from_u8(state)? == ChannelStatus::Settled {
            return Ok(None);
        }
        if participant1 == self.own_address {
            Ok(Some(participant2))
        } else if participant2 == self.own_address {
            Ok(Some(participant1))
        } else {
            Ok(None)
        }
    }

    /// [`channel_counterparty`](Self::channel_counterparty), plus that
    /// counterparty's own `ParticipantState.deposit` -- the bound
    /// `claimFromChannel` enforces at redemption (`TokenNetwork.sol:316-318`,
    /// `InsufficientChannelBalance`), and therefore the most a claim this
    /// backend resolves can ever be worth (issue #646).
    ///
    /// **One `eth_call` more than [`channel_counterparty`](Self::channel_counterparty)**,
    /// and it is unavoidable: the deposit is not in `channels(id)` at all,
    /// it lives in `participants[channelId][counterparty]`
    /// (`TokenNetwork.sol:73-77`). It is the same second read
    /// [`read_state`](Self::read_state) already makes, and the client edge
    /// memoises the answer per channel, so it costs one extra call the
    /// first time a channel is seen and none on any later packet.
    ///
    /// `U256`, not `u128`/`u64`: the caller decides how to narrow it. A
    /// deposit larger than the width a claim's cumulative amount is carried
    /// in cannot be exceeded by one, so saturating there is sound.
    pub async fn channel_counterparty_deposit(
        &self,
        channel_id: [u8; 32],
    ) -> Result<Option<(Address, U256)>, SettlementError> {
        let Some(counterparty) = self.channel_counterparty(channel_id).await? else {
            return Ok(None);
        };
        let (deposit, _nonce, _transferred_amount) = self
            .contract
            .participants(channel_id, counterparty)
            .call()
            .await
            .map_err(backend_error)?;
        Ok(Some((counterparty, deposit)))
    }

    /// This node's current channel epoch with `counterparty`, read from
    /// the chain: `TokenNetwork.channelEpoch(p1, p2)` over the sorted pair
    /// (ADR 0059). One `eth_call`.
    ///
    /// Zero for a pair that has never settled a channel here -- including
    /// a pair that has never opened one -- and one higher after each of
    /// their channels settles. It is not a count of live channels: there
    /// is at most one of those, and `openChannel` refuses a second
    /// (`ChannelAlreadyExists`).
    pub async fn channel_epoch(&self, counterparty: Address) -> Result<U256, SettlementError> {
        let (p1, p2) = sort_participants(self.own_address, counterparty);
        self.contract
            .channel_epoch(p1, p2)
            .call()
            .await
            .map_err(backend_error)
    }

    /// The channel id this node and `counterparty` derive **right now** --
    /// [`channel_epoch`](Self::channel_epoch) plus
    /// [`derive_channel_id`]. One `eth_call`.
    ///
    /// It names where their next channel will land, which is the same
    /// place their current one already is if they have one. It says
    /// nothing about whether anything is there: ask
    /// [`channel_with`](Self::channel_with) for that.
    pub async fn derived_channel_id(
        &self,
        counterparty: Address,
    ) -> Result<ChannelId, SettlementError> {
        let epoch = self.channel_epoch(counterparty).await?;
        Ok(derive_channel_id(self.own_address, counterparty, epoch))
    }

    /// **"Do I already have a channel with this counterparty?"**, answered
    /// from the chain (ADR 0059, issue #1158). `Ok(Some(id))` when one is
    /// live, `Ok(None)` when the pair has none and
    /// [`open`](SettlementBackend::open) is what to do next; `Err` only
    /// when the chain could not be asked, so "there is no channel" is
    /// never confused with "I could not find out". Two `eth_call`s:
    /// `channelEpoch`, then `channels` at the id that derives from it.
    ///
    /// The read goes to the chain rather than to
    /// [`EvmChannelIndex`]'s `lookup`, and deliberately: that index is a
    /// projection of `ChannelOpened` logs and is only complete once
    /// `channel_index_from_block` has been replayed, so a "none exists"
    /// out of a half-built index opens a duplicate channel. A derivation
    /// plus a point read has no such window -- ADR 0059 rejects building
    /// a local participant index for exactly this reason.
    ///
    /// "Live" is `Opened` or `Closed`: a `Closed` channel is still in its
    /// challenge window, still holds collateral and still occupies the
    /// pair's id, so reporting it absent would hand the caller an
    /// `openChannel` that reverts. `Settled` cannot appear at the current
    /// epoch at all -- `settleChannel` advances the epoch in the same
    /// transaction that sets that state
    /// (`packages/contracts/test/TokenNetworkChannelDerivation.t.sol`) --
    /// so it needs no case of its own here.
    pub async fn channel_with(
        &self,
        counterparty: Address,
    ) -> Result<Option<ChannelId>, SettlementError> {
        let channel = self.derived_channel_id(counterparty).await?;
        let (_, state, _, _, _, _) = self.fetch_channel(parse_channel_id(&channel)?).await?;
        if state == CHANNEL_STATE_NONEXISTENT {
            return Ok(None);
        }
        Ok(Some(channel))
    }

    /// Resolve `channel` to the on-chain id it names and confirm a channel
    /// actually exists there (`TokenNetwork.channels(id).state !=
    /// NonExistent`) -- [`SettlementError::ChannelNotFound`] either because
    /// `channel`'s string does not parse as a `bytes32` id at all, or
    /// because nothing was ever opened at the one it names.
    async fn existing_channel_id(&self, channel: &ChannelId) -> Result<[u8; 32], SettlementError> {
        let id = parse_channel_id(channel)?;
        let (_, state, _, _, _, _) = self.fetch_channel(id).await?;
        if state == CHANNEL_STATE_NONEXISTENT {
            return Err(SettlementError::ChannelNotFound(channel.clone()));
        }
        Ok(id)
    }

    /// The one place this backend calls `TokenNetwork.channels` -- both
    /// [`read_state`](Self::read_state) and
    /// [`settle`](SettlementBackend::settle) need a subset of the same
    /// six-tuple.
    async fn fetch_channel(
        &self,
        id: [u8; 32],
    ) -> Result<(U256, u8, U256, U256, Address, Address), SettlementError> {
        self.contract
            .channels(id)
            .call()
            .await
            .map_err(backend_error)
    }

    /// The latest block's own timestamp, read from the chain this backend
    /// talks to -- what [`settle`](SettlementBackend::settle) compares a
    /// channel's settlement deadline against, rather than this process's
    /// own wall clock. `TokenNetwork.settleChannel` itself checks
    /// `block.timestamp`, which a real chain's block production can drift
    /// from this process's system clock by more than a negligible amount
    /// (and, for a test chain whose clock has been deliberately warped
    /// ahead via `evm_increaseTime`, by a great deal) -- reading the
    /// chain's own notion of "now" is what actually agrees with the
    /// on-chain check this precondition exists to anticipate.
    async fn chain_timestamp(&self) -> Result<U256, SettlementError> {
        let block = self
            .contract
            .client()
            .get_block(BlockNumber::Latest)
            .await
            .map_err(backend_error)?
            .ok_or_else(|| SettlementError::Backend("chain has no latest block".to_string()))?;
        Ok(block.timestamp)
    }

    /// Which of a channel's two on-chain participants is this backend's
    /// own counterparty -- the other one, whichever side `own_address`
    /// is not. [`SettlementError::Backend`] if neither side is
    /// `own_address` at all, which should never happen for a channel this
    /// backend itself opened (every [`open`](SettlementBackend::open) call
    /// names `own_address` as one of the two participants), but is
    /// reported rather than silently guessed at if it ever does.
    fn counterparty_of(
        &self,
        participant1: Address,
        participant2: Address,
    ) -> Result<Address, SettlementError> {
        if participant1 == self.own_address {
            Ok(participant2)
        } else if participant2 == self.own_address {
            Ok(participant1)
        } else {
            Err(SettlementError::Backend(format!(
                "channel participants {participant1:?}/{participant2:?} include neither this \
                 backend's own signing address {:?}",
                self.own_address
            )))
        }
    }

    /// Approve-then-`setTotalDeposit`: two transactions, where a single
    /// `payable` call sufficed for native ETH. Approving a large fixed
    /// allowance, rather than exactly the increment, means a stale
    /// approval from an earlier call (or another channel funded through
    /// this same backend) is still always enough -- `deposit_lock` already
    /// rules out two `fund` calls racing each other's approval.
    ///
    /// `total` is `participant`'s **new cumulative** deposit, not an
    /// increment: that is `setTotalDeposit`'s own parameter
    /// (`TokenNetwork.sol:252`), and computing it is the read-then-write
    /// `deposit_lock` exists to serialize.
    async fn set_total_deposit(
        &self,
        id: [u8; 32],
        participant: Address,
        total: U256,
    ) -> Result<(), SettlementError> {
        let approve = self.token.approve(self.contract.address(), U256::MAX);
        let pending = approve.send().await.map_err(backend_error)?;
        confirm(pending).await?;

        let call = self.contract.set_total_deposit(id, participant, total);
        let pending = call.send().await.map_err(backend_error)?;
        confirm(pending).await?;
        Ok(())
    }

    /// Test/dev-only (issue #1118): mint `amount` of this backend's token
    /// to `owner`, which works only because the local chain's token is
    /// `MockERC20` (`packages/contracts/test/mocks/MockERC20.sol`) and this
    /// backend's signer is its minter.
    ///
    /// The EVM twin of `SolanaSettlementBackend::test_mint_tokens_to`, and
    /// needed for the same reason: once `fund` is a self-deposit, each side
    /// of a channel spends its **own** tokens to collateralise, so a test
    /// standing up a second participant has to put real tokens in that
    /// participant's balance first -- exactly as a real deployment has to.
    #[cfg(any(test, feature = "test-util"))]
    pub async fn mint_mock_tokens_to(
        &self,
        owner: Address,
        amount: u128,
    ) -> Result<(), SettlementError> {
        let token = MockErc20Contract::new(self.token.address(), self.token.client());
        let call = token.mint(owner, U256::from(amount));
        let pending = call.send().await.map_err(backend_error)?;
        confirm(pending).await?;
        Ok(())
    }

    /// Deposit `amount` into the **counterparty's** side of `channel`,
    /// from this backend's own token balance -- the delegate deposit
    /// `TokenNetwork.setTotalDeposit` permits by naming the credited
    /// participant separately from the caller whose tokens are pulled
    /// (`TokenNetwork.sol:255`, `:273`, `:282`).
    ///
    /// **Not on the [`SettlementBackend`] port, and not for production**
    /// (issue #1118). No node should pay for its counterparty's
    /// collateral, and no other chain this port settles on can:
    /// `packages/solana-program`'s `Deposit` credits strictly by signer.
    /// This exists so a fixture can stand in for the external actor that
    /// would really make that deposit -- it is what this crate's
    /// `connector_settlement::contract::ContractFixture::fund_counterparty`
    /// is wired to -- which is why it is `test-util`-gated rather than
    /// merely documented as "do not call".
    #[cfg(any(test, feature = "test-util"))]
    pub async fn fund_counterparty(
        &self,
        channel: &ChannelId,
        amount: u128,
    ) -> Result<ChannelState, SettlementError> {
        let _guard = self.deposit_lock.lock().await;

        let (id, state) = self.open_channel(channel).await?;
        let counterparty = Address::from_slice(&state.counterparty);
        let new_total = U256::from(state.counterparty_deposited) + U256::from(amount);
        self.set_total_deposit(id, counterparty, new_total).await?;
        self.read_state(channel, id).await
    }

    /// Derive a single [`ChannelState`] from `TokenNetwork`'s two-sided
    /// state (issue #576's core mismatch): `counterparty_deposited` is the
    /// counterparty's own `ParticipantState.deposit` -- the balance
    /// `claimFromChannel` actually bounds a claim against
    /// (`TokenNetwork.sol:317`) -- `own_deposited` is this backend's own
    /// side of the same mapping (issue #1118), what
    /// [`fund`](SettlementBackend::fund) raises and what
    /// `settleChannel` eventually returns; and `redeemed` is
    /// `claimedAmounts[channelId][self]` -- what *this* backend has
    /// already pulled out via [`redeem`](SettlementBackend::redeem).
    /// Reading the sides the other way round would report a channel that
    /// looks funded and is not.
    async fn read_state(
        &self,
        channel: &ChannelId,
        id: [u8; 32],
    ) -> Result<ChannelState, SettlementError> {
        let (_settlement_timeout, state, _closed_at, _opened_at, participant1, participant2) =
            self.fetch_channel(id).await?;
        let counterparty = self.counterparty_of(participant1, participant2)?;
        let (counterparty_deposit, _nonce, _transferred_amount) = self
            .contract
            .participants(id, counterparty)
            .call()
            .await
            .map_err(backend_error)?;
        let (own_deposit, _nonce, _transferred_amount) = self
            .contract
            .participants(id, self.own_address)
            .call()
            .await
            .map_err(backend_error)?;
        let self_claimed = self
            .contract
            .claimed_amounts(id, self.own_address)
            .call()
            .await
            .map_err(backend_error)?;
        Ok(ChannelState {
            id: channel.clone(),
            counterparty: counterparty.as_bytes().to_vec(),
            status: status_from_u8(state)?,
            counterparty_deposited: counterparty_deposit.as_u128(),
            own_deposited: own_deposit.as_u128(),
            redeemed: self_claimed.as_u128(),
        })
    }

    /// Resolve `channel` to its on-chain id and current state, rejecting
    /// with [`SettlementError::ChannelClosed`]/[`SettlementError::ChannelSettled`]
    /// if it is not still `Open` -- the one precondition
    /// [`fund`](SettlementBackend::fund) and
    /// [`close`](SettlementBackend::close) share before doing their own,
    /// method-specific checks. [`redeem`](SettlementBackend::redeem) uses
    /// [`redeemable_channel`](Self::redeemable_channel) instead, since it
    /// still succeeds against a `Closed` channel (issue #574).
    async fn open_channel(
        &self,
        channel: &ChannelId,
    ) -> Result<([u8; 32], ChannelState), SettlementError> {
        let id = self.existing_channel_id(channel).await?;
        let state = self.read_state(channel, id).await?;
        match state.status {
            ChannelStatus::Open => Ok((id, state)),
            ChannelStatus::Closed => Err(SettlementError::ChannelClosed(channel.clone())),
            ChannelStatus::Settled => Err(SettlementError::ChannelSettled(channel.clone())),
        }
    }

    /// Resolve `channel` to its on-chain id and current state, rejecting
    /// only a `Settled` channel (issue #574) -- used by
    /// [`redeem`](SettlementBackend::redeem), which succeeds against both
    /// `Open` and `Closed`.
    async fn redeemable_channel(
        &self,
        channel: &ChannelId,
    ) -> Result<([u8; 32], ChannelState), SettlementError> {
        let id = self.existing_channel_id(channel).await?;
        let state = self.read_state(channel, id).await?;
        if state.status == ChannelStatus::Settled {
            return Err(SettlementError::ChannelSettled(channel.clone()));
        }
        Ok((id, state))
    }
}

const CHANNEL_STATE_NONEXISTENT: u8 = 0;

fn status_from_u8(state: u8) -> Result<ChannelStatus, SettlementError> {
    match state {
        1 => Ok(ChannelStatus::Open),
        2 => Ok(ChannelStatus::Closed),
        3 => Ok(ChannelStatus::Settled),
        other => Err(SettlementError::Backend(format!(
            "TokenNetwork reported an unknown channel state {other}"
        ))),
    }
}

/// Builds the signing client every contract call goes through, alongside
/// the address it signs as -- callers that need to name that address
/// directly (minting a mock token to it, for instance) would otherwise
/// have to re-derive it from `private_key` a second time.
async fn build_client(
    rpc_url: &str,
    private_key: &str,
) -> Result<(EvmClient, Address, u64), SettlementError> {
    // ethers' default HTTP polling interval (7s) is tuned for mainnet block
    // times, not a fast-confirming chain -- every open/fund/redeem/close
    // otherwise pays that whole interval waiting for a receipt Anvil (or
    // any low-block-time chain) already mined.
    let provider = Provider::<Http>::try_from(rpc_url)
        .map_err(backend_error)?
        .interval(std::time::Duration::from_millis(100));
    let chain_id = provider
        .get_chainid()
        .await
        .map_err(backend_error)?
        .as_u64();
    let wallet: LocalWallet = private_key.parse().map_err(backend_error)?;
    let address = wallet.address();
    let signer = SignerMiddleware::new(provider, wallet.with_chain_id(chain_id));
    Ok((
        NonceManagerMiddleware::new(signer, address),
        address,
        chain_id,
    ))
}

/// A `TokenNetwork` counterparty must be a real 20-byte EVM address: it has
/// to *sign* balance proofs (`TokenNetwork.claimFromChannel` recovers the
/// signer and checks it against this exact address), so hashing an
/// arbitrary identifier down to something address-shaped -- what this
/// backend did before issue #576 -- produces an address nobody holds the
/// key to. `SettlementChannel.sol`'s `redeem` transferred to exactly such
/// an unrecoverable address; `open` now refuses rather than inventing one
/// (issue #566's first comment, issue #576's AC).
fn counterparty_address(counterparty: &[u8]) -> Result<Address, SettlementError> {
    if counterparty.len() != 20 {
        return Err(SettlementError::Backend(format!(
            "a TokenNetwork counterparty must be a 20-byte EVM address able to sign balance \
             proofs, got {} bytes",
            counterparty.len()
        )));
    }
    Ok(Address::from_slice(counterparty))
}

fn backend_error<E: std::fmt::Display>(error: E) -> SettlementError {
    SettlementError::Backend(error.to_string())
}

/// Put a claim's `r || s || v` signature's trailing recovery-id byte into
/// the `{27, 28}` range `TokenNetwork.claimFromChannel`'s `ECDSA.recover`
/// requires (issue #590). The peer semantics (and every other producer of a
/// [`Claim`]) carries whatever libsecp256k1 itself emits -- `{0, 1}` -- so
/// this is the one place that convention is bridged to the Ethereum-wallet
/// one an on-chain verifier expects; a value already in `{27, 28}` is left
/// unchanged rather than shifted again, so this is safe to call regardless
/// of which convention a caller happens to hand in. Anything else is a
/// malformed signature refused up front rather than submitted to revert on
/// chain.
fn normalize_recovery_id(mut signature: Vec<u8>) -> Result<Vec<u8>, SettlementError> {
    let Some(&last) = signature.last() else {
        return Err(SettlementError::InvalidClaimSignature(
            "claim signature is empty".to_string(),
        ));
    };
    let normalized = match last {
        0 | 1 => last + 27,
        27 | 28 => last,
        other => {
            return Err(SettlementError::InvalidClaimSignature(format!(
                "recovery id {other} is outside both libsecp256k1's {{0,1}} and Ethereum's \
                 {{27,28}} ranges"
            )))
        }
    };
    let last_index = signature.len() - 1;
    signature[last_index] = normalized;
    Ok(signature)
}

/// Wait for `pending` to mine and confirm it actually succeeded (issue
/// #425: "confirmation ... handled explicitly rather than assumed",
/// "a failed or reverted settlement transaction leaves recoverable
/// state"). A transaction that reverts on chain is still mined -- it
/// consumes gas and produces a receipt exactly like a successful one, with
/// only `status` distinguishing the two -- so a caller that stopped at
/// "did a receipt come back" would treat a reverted `redeem` or `close` as
/// success and report whatever state happened to be there already as if
/// the operation had taken effect. Checking `status` here, in the one
/// place every channel operation confirms through, means every one of
/// them fails loudly instead: nothing is ever recorded beyond what the
/// chain itself did, so a reverted transaction leaves nothing to recover
/// from beyond retrying with a fresh read of the real state.
///
/// `pending`'s own `Ok(None)` (issue #907) means ethers' `PendingTransaction`
/// gave up polling `eth_getTransactionByHash` for this hash after a handful
/// of tries -- it is *not* proof the transaction was dropped, only that this
/// endpoint had not observed it yet. Against a load-balanced RPC (the
/// devnet repro used `base-sepolia-rpc.publicnode.com`), consecutive polls
/// can land on different backend nodes, some of which simply have not
/// caught up to the block the transaction is actually in. Reporting that as
/// "dropped" told an operator (or an automated caller) that resubmitting
/// was safe when the transaction went on to mine -- a second `open`/`fund`
/// call is a real double spend, not a retry. So `Ok(None)` here is not the
/// end of the story: [`recheck_unobserved`] asks the chain directly, by
/// hash, before this function concludes anything, and the failure it can
/// still return says "not observed", carries the hash, and says plainly
/// that retrying is not known to be safe -- never "dropped".
async fn confirm<P: JsonRpcClient + Clone>(
    pending: PendingTransaction<'_, P>,
) -> Result<TransactionReceipt, SettlementError> {
    let tx_hash = pending.tx_hash();
    let provider = pending.provider();
    let receipt = match pending
        .await
        .map_err(|error: ProviderError| backend_error(error))?
    {
        Some(receipt) => receipt,
        None => recheck_unobserved(&provider, tx_hash).await?,
    };
    if receipt.status == Some(ethers::types::U64::zero()) {
        return Err(SettlementError::Backend(format!(
            "transaction {:#x} reverted on chain",
            receipt.transaction_hash
        )));
    }
    Ok(receipt)
}

/// How many direct `eth_getTransactionReceipt` calls [`recheck_unobserved`]
/// makes, spaced [`UNOBSERVED_RECHECK_INTERVAL`] apart, before concluding a
/// transaction genuinely cannot be confirmed right now. ethers' own
/// `PendingTransaction` has already spent its own retry budget (at the
/// endpoint's polling interval) failing to observe the hash before this
/// runs at all, so this only needs to cover a load-balanced endpoint
/// happening to route a further handful of requests to lagging backends.
const UNOBSERVED_RECHECK_ATTEMPTS: usize = 4;

/// How long [`recheck_unobserved`] waits between those calls -- long enough
/// for a lagging backend to have caught up on a new block, short enough
/// that the whole re-check stays within a caller's patience.
const UNOBSERVED_RECHECK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

/// The authoritative re-check `confirm` falls back to once ethers' own
/// mempool-visibility polling has given up on a hash (issue #907). Unlike
/// `PendingTransaction`'s `GettingTx` loop -- which asks whether the
/// endpoint has the transaction *pending* -- this asks for the receipt
/// directly, which exists the moment the transaction mines regardless of
/// whether this endpoint's mempool view ever showed it as pending. A few
/// spaced-out tries give a lagging load-balanced backend a chance to catch
/// up before this reports anything.
async fn recheck_unobserved<P: JsonRpcClient>(
    provider: &Provider<P>,
    tx_hash: ethers::types::TxHash,
) -> Result<TransactionReceipt, SettlementError> {
    for attempt in 0..UNOBSERVED_RECHECK_ATTEMPTS {
        if attempt > 0 {
            tokio::time::sleep(UNOBSERVED_RECHECK_INTERVAL).await;
        }
        if let Ok(Some(receipt)) = provider.get_transaction_receipt(tx_hash).await {
            return Ok(receipt);
        }
    }
    Err(SettlementError::Backend(format!(
        "transaction {tx_hash:#x} was not observed within the timeout -- this is not proof it \
         was dropped, only that this endpoint has not confirmed it yet; check its status by \
         hash (a block explorer or a fresh eth_getTransactionReceipt call) before resubmitting, \
         since a transaction that later mines would be double-spent by a retry"
    )))
}

#[async_trait]
impl SettlementBackend for EvmSettlementBackend {
    async fn open(
        &self,
        counterparty: Vec<u8>,
        settlement_timeout: Duration,
    ) -> Result<ChannelId, SettlementError> {
        let participant2 = counterparty_address(&counterparty)?;
        let seconds = settlement_timeout.num_seconds().max(0) as u64;

        let call = self
            .contract
            .open_channel(participant2, U256::from(seconds));
        let pending = call.send().await.map_err(backend_error)?;
        let receipt = confirm(pending).await?;

        for log in &receipt.logs {
            if let Ok(decoded) = self.contract.decode_event::<ChannelOpenedFilter>(
                "ChannelOpened",
                log.topics.clone(),
                log.data.clone(),
            ) {
                return Ok(format_channel_id(decoded.channel_id));
            }
        }
        Err(SettlementError::Backend(
            "open: no ChannelOpened event in the transaction receipt".to_string(),
        ))
    }

    /// A **self**-deposit (issue #1118): `setTotalDeposit` is called
    /// naming this backend's own `own_address` as the participant to
    /// credit, so the tokens it pulls from this node land behind this
    /// node's own claims and nowhere else. Until #1118 this named the
    /// *counterparty* instead -- a delegate deposit `TokenNetwork` happens
    /// to permit (caller and credited participant are independent
    /// parameters, `TokenNetwork.sol:255`, `:273`, `:282`) and
    /// `packages/solana-program` deliberately does not, which is what left
    /// the Solana backend's `fund` an unconditional error. It is also the
    /// shape production should never have: a node paying for its
    /// counterparty's collateral out of its own token balance.
    /// [`fund_counterparty`](Self::fund_counterparty) keeps the delegate
    /// deposit available to this crate's own fixtures, where standing in
    /// for an absent external actor is the whole point.
    async fn fund(
        &self,
        channel: &ChannelId,
        amount: u128,
    ) -> Result<ChannelState, SettlementError> {
        // Serializes the read-then-write below against other concurrent
        // `fund` calls on this backend -- see `deposit_lock`'s own doc.
        let _guard = self.deposit_lock.lock().await;

        let (id, state) = self.open_channel(channel).await?;
        let new_total = U256::from(state.own_deposited) + U256::from(amount);
        self.set_total_deposit(id, self.own_address, new_total)
            .await?;
        self.read_state(channel, id).await
    }

    async fn redeem(
        &self,
        channel: &ChannelId,
        claim: Claim,
    ) -> Result<ChannelState, SettlementError> {
        let (id, state) = self.redeemable_channel(channel).await?;
        if claim.cumulative_amount <= state.redeemed {
            return Err(SettlementError::StaleClaim {
                claimed: claim.cumulative_amount,
                already_redeemed: state.redeemed,
            });
        }
        if claim.cumulative_amount > state.counterparty_deposited {
            return Err(SettlementError::InsufficientChannelBalance {
                requested: claim.cumulative_amount,
                deposited: state.counterparty_deposited,
            });
        }

        let balance_proof = BalanceProof {
            channel_id: id,
            nonce: U256::from(claim.nonce),
            transferred_amount: U256::from(claim.cumulative_amount),
            // ADR 0004: this port never uses HTLCs, but the deployed
            // typehash still hashes these two fields -- omitting them
            // would compute a different EIP-712 digest than the one
            // `claim.signature` was actually produced over.
            locked_amount: U256::zero(),
            locks_root: [0u8; 32],
        };
        let signature = normalize_recovery_id(claim.signature)?;
        let call = self
            .contract
            .claim_from_channel(id, balance_proof, Bytes::from(signature));
        let pending = call.send().await.map_err(backend_error)?;
        confirm(pending).await?;

        self.read_state(channel, id).await
    }

    async fn close(&self, channel: &ChannelId) -> Result<ChannelState, SettlementError> {
        let (id, _state) = self.open_channel(channel).await?;

        let call = self.contract.close_channel(id);
        let pending = call.send().await.map_err(backend_error)?;
        confirm(pending).await?;

        self.read_state(channel, id).await
    }

    async fn settle(&self, channel: &ChannelId) -> Result<ChannelState, SettlementError> {
        let id = self.existing_channel_id(channel).await?;
        let (settlement_timeout, state, closed_at, _opened_at, _p1, _p2) =
            self.fetch_channel(id).await?;

        match status_from_u8(state)? {
            ChannelStatus::Settled => return Err(SettlementError::ChannelSettled(channel.clone())),
            // Never closed (still Open) has no deadline to have passed.
            ChannelStatus::Open => {
                return Err(SettlementError::SettlementNotYetDue(channel.clone()))
            }
            ChannelStatus::Closed => {}
        }
        let available_at = closed_at + settlement_timeout;
        let now = self.chain_timestamp().await?;
        if now < available_at {
            return Err(SettlementError::SettlementNotYetDue(channel.clone()));
        }

        let call = self.contract.settle_channel(id);
        let pending = call.send().await.map_err(backend_error)?;
        confirm(pending).await?;

        self.read_state(channel, id).await
    }

    async fn channel_state(&self, channel: &ChannelId) -> Result<ChannelState, SettlementError> {
        let id = self.existing_channel_id(channel).await?;
        self.read_state(channel, id).await
    }

    /// The port's ADR 0059 question, answered by
    /// [`channel_with`](Self::channel_with) -- `channelEpoch` over the
    /// sorted pair, then `channels` at the id that derives from it. The
    /// counterparty is a 20-byte `TokenNetwork` participant address, the
    /// same identity [`open`](SettlementBackend::open) takes and the same
    /// one whose signature `claimFromChannel` recovers.
    async fn live_channel_with(
        &self,
        counterparty: Vec<u8>,
    ) -> Result<Option<ChannelId>, SettlementError> {
        self.channel_with(counterparty_address(&counterparty)?)
            .await
    }
}

#[cfg(test)]
mod recovery_id_tests {
    use super::normalize_recovery_id;
    use connector_settlement::SettlementError;

    fn signature_ending_in(last: u8) -> Vec<u8> {
        let mut bytes = vec![0u8; 65];
        bytes[64] = last;
        bytes
    }

    #[test]
    fn a_libsecp256k1_recovery_id_of_zero_is_normalized_to_the_ethereum_wallet_convention() {
        let normalized = normalize_recovery_id(signature_ending_in(0)).unwrap();
        assert_eq!(normalized.last(), Some(&27));
    }

    #[test]
    fn a_libsecp256k1_recovery_id_of_one_is_normalized_to_the_ethereum_wallet_convention() {
        let normalized = normalize_recovery_id(signature_ending_in(1)).unwrap();
        assert_eq!(normalized.last(), Some(&28));
    }

    #[test]
    fn a_recovery_id_already_in_the_ethereum_wallet_convention_is_left_unchanged() {
        assert_eq!(
            normalize_recovery_id(signature_ending_in(27))
                .unwrap()
                .last(),
            Some(&27)
        );
        assert_eq!(
            normalize_recovery_id(signature_ending_in(28))
                .unwrap()
                .last(),
            Some(&28)
        );
    }

    #[test]
    fn normalization_is_idempotent_and_never_shifts_an_already_normalized_signature_again() {
        let once = normalize_recovery_id(signature_ending_in(0)).unwrap();
        let twice = normalize_recovery_id(once.clone()).unwrap();
        assert_eq!(once, twice);
        assert_eq!(twice.last(), Some(&27));
    }

    #[test]
    fn an_out_of_range_recovery_id_is_refused_with_a_named_error_rather_than_submitted() {
        let error = normalize_recovery_id(signature_ending_in(2)).unwrap_err();
        assert!(matches!(error, SettlementError::InvalidClaimSignature(_)));
    }

    #[test]
    fn an_empty_signature_is_refused_rather_than_panicking() {
        let error = normalize_recovery_id(Vec::new()).unwrap_err();
        assert!(matches!(error, SettlementError::InvalidClaimSignature(_)));
    }
}

/// Issue #907: a transaction that mines must never be reported as dropped,
/// because the obvious response to "dropped" is to retry, and retrying a
/// transaction that already mined double-spends it.
///
/// The repro was a load-balanced RPC endpoint whose `eth_getTransactionByHash`
/// answers came back empty for several consecutive polls even though the
/// transaction had already mined -- ethers' own `PendingTransaction` gives up
/// after exactly that pattern and resolves to `Ok(None)`, which `confirm`
/// used to report verbatim as "dropped before mining". [`FlakyMempoolClient`]
/// reproduces the endpoint's half of that behaviour deterministically: every
/// RPC call reaches the real `anvil` chain except `eth_getTransactionByHash`,
/// which always answers empty, forcing the exact `Ok(None)` `confirm` must
/// now treat as "not observed" rather than "dropped".
#[cfg(test)]
mod confirm_tests {
    use std::str::FromStr;
    use std::sync::Arc;
    use std::time::Duration;

    use async_trait::async_trait;
    use ethers::middleware::SignerMiddleware;
    use ethers::providers::{Http, JsonRpcClient, Middleware, PendingTransaction, Provider};
    use ethers::signers::{LocalWallet, Signer};
    use ethers::types::{TransactionRequest, TxHash};
    use serde::de::DeserializeOwned;
    use serde::Serialize;

    use crate::test_support::{require_anvil, Anvil, DEPLOYER_PRIVATE_KEY};
    use connector_settlement::SettlementError;

    /// Forwards every RPC call to a real endpoint except
    /// `eth_getTransactionByHash`, which always answers "not found" --
    /// simulating a load-balanced RPC node that has never observed a given
    /// transaction, regardless of what the chain itself has already done.
    #[derive(Debug, Clone)]
    struct FlakyMempoolClient {
        inner: Http,
    }

    #[async_trait]
    impl JsonRpcClient for FlakyMempoolClient {
        type Error = <Http as JsonRpcClient>::Error;

        async fn request<T, R>(&self, method: &str, params: T) -> Result<R, Self::Error>
        where
            T: std::fmt::Debug + Serialize + Send + Sync,
            R: DeserializeOwned + Send,
        {
            if method == "eth_getTransactionByHash" {
                return Ok(serde_json::from_value(serde_json::Value::Null).expect(
                    "R is Option<Transaction> for this method, which `null` deserializes into",
                ));
            }
            self.inner.request(method, params).await
        }
    }

    fn flaky_provider(rpc_url: &str) -> Provider<FlakyMempoolClient> {
        let http = Http::from_str(rpc_url).expect("valid anvil url");
        Provider::new(FlakyMempoolClient { inner: http }).interval(Duration::from_millis(50))
    }

    #[tokio::test]
    async fn a_transaction_the_endpoint_never_observed_pending_but_did_mine_still_confirms() {
        if !require_anvil() {
            return;
        }
        let anvil = Anvil::spawn(19_700).await;
        let provider = flaky_provider(&anvil.rpc_url);
        let wallet: LocalWallet = DEPLOYER_PRIVATE_KEY
            .parse::<LocalWallet>()
            .expect("valid key")
            .with_chain_id(31_337u64);
        let client = Arc::new(SignerMiddleware::new(provider, wallet));

        // A self-transfer: what address it lands on doesn't matter, only
        // that a real transaction is submitted and mines.
        let tx = TransactionRequest::new()
            .to(client.address())
            .value(1u64)
            .from(client.address());
        let pending = client
            .send_transaction(tx, None)
            .await
            .expect("submit to the real chain");

        // The flaky client makes ethers' own polling give up and resolve to
        // `Ok(None)` -- confirmed separately below so this test does not
        // silently pass because the transaction never even reached that
        // state.
        let tx_hash = pending.tx_hash();
        let gave_up = PendingTransaction::new(tx_hash, client.provider())
            .interval(Duration::from_millis(20))
            .retries(1)
            .await
            .expect("no transport error");
        assert!(
            gave_up.is_none(),
            "expected the flaky client to reproduce ethers' own give-up (Ok(None)); the test \
             setup is not exercising the case this fix addresses"
        );

        let receipt = super::confirm(
            PendingTransaction::new(tx_hash, client.provider()).interval(Duration::from_millis(50)),
        )
        .await
        .expect("a transaction that actually mined must confirm, not report as dropped");
        assert_eq!(receipt.transaction_hash, tx_hash);
        assert_eq!(receipt.status, Some(1.into()));
    }

    #[tokio::test]
    async fn a_transaction_truly_never_observed_is_reported_as_not_observed_not_dropped() {
        if !require_anvil() {
            return;
        }
        let anvil = Anvil::spawn(19_750).await;
        let provider = flaky_provider(&anvil.rpc_url);

        // A hash nothing was ever submitted under -- the real chain
        // genuinely has no receipt for it, so `confirm` must exhaust its
        // recheck budget and fail, but with wording that does not claim the
        // (nonexistent) transaction was dropped.
        let tx_hash: TxHash = TxHash::from_low_u64_be(1);
        let pending = PendingTransaction::new(tx_hash, &provider)
            .interval(Duration::from_millis(20))
            .retries(1);

        let error = super::confirm(pending).await.unwrap_err();
        let SettlementError::Backend(message) = error else {
            panic!("expected SettlementError::Backend, got {error:?}");
        };
        assert!(
            !message.contains("dropped before mining"),
            "message must not assert the transaction was dropped -- that is a stronger claim \
             than this function can ever verify: {message}"
        );
        assert!(
            message.contains("not observed"),
            "message should say plainly that it was not observed: {message}"
        );
        assert!(
            message.contains(&format!("{tx_hash:#x}")),
            "message must carry the transaction hash so a caller is not left to recover it via \
             a block explorer: {message}"
        );
    }
}
