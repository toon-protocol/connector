//! Solana settlement backend (issue #567, ADR 0001, ADR 0002): a real,
//! chain-backed [`SettlementBackend`] driven against
//! `packages/solana-program` -- the SPL-token, PDA-addressed payment
//! channel program the live fleet already settles through on
//! `solana:devnet` (program id `2aEVJ8koKD8LTZrLRSGtAtU7LBt4e7QjjCgf1kzQ7Rip`,
//! see `packages/solana-program/deployments/devnet-public.md`).
//!
//! `connector-settlement-solana-program` -- this crate's own former,
//! throwaway, native-SOL, signature-unverified channel program -- is gone.
//! Nothing here constructs, deploys or calls it anymore; [`wire`] speaks
//! `packages/solana-program`'s own wire directly (discriminators, PDA
//! seeds, account layouts, the Ed25519-precompile balance-proof check)
//! since that crate builds for the SBF target only and exports no client
//! SDK of its own.
//!
//! Like [`connector_settlement::InMemorySettlementBackend`] this holds no
//! local channel *ledger* -- every method reads the chain fresh before
//! deciding what the port's rules require. It does hold one small piece of
//! local memory, [`SolanaSettlementBackend`]'s `settled` set: `settle`
//! (`packages/solana-program/src/processor.rs:635-647`) zeroes the
//! channel PDA's lamports and data as its very last step, which is how a
//! genuinely rent-exempt Solana account is closed -- so a channel this
//! backend has itself driven to `Settled` no longer exists on chain at
//! all, and is indistinguishable from "never opened" without that memory.

mod submit;
#[cfg(feature = "test-util")]
pub mod test_support;
pub mod wire;

/// A settlement table's endpoint, which [`SolanaSettlementBackend::connect`]
/// takes in place of a URL (ADR 0073). Re-exported so a caller building one
/// does not need a second dependency to name it.
pub use connector_chain_rpc::RpcTransport;

use std::collections::HashSet;
use std::str::FromStr;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use chrono::Duration;

use connector_chain_rpc::{retry_read, solana::rpc_client};
use solana_rpc_client::nonblocking::rpc_client::RpcClient;
use solana_rpc_client::rpc_client::RpcClientConfig;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::genesis_config::ClusterType;
use solana_sdk::hash::Hash;
use solana_sdk::instruction::Instruction;
use solana_sdk::program_pack::Pack;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{Keypair, Signer as SolanaSigner};
use solana_sdk::transaction::Transaction;

use connector_settlement::{
    ChannelId, ChannelState, ChannelStatus, Claim, SettlementBackend, SettlementError,
};

use submit::{send_and_confirm, ConfirmPolicy};

/// A [`SettlementBackend`] backed by a real `packages/solana-program`
/// instance on a Solana chain.
pub struct SolanaSettlementBackend {
    rpc: RpcClient,
    /// How [`submit`](Self::submit) paces its confirmation polls: more
    /// slowly over a circuit (ADR 0073).
    confirm: ConfirmPolicy,
    program_id: Pubkey,
    /// This backend's own identity -- every channel it opens names this as
    /// one of the two on-chain participants, exactly like
    /// `EvmSettlementBackend::own_address` (issue #576's precedent), so
    /// [`counterparty_of`](Self::counterparty_of) can tell which side is
    /// "self" and which is the counterparty's.
    payer: Keypair,
    /// The SPL mint every channel this backend opens settles in --
    /// `[settlement] token_address` in production, a freshly created mock
    /// mint for [`deploy`](Self::deploy).
    token_mint: Pubkey,
    /// Present only for a [`deploy`](Self::deploy)-built backend:
    /// counterparty identities this backend privately holds keys for, so
    /// this crate's own tests can stand in for the external actor that
    /// would really deposit on the counterparty's side --
    /// [`test_fund_counterparty`](Self::test_fund_counterparty) -- and
    /// sign the claims this backend then redeems
    /// ([`test_sign_claim`](Self::test_sign_claim)). Two of them, because
    /// the deployed program holds exactly one live channel per (pair,
    /// mint) -- its channel PDA is seeded `["channel", min, max, mint]` --
    /// so a test needing two concurrently-live channels with real
    /// counterparty collateral needs two distinct identities with real
    /// keys.
    ///
    /// `packages/solana-program`'s `Deposit` instruction requires the
    /// depositing participant to sign for *themselves*
    /// (`processor.rs:309-311`, `:356-360`): there is no delegate-deposit
    /// path here, unlike `TokenNetwork.setTotalDeposit`, which lets any
    /// caller credit an arbitrary participant from the caller's own token
    /// balance. A `connect`-built production backend holds no such key and
    /// needs none: [`fund`](SettlementBackend::fund) is a *self*-deposit
    /// on both chains as of issue #1118, and the counterparty's own
    /// deposit is the counterparty's own transaction. The port's contract
    /// suite runs against a `connect`-built backend for exactly that
    /// reason (`tests/contract_suite.rs`) -- these keys are a convenience
    /// for this crate's other tests, not the thing that makes `fund`
    /// work.
    counterparty_signers: Vec<Keypair>,
    /// Channel PDAs this backend has itself driven
    /// [`settle`](SettlementBackend::settle) to completion on -- see this
    /// struct's own top-of-file doc for why the chain alone cannot answer
    /// "was this settled, or did it never exist" once that has happened.
    settled: Mutex<HashSet<Pubkey>>,
    /// Serializes [`fund`](SettlementBackend::fund) and
    /// [`fund_to`](SettlementBackend::fund_to): `fund_to` reads the own
    /// deposit and deposits the difference, and `Deposit` is an increment,
    /// so two of them interleaving would both deposit it (ADR 0073).
    deposit_lock: tokio::sync::Mutex<()>,
    /// Which Solana cluster the endpoint this backend connected to is
    /// actually on, read from the chain itself at
    /// [`connect`](Self::connect) -- see [`Self::cluster`] and
    /// [`cluster_for_genesis_hash`] (issue #1131).
    cluster: Option<&'static str>,
}

/// The public Solana cluster whose genesis block hashes to `genesis_hash`
/// -- `"mainnet-beta"`, `"devnet"` or `"testnet"` -- and `None` for a chain
/// that is none of the three (issue #1131).
///
/// A cluster's genesis hash is the one identity a Solana chain states about
/// itself. It is not a naming convention, so it holds however the node
/// reached the chain: `api.devnet.solana.com`, a Helius or Triton URL, an
/// SSH tunnel, a caching proxy, an IP literal. That is the whole reason this
/// exists next to `SolanaSettlementConfig::cluster_hint`, which can only
/// recognise a hostname it was told about in advance and answers `None` for
/// every paid RPC provider.
///
/// The three hashes are **not** written down here. They come from
/// [`ClusterType::get_genesis_hash`], `solana-sdk`'s own table, so the
/// values track the pinned SDK rather than this repository's memory of
/// them; `cluster_names_match_the_published_genesis_hashes` in this
/// module's tests pins that table to the base58 the public RPC endpoints
/// answer with, so an SDK bump that moved a value would fail the gate
/// rather than silently relabel a chain.
///
/// # `None` is a chain this connector cannot name, not an error
///
/// `ClusterType::Development` -- a `solana-test-validator`, which mints a
/// fresh genesis on every run, and therefore every `local/` topology and
/// every tier-3 test in this workspace -- has no published hash and can
/// never have one. It answers `None`, which is exactly what `cluster_hint`
/// already answers for an unrecognised host, and means the same thing: this
/// node cannot say which cluster it is on, so it compares nothing rather
/// than guessing. Refusing here would refuse to boot on every local
/// topology.
pub fn cluster_for_genesis_hash(genesis_hash: &Hash) -> Option<&'static str> {
    [
        (ClusterType::MainnetBeta, "mainnet-beta"),
        (ClusterType::Devnet, "devnet"),
        (ClusterType::Testnet, "testnet"),
    ]
    .into_iter()
    .find(|(cluster, _)| cluster.get_genesis_hash().as_ref() == Some(genesis_hash))
    .map(|(_, name)| name)
}

impl SolanaSettlementBackend {
    /// Bind to an already-deployed `packages/solana-program` instance at
    /// `program_id`, settling in `token_mint`, signing every transaction
    /// with the keypair `payer_seed` derives (a 32-byte ed25519 seed, the
    /// same key-file shape `[settlement] key` already resolves for the EVM
    /// backend's private key).
    ///
    /// Refuses -- naming the address -- if `program_id` names no
    /// executable account, or `token_mint` is not owned by the SPL Token
    /// program: the coarse, "did I misconfigure this entirely" check issue
    /// #567 made on its own. Executable alone is not identity -- a typo'd
    /// `program_id` naming any *other* real program (SPL Token itself,
    /// say) would pass it and fail lazily at the first settle -- so
    /// `connect` also proves the program actually behaves like the
    /// deployed payment-channel program before this node serves traffic
    /// ([`Self::verify_program_identity`]), the Solana twin of
    /// `EvmSettlementBackend::connect` proving its contract by calling
    /// `get_token_network` on it (issue #630's review).
    ///
    /// `expected_decimals` is the scale the operator wrote down
    /// (`[settlement.solana] decimals`), the Solana twin of
    /// `EvmSettlementBackend::connect`'s own `expected_decimals` (issue
    /// #564): nothing here scales by it -- every amount this backend moves
    /// is already in the mint's own base units -- so it is checked rather
    /// than applied. `connect` reads the mint's own `decimals` field and
    /// refuses, naming both values, when they disagree (issue #630, ADR
    /// 0009): the fuller "does the fleet's own identity match what's
    /// deployed" check issue #567 deferred here.
    ///
    /// It also asks the endpoint which chain it is on
    /// ([`Self::cluster`], issue #1131) -- one `getGenesisHash` read
    /// beside the identity reads already here. Unlike them it never
    /// refuses: a chain this connector cannot name is recorded as
    /// unnamed, never rejected, because `solana-test-validator` is
    /// unnameable by construction and every `local/` topology runs on one.
    ///
    /// Every read here is retried with backoff before it fails the node
    /// ([`retry_read`], ADR 0073 decision 5): boot is when a circuit is
    /// youngest, and one lost round trip used to be a failed start.
    pub async fn connect(
        transport: &RpcTransport,
        payer_seed: &[u8; 32],
        program_id: Pubkey,
        token_mint: Pubkey,
        expected_decimals: u8,
    ) -> Result<Self, SettlementError> {
        let payer = solana_sdk::signer::keypair::keypair_from_seed(payer_seed)
            .map_err(|error| SettlementError::Backend(error.to_string()))?;
        let rpc = rpc_client(
            transport,
            RpcClientConfig::with_commitment(CommitmentConfig::confirmed()),
        );

        let program_account = retry_read(|| rpc.get_account(&program_id))
            .await
            .map_err(backend_error)?;
        if !program_account.executable {
            return Err(SettlementError::Backend(format!(
                "settlement program_id {program_id} is not an executable program account"
            )));
        }
        let mint_account = retry_read(|| rpc.get_account(&token_mint))
            .await
            .map_err(backend_error)?;
        if mint_account.owner != spl_token::id() {
            return Err(SettlementError::Backend(format!(
                "settlement token_address {token_mint} is not owned by the SPL Token program"
            )));
        }
        let mint = spl_token::state::Mint::unpack(&mint_account.data).map_err(backend_error)?;
        if mint.decimals != expected_decimals {
            return Err(SettlementError::Backend(format!(
                "[settlement.solana] decimals is {expected_decimals}, but mint {token_mint} \
                 reports decimals = {}",
                mint.decimals
            )));
        }

        // The chain's own answer to "which cluster am I", read once here
        // rather than guessed from `rpc_url` forever after (issue #1131).
        // Ordered with the other identity reads and, like them, a failed
        // read refuses the connection -- which costs nothing this
        // `connect` did not already cost, since the three reads above have
        // already failed by now if the endpoint is unreachable.
        let cluster =
            cluster_for_genesis_hash(&retry_read(|| rpc.get_genesis_hash()).await.map_err(
                |error| {
                    SettlementError::Backend(format!(
                        "could not read the cluster's genesis hash: {error}"
                    ))
                },
            )?);

        // Read rather than inferred from a transaction. Until ADR 0073 the
        // ATA create below ran on every start and doubled as proof that the
        // payer held lamports; it now runs once in a key's life, so an
        // unfunded payer is named here instead of surfacing as a failed
        // program-identity probe.
        let payer_pubkey = payer.pubkey();
        let lamports = retry_read(|| rpc.get_balance(&payer_pubkey))
            .await
            .map_err(backend_error)?;
        if lamports == 0 {
            return Err(SettlementError::Backend(format!(
                "[settlement.solana] payer {} holds no lamports, so it cannot pay for any \
                 settlement transaction; fund it before starting the node",
                payer.pubkey()
            )));
        }

        let backend = Self {
            rpc,
            confirm: ConfirmPolicy::for_transport(transport),
            program_id,
            payer,
            token_mint,
            counterparty_signers: Vec::new(),
            settled: Mutex::new(HashSet::new()),
            deposit_lock: tokio::sync::Mutex::new(()),
            cluster,
        };
        // The payer's lamports were read above, so a probe failure below
        // means program identity, never an unfunded payer reported as the
        // wrong program.
        backend.ensure_own_ata_exists().await?;
        backend.verify_program_identity().await?;
        Ok(backend)
    }

    /// Prove the account at `program_id` actually implements the deployed
    /// payment-channel program, not merely that *something* executable
    /// lives there (issue #630's review, finding 2): simulate -- never
    /// submit -- an `InitializeChannel` against a freshly generated,
    /// throwaway counterparty, whose channel PDA therefore cannot exist,
    /// and require the simulation to succeed. Only the real program
    /// accepts that instruction: it must recognize the discriminator,
    /// derive the same `["channel", ..]`/`["vault", ..]` PDAs from the
    /// same seeds, and drive the create-account and SPL-token CPIs to
    /// completion -- any other executable program (SPL Token itself, say)
    /// rejects the instruction data or its accounts. The counterparty is
    /// random rather than fixed so nobody can pre-create the probe's PDA
    /// on a public cluster and wedge this node's startup.
    ///
    /// Refusing here, at connect, is the point: the alternative is a node
    /// that starts clean against a typo'd `program_id` and discovers it at
    /// its first settle, hours later, with claims already accepted (the
    /// `#564` fail-closed pattern, ADR 0009).
    async fn verify_program_identity(&self) -> Result<(), SettlementError> {
        let own = self.payer.pubkey();
        let probe_counterparty = Keypair::new().pubkey();
        let (channel, _bump) = wire::channel_pda(
            &own,
            &probe_counterparty,
            &self.token_mint,
            &self.program_id,
        );
        let (vault, _bump) = wire::vault_pda(&channel, &self.program_id);
        let instruction = Instruction::new_with_bytes(
            self.program_id,
            &wire::pack_initialize_channel(3600),
            wire::Accounts::initialize_channel(
                &own,
                &own,
                &probe_counterparty,
                &self.token_mint,
                &channel,
                &vault,
            ),
        );
        let recent_blockhash = retry_read(|| self.rpc.get_latest_blockhash())
            .await
            .map_err(backend_error)?;
        let transaction = Transaction::new_signed_with_payer(
            &[instruction],
            Some(&own),
            &[&self.payer],
            recent_blockhash,
        );
        let simulation = retry_read(|| self.rpc.simulate_transaction(&transaction))
            .await
            .map_err(backend_error)?;
        if let Some(error) = simulation.value.err {
            return Err(SettlementError::Backend(format!(
                "settlement program_id {} is executable but rejected a simulated \
                 payment-channel InitializeChannel ({error}) -- it does not behave like the \
                 deployed packages/solana-program payment-channel program, so this node \
                 refuses to start rather than fail at its first settlement",
                self.program_id
            )));
        }
        Ok(())
    }

    /// Bind to `program_id` (already loaded into the target validator's
    /// genesis -- see this crate's `test_support` module), creating a
    /// fresh mock SPL mint and airdropping and funding this backend's own
    /// identity and two privately-held counterparty identities (see
    /// [`counterparty_signers`](Self::counterparty_signers)'s doc for why
    /// two) with test tokens. Used by this crate's own tests, mirroring
    /// `EvmSettlementBackend::deploy`'s role against `anvil` -- never used
    /// against a real chain, where the mint and every identity already
    /// exist.
    pub async fn deploy(rpc_url: &str, program_id: Pubkey) -> Result<Self, SettlementError> {
        let transport = RpcTransport::direct(rpc_url).map_err(backend_error)?;
        let rpc = rpc_client(
            &transport,
            RpcClientConfig::with_commitment(CommitmentConfig::confirmed()),
        );
        let payer = Keypair::new();
        let counterparties = [Keypair::new(), Keypair::new()];
        for keypair in [&payer, &counterparties[0], &counterparties[1]] {
            let signature = rpc
                .request_airdrop(&keypair.pubkey(), 100_000_000_000)
                .await
                .map_err(backend_error)?;
            wait_for_confirmation(&rpc, &signature).await?;
        }

        let mint = Keypair::new();
        let rent = rpc
            .get_minimum_balance_for_rent_exemption(spl_token::state::Mint::LEN)
            .await
            .map_err(backend_error)?;
        let create_mint_account = solana_sdk::system_instruction::create_account(
            &payer.pubkey(),
            &mint.pubkey(),
            rent,
            spl_token::state::Mint::LEN as u64,
            &spl_token::id(),
        );
        let initialize_mint = spl_token::instruction::initialize_mint2(
            &spl_token::id(),
            &mint.pubkey(),
            &payer.pubkey(),
            None,
            6,
        )
        .map_err(backend_error)?;
        let recent_blockhash = rpc.get_latest_blockhash().await.map_err(backend_error)?;
        let transaction = Transaction::new_signed_with_payer(
            &[create_mint_account, initialize_mint],
            Some(&payer.pubkey()),
            &[&payer, &mint],
            recent_blockhash,
        );
        rpc.send_and_confirm_transaction(&transaction)
            .await
            .map_err(backend_error)?;

        let backend = Self {
            rpc,
            confirm: ConfirmPolicy::for_transport(&transport),
            program_id,
            payer,
            token_mint: mint.pubkey(),
            counterparty_signers: counterparties.into(),
            settled: Mutex::new(HashSet::new()),
            deposit_lock: tokio::sync::Mutex::new(()),
            // A `deploy`-built backend only ever runs against this
            // workspace's own `solana-test-validator`, whose fresh genesis
            // no published hash can match -- so the read is skipped rather
            // than spent to learn `None` (issue #1131).
            cluster: None,
        };
        backend.ensure_own_ata_exists().await?;
        backend
            .mint_test_tokens_to(&backend.payer.pubkey(), 1_000_000_000)
            .await?;
        let counterparty_pubkeys: Vec<Pubkey> = backend
            .counterparty_signers
            .iter()
            .map(|keypair| keypair.pubkey())
            .collect();
        for counterparty_pubkey in counterparty_pubkeys {
            backend
                .test_mint_tokens_to(&counterparty_pubkey, 1_000_000_000)
                .await?;
        }

        Ok(backend)
    }

    /// Which Solana cluster this backend is settling on, as the chain
    /// itself stated at [`connect`](Self::connect) -- and `None` for a
    /// chain none of the three public clusters' genesis hashes match, which
    /// is every `solana-test-validator` (issue #1131,
    /// [`cluster_for_genesis_hash`]).
    ///
    /// This is the authoritative answer to a question
    /// `SolanaSettlementConfig::cluster_hint` can only guess at from the
    /// configured URL's hostname, and it is what a Solana claim's
    /// self-declared `cluster` is compared against (issue #975): a node
    /// behind a paid RPC provider gets a real cluster here where the hint
    /// gets nothing.
    pub fn cluster(&self) -> Option<&'static str> {
        self.cluster
    }

    /// This backend's own signing address -- the on-chain identity every
    /// channel it opens names as one of the two participants.
    pub fn own_pubkey(&self) -> Pubkey {
        self.payer.pubkey()
    }

    /// The SPL mint every channel this backend opens settles in.
    pub fn token_mint(&self) -> Pubkey {
        self.token_mint
    }

    /// The deployed `payment-channel` program instance this backend drives
    /// (issue #632's greeting facts -- the Solana twin of
    /// `EvmSettlementBackend::address`/`registry_address`).
    pub fn program_id(&self) -> Pubkey {
        self.program_id
    }

    /// Who this backend's counterparty is on the channel at `channel_account`,
    /// as the chain itself holds it -- the Solana twin of
    /// `EvmSettlementBackend::channel_counterparty` (issue #611), and the
    /// client edge's own channel record for a Solana channel nothing was
    /// declared for (issue #629/#631).
    ///
    /// `Ok(None)`, rather than an error, for every "this is not a channel
    /// this backend can be paid on":
    ///
    /// - nothing was ever opened at `channel_account`, or it has already
    ///   `Settled` (`redeem` requires `Opened` or `Closed` --
    ///   [`SolanaSettlementBackend`]'s own top-of-file doc on why a settled
    ///   channel's account does not even exist to be fetched -- and honouring
    ///   a claim against it would be giving the app's work away; a merely
    ///   `Closed` channel still redeems during its challenge window (issue
    ///   #574), so it still answers);
    /// - neither participant is this backend's own signing address, i.e. it
    ///   is somebody else's channel;
    /// - its `token_mint` is not the mint this backend is configured to
    ///   settle in. The deployed program lets any payer open a channel
    ///   with ANY mint, and the balance-proof signature does not cover the
    ///   mint (`connector_signer::solana_balance_proof_message` signs
    ///   channel account, nonce and amount alone), so this resolution
    ///   check is the one place the mint is bound: without it, a claim on
    ///   a channel funded with a worthless SPL token would buy
    ///   USDC-priced writes.
    ///
    /// `Err` is reserved for a lookup that genuinely failed -- an
    /// unreachable endpoint, a malformed response -- so a caller can tell
    /// "there is no such channel" apart from "I could not find out".
    pub async fn channel_counterparty(
        &self,
        channel_account: Pubkey,
    ) -> Result<Option<Pubkey>, SettlementError> {
        Ok(self
            .channel_counterparty_deposit(channel_account)
            .await?
            .map(|(counterparty, _deposit)| counterparty))
    }

    /// [`channel_counterparty`](Self::channel_counterparty), plus the
    /// deposit that counterparty has actually put into the channel's vault
    /// -- the bound `packages/solana-program`'s claim handler enforces at
    /// redemption (`processor.rs:781-788`,
    /// `TransferredAmountExceedsDeposit`), and therefore the most a claim
    /// this backend resolves can ever be worth (issue #646).
    ///
    /// **No extra RPC.** `deposit_a`/`deposit_b` are already decoded out of
    /// the same account bytes the counterparty comes from
    /// ([`wire::ChannelAccount::parse`]); before this they were parsed and
    /// thrown away. The client edge asks for both together so the number is
    /// carried through the resolution seam instead of re-read there.
    pub async fn channel_counterparty_deposit(
        &self,
        channel_account: Pubkey,
    ) -> Result<Option<(Pubkey, u64)>, SettlementError> {
        let Some(account) = self.fetch_account(&channel_account).await? else {
            return Ok(None);
        };
        Ok(Self::resolvable_counterparty(
            &account,
            self.payer.pubkey(),
            self.token_mint,
        ))
    }

    /// The pure per-account half of
    /// [`channel_counterparty`](Self::channel_counterparty): given a parsed
    /// channel account, decide whether it is a channel a backend with
    /// identity `own` settling in `token_mint` can be paid on, and if so by
    /// whom -- and how much *they* have deposited (issue #646), which is
    /// the side of the two-sided deposit a claim signed by them redeems
    /// against. Extracted so every refusal branch -- settled, wrong mint,
    /// not a participant -- is unit-testable against a fabricated account
    /// without a validator.
    fn resolvable_counterparty(
        account: &wire::ChannelAccount,
        own: Pubkey,
        token_mint: Pubkey,
    ) -> Option<(Pubkey, u64)> {
        if account.status == wire::ChannelStatus::Settled {
            return None;
        }
        if account.token_mint != token_mint {
            return None;
        }
        if account.participant_a == own {
            Some((account.participant_b, account.deposit_b))
        } else if account.participant_b == own {
            Some((account.participant_a, account.deposit_a))
        } else {
            None
        }
    }

    /// Test/dev-only accessor (issue #631's mint-binding review): the
    /// 32-byte ed25519 seed of this backend's own signing keypair, so a
    /// test can [`connect`](Self::connect) a second backend under the SAME
    /// on-chain identity but a different `token_mint` -- the shape the
    /// mint-binding check in
    /// [`channel_counterparty`](Self::channel_counterparty) exists to
    /// refuse.
    pub fn test_payer_seed(&self) -> [u8; 32] {
        self.payer.to_bytes()[..32]
            .try_into()
            .expect("a Keypair's first 32 bytes are its seed")
    }

    /// Test/dev-only accessor (issue #567): the pubkey bytes of the first
    /// counterparty identity a [`deploy`](Self::deploy)-built backend
    /// privately holds a key for -- what this crate's own tests pass as
    /// [`connector_settlement::contract::ContractFixture::counterparty`].
    /// `None` for a [`connect`](Self::connect)-built production backend.
    pub fn test_counterparty_pubkey(&self) -> Option<Vec<u8>> {
        self.counterparty_signers
            .first()
            .map(|keypair| keypair.pubkey().to_bytes().to_vec())
    }

    /// Test/dev-only accessor (issue #567): the pubkey bytes of the second
    /// held counterparty identity -- what this crate's own tests pass as
    /// [`connector_settlement::contract::ContractFixture::instant_counterparty`],
    /// distinct from [`test_counterparty_pubkey`](Self::test_counterparty_pubkey)'s
    /// because the deployed program holds one live channel per (pair, mint)
    /// (see [`counterparty_signers`](Self::counterparty_signers)'s doc).
    /// `None` for a [`connect`](Self::connect)-built production backend.
    pub fn test_instant_counterparty_pubkey(&self) -> Option<Vec<u8>> {
        self.counterparty_signers
            .get(1)
            .map(|keypair| keypair.pubkey().to_bytes().to_vec())
    }

    /// Test/dev-only accessor (issue #567): sign the balance-proof message
    /// for `channel`, `nonce` and `cumulative_amount` as whichever held
    /// counterparty identity `channel` was opened against -- the Ed25519
    /// signature `redeem`'s `Claim::signature` must carry for the deployed
    /// program's precompile check to accept. Which identity that is is
    /// re-derived rather than looked up: the channel PDA is a pure function
    /// of (own identity, counterparty, mint), so the held key whose derived
    /// PDA matches `channel`'s id is the channel's counterparty. `None` for
    /// a [`connect`](Self::connect)-built production backend, or if no held
    /// key derives `channel`'s id.
    pub fn test_sign_claim(
        &self,
        channel: &ChannelId,
        nonce: u64,
        cumulative_amount: u128,
    ) -> Option<Vec<u8>> {
        let pubkey = Pubkey::from_str(&channel.0).ok()?;
        let counterparty = self.counterparty_signers.iter().find(|keypair| {
            wire::channel_pda(
                &self.payer.pubkey(),
                &keypair.pubkey(),
                &self.token_mint,
                &self.program_id,
            )
            .0 == pubkey
        })?;
        let units = to_units(cumulative_amount).ok()?;
        let message = wire::balance_proof_message(&self.program_id, &pubkey, nonce, units);
        Some(counterparty.sign_message(&message).as_ref().to_vec())
    }

    /// Test/dev-only (issue #1118): mint `amount` of this backend's mock
    /// mint into `owner`'s associated token account, creating that account
    /// first if it does not exist. Only a [`deploy`](Self::deploy)-built
    /// backend is the mint's authority, so only one of those can do this.
    ///
    /// The token faucet a test needs once `fund` is a *self*-deposit: a
    /// [`connect`](Self::connect)-built backend spends its **own** tokens
    /// to collateralise a channel, so a test standing one up has to put
    /// real tokens in its ATA first -- exactly as a real deployment has to
    /// (see `local/keys.sh`, which mints mock USDC to each node's EVM
    /// settlement address and, as of issue #1118, must do the same for its
    /// Solana one).
    pub async fn test_mint_tokens_to(
        &self,
        owner: &Pubkey,
        amount: u64,
    ) -> Result<(), SettlementError> {
        let create_ata =
            spl_associated_token_account::instruction::create_associated_token_account_idempotent(
                &self.payer.pubkey(),
                owner,
                &self.token_mint,
                &spl_token::id(),
            );
        self.submit(&[create_ata], &[]).await?;
        self.mint_test_tokens_to(owner, amount).await
    }

    /// Test/dev-only (issue #1118): deposit `amount` on the
    /// **counterparty's** own side of `channel`, signed by whichever held
    /// counterparty identity is that channel's participant -- the external
    /// actor a real deployment supplies and this crate's own tests must
    /// stand in for. What this crate wires
    /// `connector_settlement::contract::ContractFixture::fund_counterparty`
    /// to for a [`deploy`](Self::deploy)-built backend.
    ///
    /// This is the one deposit the deployed program will not let a node
    /// make for somebody else: `Deposit` credits by signer
    /// (`processor.rs:356-360`), so this works only because
    /// [`deploy`](Self::deploy) privately holds the counterparty's key. It
    /// is emphatically not a production path -- `fund` is the self-deposit
    /// every real node uses.
    ///
    /// [`SettlementError::Backend`] on a [`connect`](Self::connect)-built
    /// backend, which holds no such key.
    pub async fn test_fund_counterparty(
        &self,
        channel: &ChannelId,
        amount: u128,
    ) -> Result<ChannelState, SettlementError> {
        let (pubkey, account) = self.open_channel(channel).await?;
        let Some(counterparty_signer) = self.counterparty_signers.iter().find(|keypair| {
            let counterparty_pubkey = keypair.pubkey();
            account.participant_a == counterparty_pubkey
                || account.participant_b == counterparty_pubkey
        }) else {
            return Err(SettlementError::Backend(
                "none of this backend's held counterparty keys is a participant of this channel \
                 -- only a deploy()-built backend holds any, and depositing on a counterparty's \
                 behalf is a test fixture's job, never a node's"
                    .to_string(),
            ));
        };
        let counterparty_pubkey = counterparty_signer.pubkey();
        let units = to_units(amount)?;
        let depositor_token_account = spl_associated_token_account::get_associated_token_address(
            &counterparty_pubkey,
            &self.token_mint,
        );
        let (vault, _bump) = wire::vault_pda(&pubkey, &self.program_id);

        let instruction = Instruction::new_with_bytes(
            self.program_id,
            &wire::pack_deposit(units),
            wire::Accounts::deposit(
                &counterparty_pubkey,
                &depositor_token_account,
                &vault,
                &pubkey,
            ),
        );
        self.submit(&[instruction], &[counterparty_signer]).await?;

        let (_pubkey, account) = self.open_channel(channel).await?;
        self.to_channel_state(channel, &account)
    }

    /// Create this backend's own associated token account if, and only if,
    /// it does not exist yet (ADR 0073 decision 5).
    ///
    /// Read first. The create is idempotent on chain, so this used to submit
    /// it on every start, which made every boot a real transaction and every
    /// boot's success depend on confirming one. Now a key's first start
    /// creates the account and every later start reads it and moves on.
    async fn ensure_own_ata_exists(&self) -> Result<(), SettlementError> {
        let own_ata = spl_associated_token_account::get_associated_token_address(
            &self.payer.pubkey(),
            &self.token_mint,
        );
        let existing = retry_read(|| {
            self.rpc
                .get_account_with_commitment(&own_ata, CommitmentConfig::confirmed())
        })
        .await
        .map_err(backend_error)?;
        if existing.value.is_some() {
            return Ok(());
        }
        let instruction =
            spl_associated_token_account::instruction::create_associated_token_account_idempotent(
                &self.payer.pubkey(),
                &self.payer.pubkey(),
                &self.token_mint,
                &spl_token::id(),
            );
        self.submit(&[instruction], &[]).await
    }

    async fn mint_test_tokens_to(
        &self,
        owner: &Pubkey,
        amount: u64,
    ) -> Result<(), SettlementError> {
        let destination =
            spl_associated_token_account::get_associated_token_address(owner, &self.token_mint);
        let instruction = spl_token::instruction::mint_to(
            &spl_token::id(),
            &self.token_mint,
            &destination,
            &self.payer.pubkey(),
            &[],
            amount,
        )
        .map_err(backend_error)?;
        self.submit(&[instruction], &[]).await
    }

    /// Sign `instructions` over a fresh blockhash, send them, and wait until
    /// their outcome is known ([`submit::send_and_confirm`], ADR 0073
    /// decision 5). An error from here names the transaction's signature
    /// and says whether it landed, failed, expired or could not be read.
    async fn submit(
        &self,
        instructions: &[Instruction],
        extra_signers: &[&Keypair],
    ) -> Result<(), SettlementError> {
        // Nothing has been sent yet, so a failed read is simply retried.
        let (recent_blockhash, last_valid_block_height) = retry_read(|| {
            self.rpc
                .get_latest_blockhash_with_commitment(CommitmentConfig::confirmed())
        })
        .await
        .map_err(backend_error)?;
        let mut signers: Vec<&Keypair> = vec![&self.payer];
        signers.extend_from_slice(extra_signers);
        let transaction = Transaction::new_signed_with_payer(
            instructions,
            Some(&self.payer.pubkey()),
            &signers,
            recent_blockhash,
        );
        send_and_confirm(
            &self.rpc,
            &transaction,
            last_valid_block_height,
            self.confirm,
        )
        .await?;
        Ok(())
    }

    /// Deposit `amount` of this backend's own tokens into the channel at
    /// `pubkey`, crediting its own side, and return the state after. The
    /// caller holds `deposit_lock` and has checked the channel is open.
    async fn deposit_own(
        &self,
        channel: &ChannelId,
        pubkey: Pubkey,
        amount: u128,
    ) -> Result<ChannelState, SettlementError> {
        let own = self.payer.pubkey();
        let units = to_units(amount)?;
        let depositor_token_account =
            spl_associated_token_account::get_associated_token_address(&own, &self.token_mint);
        let (vault, _bump) = wire::vault_pda(&pubkey, &self.program_id);

        let instruction = Instruction::new_with_bytes(
            self.program_id,
            &wire::pack_deposit(units),
            wire::Accounts::deposit(&own, &depositor_token_account, &vault, &pubkey),
        );
        self.submit(&[instruction], &[]).await?;

        let (_pubkey, account) = self.open_channel(channel).await?;
        self.to_channel_state(channel, &account)
    }

    /// Fetch and parse the `ChannelState` account at `pubkey`, or `None`
    /// if nothing this program owns lives there.
    async fn fetch_account(
        &self,
        pubkey: &Pubkey,
    ) -> Result<Option<wire::ChannelAccount>, SettlementError> {
        let response = self
            .rpc
            .get_account_with_commitment(pubkey, CommitmentConfig::confirmed())
            .await
            .map_err(backend_error)?;
        let Some(account) = response.value else {
            return Ok(None);
        };
        if account.owner != self.program_id {
            return Ok(None);
        }
        Ok(wire::ChannelAccount::parse(&account.data))
    }

    /// Resolve `channel` to its on-chain pubkey and current state: either
    /// a live, parsed account, or -- if nothing lives there but this
    /// backend itself remembers settling it -- the fact that it was
    /// settled and its account is now gone (this struct's own top-of-file
    /// doc). Anything else nothing was ever opened at is
    /// [`SettlementError::ChannelNotFound`].
    async fn resolve(&self, channel: &ChannelId) -> Result<(Pubkey, Resolved), SettlementError> {
        let pubkey = Pubkey::from_str(&channel.0)
            .map_err(|_| SettlementError::ChannelNotFound(channel.clone()))?;
        match self.fetch_account(&pubkey).await? {
            Some(account) => Ok((pubkey, Resolved::Live(account))),
            None => {
                if self
                    .settled
                    .lock()
                    .expect("settled mutex poisoned")
                    .contains(&pubkey)
                {
                    Ok((pubkey, Resolved::Settled))
                } else {
                    Err(SettlementError::ChannelNotFound(channel.clone()))
                }
            }
        }
    }

    /// [`resolve`](Self::resolve), then reject anything but a live
    /// `Opened` channel -- the precondition [`fund`](SettlementBackend::fund)
    /// and [`close`](SettlementBackend::close) share.
    /// [`redeem`](SettlementBackend::redeem) uses
    /// [`redeemable_channel`](Self::redeemable_channel) instead, since it
    /// still succeeds against a `Closed` channel (issue #574).
    async fn open_channel(
        &self,
        channel: &ChannelId,
    ) -> Result<(Pubkey, wire::ChannelAccount), SettlementError> {
        let (pubkey, resolved) = self.resolve(channel).await?;
        match resolved {
            Resolved::Live(account) => match account.status {
                wire::ChannelStatus::Opened => Ok((pubkey, account)),
                wire::ChannelStatus::Closed => Err(SettlementError::ChannelClosed(channel.clone())),
                wire::ChannelStatus::Settled => {
                    Err(SettlementError::ChannelSettled(channel.clone()))
                }
            },
            Resolved::Settled => Err(SettlementError::ChannelSettled(channel.clone())),
        }
    }

    /// [`resolve`](Self::resolve), then reject only a settled channel
    /// (issue #574) -- the shared precondition/read-back for anything
    /// that still works against a `Closed` channel:
    /// [`redeem`](SettlementBackend::redeem)'s precondition and post-submit
    /// read-back, and [`close`](SettlementBackend::close)'s post-submit
    /// read-back (the channel is expected to be `Closed` by then, but
    /// accepting `Opened` too costs nothing and avoids a near-identical
    /// third check).
    async fn redeemable_channel(
        &self,
        channel: &ChannelId,
    ) -> Result<(Pubkey, wire::ChannelAccount), SettlementError> {
        let (pubkey, resolved) = self.resolve(channel).await?;
        match resolved {
            Resolved::Live(account) if account.status == wire::ChannelStatus::Settled => {
                Err(SettlementError::ChannelSettled(channel.clone()))
            }
            Resolved::Live(account) => Ok((pubkey, account)),
            Resolved::Settled => Err(SettlementError::ChannelSettled(channel.clone())),
        }
    }

    /// Whether this backend's own signing address is `account`'s
    /// participant A (`true`) or participant B (`false`) --
    /// [`SettlementError::Backend`] if it is neither, which should never
    /// happen for a channel this backend itself opened. The one place
    /// [`counterparty_of`](Self::counterparty_of) and
    /// [`to_channel_state`](Self::to_channel_state) both need to know
    /// which side is "self".
    fn own_is_participant_a(
        &self,
        account: &wire::ChannelAccount,
    ) -> Result<bool, SettlementError> {
        let own = self.payer.pubkey();
        if account.participant_a == own {
            Ok(true)
        } else if account.participant_b == own {
            Ok(false)
        } else {
            Err(SettlementError::Backend(format!(
                "channel participants {}/{} include neither this backend's own signing address {own}",
                account.participant_a, account.participant_b
            )))
        }
    }

    /// Which side of `account` is the counterparty's identity (the other
    /// side from [`own_is_participant_a`](Self::own_is_participant_a)).
    fn counterparty_of(&self, account: &wire::ChannelAccount) -> Result<Pubkey, SettlementError> {
        Ok(if self.own_is_participant_a(account)? {
            account.participant_b
        } else {
            account.participant_a
        })
    }

    /// Derive a single [`ChannelState`] from `account`'s two-sided shape
    /// (mirrors `EvmSettlementBackend::read_state`'s own doc):
    /// `counterparty_deposited` and `redeemed` are the counterparty's own
    /// `deposit_x` and `transferred_amount_x` -- what a claim signed by
    /// the counterparty is bounded against on chain
    /// (`processor.rs:770-789`), and what `redeem` actually advances
    /// (`processor.rs:800-807`) -- while `own_deposited` is this backend's
    /// own side of the same pair (issue #1118), what
    /// [`fund`](SettlementBackend::fund) raises and what `SettleChannel`
    /// eventually pays back out.
    fn to_channel_state(
        &self,
        channel: &ChannelId,
        account: &wire::ChannelAccount,
    ) -> Result<ChannelState, SettlementError> {
        let (counterparty, deposited, own_deposited, redeemed) =
            if self.own_is_participant_a(account)? {
                (
                    account.participant_b,
                    account.deposit_b,
                    account.deposit_a,
                    account.transferred_amount_b,
                )
            } else {
                (
                    account.participant_a,
                    account.deposit_a,
                    account.deposit_b,
                    account.transferred_amount_a,
                )
            };
        Ok(ChannelState {
            id: channel.clone(),
            counterparty: counterparty.to_bytes().to_vec(),
            status: match account.status {
                wire::ChannelStatus::Opened => ChannelStatus::Open,
                wire::ChannelStatus::Closed => ChannelStatus::Closed,
                wire::ChannelStatus::Settled => ChannelStatus::Settled,
            },
            counterparty_deposited: deposited as u128,
            own_deposited: own_deposited as u128,
            redeemed: redeemed as u128,
        })
    }
}

enum Resolved {
    Live(wire::ChannelAccount),
    Settled,
}

async fn wait_for_confirmation(
    rpc: &RpcClient,
    signature: &solana_sdk::signature::Signature,
) -> Result<(), SettlementError> {
    for _ in 0..200 {
        if rpc.confirm_transaction(signature).await.unwrap_or(false) {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    Err(SettlementError::Backend(
        "transaction did not confirm in time".to_string(),
    ))
}

fn backend_error<E: std::fmt::Display>(error: E) -> SettlementError {
    SettlementError::Backend(error.to_string())
}

/// The port's contract suite deals in small dimensionless "amount" units,
/// which this backend passes straight through as SPL token base units with
/// no scaling: unlike the native-lamport channel this crate used to drive
/// (which needed a large multiplier to clear Solana's rent-exempt minimum
/// balance on every account), an SPL token account's own `amount` field
/// carries no such minimum -- only the *account's* lamports do, which this
/// backend never touches per unit deposited.
fn to_units(amount: u128) -> Result<u64, SettlementError> {
    u64::try_from(amount).map_err(|_| {
        SettlementError::Backend(format!(
            "amount {amount} does not fit in a u64 SPL token amount"
        ))
    })
}

#[async_trait]
impl SettlementBackend for SolanaSettlementBackend {
    async fn open(
        &self,
        counterparty: Vec<u8>,
        settlement_timeout: Duration,
    ) -> Result<ChannelId, SettlementError> {
        let counterparty_pubkey = Pubkey::try_from(counterparty.as_slice()).map_err(|_| {
            SettlementError::Backend(format!(
                "a packages/solana-program counterparty must be a 32-byte Solana pubkey, got {} bytes",
                counterparty.len()
            ))
        })?;
        let own = self.payer.pubkey();
        if counterparty_pubkey == own {
            return Err(SettlementError::Backend(
                "counterparty must differ from this backend's own signing address".to_string(),
            ));
        }
        let (channel, _bump) = wire::channel_pda(
            &own,
            &counterparty_pubkey,
            &self.token_mint,
            &self.program_id,
        );
        let (vault, _bump) = wire::vault_pda(&channel, &self.program_id);
        let seconds = settlement_timeout.num_seconds().max(0) as u64;

        let instruction = Instruction::new_with_bytes(
            self.program_id,
            &wire::pack_initialize_channel(seconds),
            wire::Accounts::initialize_channel(
                &own,
                &own,
                &counterparty_pubkey,
                &self.token_mint,
                &channel,
                &vault,
            ),
        );
        self.submit(&[instruction], &[]).await?;
        Ok(ChannelId(channel.to_string()))
    }

    /// A **self**-deposit (issue #1118): a `Deposit` signed by this
    /// backend's own `[settlement.solana]` identity, moving `amount` from
    /// the associated token account [`connect`](SolanaSettlementBackend::connect)
    /// already creates at boot into the channel's vault PDA, crediting
    /// this node's own side.
    ///
    /// This method used to attempt a deposit into the *counterparty's*
    /// side and therefore could not work at all on a
    /// [`connect`](SolanaSettlementBackend::connect)-built backend --
    /// which is every real node -- because `packages/solana-program`'s
    /// `Deposit` credits strictly by signer (`processor.rs:309-311`,
    /// `:356-360`) and no node holds its counterparty's key. The
    /// instruction was never missing; the port asked for the wrong one.
    /// Depositing on a counterparty's behalf is now the fixture-only
    /// [`test_fund_counterparty`](SolanaSettlementBackend::test_fund_counterparty).
    async fn fund(
        &self,
        channel: &ChannelId,
        amount: u128,
    ) -> Result<ChannelState, SettlementError> {
        let _guard = self.deposit_lock.lock().await;
        let (pubkey, _account) = self.open_channel(channel).await?;
        self.deposit_own(channel, pubkey, amount).await
    }

    /// `Deposit` is an increment, so this reads the own deposit and
    /// deposits the difference, under the same lock `fund` takes. That
    /// makes it safe against another call in this process. A duplicate from
    /// outside the process could still read the same starting deposit; the
    /// confirm loop leaves one only when a previous attempt's outcome could
    /// not be read for two minutes (`submit`), and that error names the
    /// signature to look up first.
    async fn fund_to(
        &self,
        channel: &ChannelId,
        own_total: u128,
    ) -> Result<ChannelState, SettlementError> {
        let _guard = self.deposit_lock.lock().await;
        let (pubkey, account) = self.open_channel(channel).await?;
        let state = self.to_channel_state(channel, &account)?;
        if own_total <= state.own_deposited {
            return Ok(state);
        }
        self.deposit_own(channel, pubkey, own_total - state.own_deposited)
            .await
    }

    async fn redeem(
        &self,
        channel: &ChannelId,
        claim: Claim,
    ) -> Result<ChannelState, SettlementError> {
        let (pubkey, account) = self.redeemable_channel(channel).await?;
        let state = self.to_channel_state(channel, &account)?;
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

        let counterparty_pubkey = self.counterparty_of(&account)?;
        let transferred_units = to_units(claim.cumulative_amount)?;
        let signature: [u8; 64] = claim.signature.as_slice().try_into().map_err(|_| {
            SettlementError::InvalidClaimSignature(format!(
                "a packages/solana-program claim signature must be exactly 64 bytes (a raw \
                 ed25519 signature), got {}",
                claim.signature.len()
            ))
        })?;
        let message =
            wire::balance_proof_message(&self.program_id, &pubkey, claim.nonce, transferred_units);
        let ed25519_instruction =
            wire::ed25519_verify_instruction(&counterparty_pubkey, &signature, &message);
        let claim_instruction = Instruction::new_with_bytes(
            self.program_id,
            &wire::pack_claim_from_channel(claim.nonce, transferred_units),
            wire::Accounts::claim_from_channel(&self.payer.pubkey(), &counterparty_pubkey, &pubkey),
        );
        // The Ed25519 precompile instruction must be at index 0
        // (`processor.rs:791-798`, `verify_ed25519_precompile`'s
        // `load_instruction_at_checked(0, ..)`), ahead of the program
        // instruction whose accounts and data it authorizes.
        self.submit(&[ed25519_instruction, claim_instruction], &[])
            .await?;

        let (_pubkey, account) = self.redeemable_channel(channel).await?;
        self.to_channel_state(channel, &account)
    }

    async fn close(&self, channel: &ChannelId) -> Result<ChannelState, SettlementError> {
        let (pubkey, _account) = self.open_channel(channel).await?;
        let instruction = Instruction::new_with_bytes(
            self.program_id,
            &wire::pack_close_channel(),
            wire::Accounts::close_channel(&self.payer.pubkey(), &pubkey),
        );
        self.submit(&[instruction], &[]).await?;

        let (_pubkey, account) = self.redeemable_channel(channel).await?;
        self.to_channel_state(channel, &account)
    }

    async fn settle(&self, channel: &ChannelId) -> Result<ChannelState, SettlementError> {
        let (pubkey, resolved) = self.resolve(channel).await?;
        let account = match resolved {
            Resolved::Settled => return Err(SettlementError::ChannelSettled(channel.clone())),
            Resolved::Live(account) => account,
        };
        match account.status {
            wire::ChannelStatus::Settled => {
                return Err(SettlementError::ChannelSettled(channel.clone()))
            }
            wire::ChannelStatus::Opened => {
                return Err(SettlementError::SettlementNotYetDue(channel.clone()))
            }
            wire::ChannelStatus::Closed => {}
        }
        // Mirrors `EvmSettlementBackend::settle`: the deployed program
        // itself checks `Clock::get()?.unix_timestamp`
        // (`processor.rs:524-533`), a real-clock value this process's own
        // wall clock agrees with far more closely than it would with, say,
        // an `anvil`-style artificially warped chain clock -- Solana has
        // no equivalent time-travel RPC method this backend's own tests
        // need to account for (see the `test_support` harness: a zero-length
        // challenge period is simply already due).
        let available_at = account
            .close_timestamp
            .saturating_add(account.challenge_duration as i64);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock is after the Unix epoch")
            .as_secs() as i64;
        if now < available_at {
            return Err(SettlementError::SettlementNotYetDue(channel.clone()));
        }

        let (vault, _bump) = wire::vault_pda(&pubkey, &self.program_id);
        let participant_a_token = spl_associated_token_account::get_associated_token_address(
            &account.participant_a,
            &self.token_mint,
        );
        let participant_b_token = spl_associated_token_account::get_associated_token_address(
            &account.participant_b,
            &self.token_mint,
        );
        let own = self.payer.pubkey();
        let instruction = Instruction::new_with_bytes(
            self.program_id,
            &wire::pack_settle_channel(),
            wire::Accounts::settle_channel(
                &own,
                &pubkey,
                &vault,
                &participant_a_token,
                &participant_b_token,
                &own,
            ),
        );
        self.submit(&[instruction], &[]).await?;

        self.settled
            .lock()
            .expect("settled mutex poisoned")
            .insert(pubkey);
        let counterparty = self.counterparty_of(&account)?;
        Ok(ChannelState {
            id: channel.clone(),
            counterparty: counterparty.to_bytes().to_vec(),
            status: ChannelStatus::Settled,
            counterparty_deposited: 0,
            own_deposited: 0,
            redeemed: 0,
        })
    }

    async fn channel_state(&self, channel: &ChannelId) -> Result<ChannelState, SettlementError> {
        let (_pubkey, resolved) = self.resolve(channel).await?;
        match resolved {
            Resolved::Live(account) => self.to_channel_state(channel, &account),
            Resolved::Settled => Ok(ChannelState {
                id: channel.clone(),
                counterparty: Vec::new(),
                status: ChannelStatus::Settled,
                counterparty_deposited: 0,
                own_deposited: 0,
                redeemed: 0,
            }),
        }
    }

    /// The port's ADR 0059 question. On Solana it has always been
    /// answerable: the channel account is a PDA seeded on the sorted pair
    /// and the mint (`processor.rs`, `wire::channel_pda`), so deriving
    /// the address and reading the account *is* the answer. One RPC.
    ///
    /// `None` when nothing is at that address -- which includes a pair
    /// whose channel has settled, since settlement closes the account and
    /// frees the PDA for their next one. `Err` only when the account
    /// could not be read at all, so "no channel" is never confused with
    /// "I could not find out".
    ///
    /// `counterparty` is a 32-byte ed25519 public key, the same identity
    /// [`open`](SettlementBackend::open) takes -- never an EVM address and
    /// never a node's secp256k1 edge identity, neither of which the
    /// deployed program could hold as a participant.
    async fn live_channel_with(
        &self,
        counterparty: Vec<u8>,
    ) -> Result<Option<ChannelId>, SettlementError> {
        let counterparty = Pubkey::try_from(counterparty.as_slice()).map_err(|_| {
            SettlementError::Backend(format!(
                "a packages/solana-program counterparty must be a 32-byte Solana pubkey, got {} bytes",
                counterparty.len()
            ))
        })?;
        let (channel, _bump) = wire::channel_pda(
            &self.payer.pubkey(),
            &counterparty,
            &self.token_mint,
            &self.program_id,
        );
        let Some(account) = self.fetch_account(&channel).await? else {
            return Ok(None);
        };
        if account.status == wire::ChannelStatus::Settled {
            return Ok(None);
        }
        Ok(Some(ChannelId(channel.to_string())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fabricated, well-formed channel account for exercising every
    /// refusal branch of
    /// [`SolanaSettlementBackend::resolvable_counterparty`] without a
    /// validator (issue #631's mint-binding review).
    fn account(
        participant_a: Pubkey,
        participant_b: Pubkey,
        token_mint: Pubkey,
        status: wire::ChannelStatus,
    ) -> wire::ChannelAccount {
        wire::ChannelAccount {
            participant_a,
            participant_b,
            token_mint,
            // Deliberately different per side: the resolution reports the
            // *counterparty's* deposit (issue #646), and equal numbers
            // would let a test pass while reading the wrong one.
            deposit_a: 500,
            deposit_b: 1_000,
            transferred_amount_a: 0,
            transferred_amount_b: 0,
            nonce_a: 0,
            nonce_b: 0,
            challenge_duration: 3_600,
            status,
            close_timestamp: 0,
        }
    }

    #[test]
    fn an_open_channel_resolves_to_the_other_participant_from_either_side() {
        let own = Pubkey::new_unique();
        let counterparty = Pubkey::new_unique();
        let mint = Pubkey::new_unique();
        assert_eq!(
            SolanaSettlementBackend::resolvable_counterparty(
                &account(own, counterparty, mint, wire::ChannelStatus::Opened),
                own,
                mint,
            ),
            Some((counterparty, 1_000)),
            "own as participant A resolves to participant B, with B's own deposit"
        );
        assert_eq!(
            SolanaSettlementBackend::resolvable_counterparty(
                &account(counterparty, own, mint, wire::ChannelStatus::Opened),
                own,
                mint,
            ),
            Some((counterparty, 500)),
            "own as participant B resolves to participant A, with A's own deposit"
        );
    }

    /// A merely `Closed` channel still redeems during its challenge window
    /// (issue #574), so it still resolves.
    #[test]
    fn a_closed_channel_still_resolves() {
        let own = Pubkey::new_unique();
        let counterparty = Pubkey::new_unique();
        let mint = Pubkey::new_unique();
        assert_eq!(
            SolanaSettlementBackend::resolvable_counterparty(
                &account(own, counterparty, mint, wire::ChannelStatus::Closed),
                own,
                mint,
            ),
            Some((counterparty, 1_000)),
        );
    }

    /// A settled channel can never be redeemed against, so honouring a
    /// claim on one would be giving the app's work away.
    #[test]
    fn a_settled_channel_resolves_to_nothing() {
        let own = Pubkey::new_unique();
        let counterparty = Pubkey::new_unique();
        let mint = Pubkey::new_unique();
        assert_eq!(
            SolanaSettlementBackend::resolvable_counterparty(
                &account(own, counterparty, mint, wire::ChannelStatus::Settled),
                own,
                mint,
            ),
            None,
        );
    }

    /// The mint binding (issue #631's security review): the deployed
    /// program lets any payer open a channel with ANY mint, and the
    /// balance-proof signature does not cover the mint, so a channel whose
    /// `token_mint` is not the one this backend settles in must be an
    /// unknown channel -- otherwise a claim funded with a worthless SPL
    /// token would buy USDC-priced writes.
    #[test]
    fn a_channel_on_a_different_mint_resolves_to_nothing() {
        let own = Pubkey::new_unique();
        let counterparty = Pubkey::new_unique();
        let configured_mint = Pubkey::new_unique();
        let junk_mint = Pubkey::new_unique();
        assert_eq!(
            SolanaSettlementBackend::resolvable_counterparty(
                &account(own, counterparty, junk_mint, wire::ChannelStatus::Opened),
                own,
                configured_mint,
            ),
            None,
        );
    }

    /// Somebody else's channel -- neither participant is this backend --
    /// is not a channel this backend can be paid on.
    #[test]
    fn a_channel_this_backend_is_no_participant_of_resolves_to_nothing() {
        let own = Pubkey::new_unique();
        let mint = Pubkey::new_unique();
        assert_eq!(
            SolanaSettlementBackend::resolvable_counterparty(
                &account(
                    Pubkey::new_unique(),
                    Pubkey::new_unique(),
                    mint,
                    wire::ChannelStatus::Opened,
                ),
                own,
                mint,
            ),
            None,
        );
    }
}

#[cfg(test)]
mod cluster_identity_tests {
    use super::*;

    /// Issue #1131. [`cluster_for_genesis_hash`] reads its hashes from
    /// `solana-sdk`'s own [`ClusterType::get_genesis_hash`] table rather
    /// than writing them down, so this test writes them down *once*, here,
    /// and pins the table to them.
    ///
    /// The three literals are what the public endpoints themselves answer
    /// to `{"jsonrpc":"2.0","id":1,"method":"getGenesisHash"}` -- verified
    /// against `api.mainnet-beta.solana.com`, `api.devnet.solana.com` and
    /// `api.testnet.solana.com` on 2026-08-24, agreeing exactly with the
    /// pinned `solana-sdk =2.1.0`. Without this, a future SDK bump that
    /// moved a hash would silently relabel a chain -- the very failure
    /// issue #975 exists to stop -- instead of failing the gate.
    #[test]
    fn cluster_names_match_the_published_genesis_hashes() {
        for (genesis_hash, expected) in [
            (
                "5eykt4UsFv8P8NJdTREpY1vzqKqZKvdpKuc147dw2N9d",
                "mainnet-beta",
            ),
            ("EtWTRABZaYq6iMfeYKouRu166VU2xqa1wcaWoxPkrZBG", "devnet"),
            ("4uhcVJyU9pJkvQyS88uRDiswHXSCkY3zQawwpjk2NsNY", "testnet"),
        ] {
            let hash = Hash::from_str(genesis_hash).expect("a published genesis hash is base58");
            assert_eq!(
                cluster_for_genesis_hash(&hash),
                Some(expected),
                "{genesis_hash} is {expected}'s published genesis hash"
            );
        }
    }

    /// The `solana-test-validator` case, stated without needing one: a
    /// genesis hash no public cluster published names no cluster, rather
    /// than being forced into the nearest one. Every `local/` topology and
    /// every tier-3 test in this workspace lands here, so this is the
    /// branch that keeps `make local-verify` booting.
    #[test]
    fn a_genesis_hash_no_public_cluster_published_names_no_cluster() {
        assert_eq!(cluster_for_genesis_hash(&Hash::new_unique()), None);
        assert_eq!(cluster_for_genesis_hash(&Hash::default()), None);
    }
}
