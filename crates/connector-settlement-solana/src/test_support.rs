//! A real, disposable `solana-test-validator` harness shared by this
//! crate's own integration tests (via a dev-dependency on itself with
//! `test-util` on) and other crates' (issue #630): `connector-cli`'s
//! settlement-construction tests need exactly what this crate's
//! integration tests already stand up, and one copy of the harness is one
//! place to fix it.
//! Gated behind the `test-util` feature for the same reason
//! `connector-settlement-evm`'s own `test_support` module is: a downstream
//! crate's tests cannot see anything behind `#[cfg(test)]`, since that cfg
//! is only active while this crate compiles its own test binary.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::Duration;

use solana_rpc_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::instruction::Instruction;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{Keypair, Signature, Signer};
use solana_sdk::transaction::Transaction;

/// Airdrop `pubkey` enough lamports to submit a handful of transactions
/// (issue #630) -- the shared "fund a freshly connected identity" step
/// every caller of this harness that goes on to sign real transactions
/// needs (`connector-cli`'s settlement-construction tests,
/// `connect_identity.rs`'s own), rather than each reimplementing the same
/// request-airdrop-then-poll-confirm loop.
pub async fn fund(rpc: &RpcClient, pubkey: &Pubkey) {
    let signature = rpc
        .request_airdrop(pubkey, 10_000_000_000)
        .await
        .expect("airdrop");
    for _ in 0..200 {
        if rpc.confirm_transaction(&signature).await.unwrap_or(false) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("airdrop did not confirm in time");
}

/// Why [`send`] failed: the RPC client's own error, boxed, since it is large.
pub type ClientError = Box<solana_rpc_client_api::client_error::Error>;

/// Sign `instructions` with `fee_payer` and `signers`, send them, and wait
/// for confirmation -- the client's side of a transaction, for tests that
/// stand in for a payer (issue #1343). `fee_payer` signs whether or not it
/// is among `signers`.
pub async fn send(
    rpc: &RpcClient,
    instructions: &[Instruction],
    fee_payer: &Keypair,
    signers: &[&Keypair],
) -> Result<Signature, ClientError> {
    let blockhash = rpc.get_latest_blockhash().await.map_err(Box::new)?;
    let mut all: Vec<&Keypair> = vec![fee_payer];
    all.extend(
        signers
            .iter()
            .filter(|signer| signer.pubkey() != fee_payer.pubkey()),
    );
    let transaction = Transaction::new_signed_with_payer(
        instructions,
        Some(&fee_payer.pubkey()),
        &all,
        blockhash,
    );
    rpc.send_and_confirm_transaction(&transaction)
        .await
        .map_err(Box::new)
}

/// Create a fresh SPL Token mint with `decimals`, whose mint authority is
/// `authority` (which also pays for it), and return its address.
pub async fn create_mint(rpc: &RpcClient, authority: &Keypair, decimals: u8) -> Pubkey {
    use solana_sdk::program_pack::Pack;
    let mint = Keypair::new();
    let rent = rpc
        .get_minimum_balance_for_rent_exemption(spl_token::state::Mint::LEN)
        .await
        .expect("rent for a mint");
    let instructions = [
        solana_sdk::system_instruction::create_account(
            &authority.pubkey(),
            &mint.pubkey(),
            rent,
            spl_token::state::Mint::LEN as u64,
            &spl_token::id(),
        ),
        spl_token::instruction::initialize_mint2(
            &spl_token::id(),
            &mint.pubkey(),
            &authority.pubkey(),
            None,
            decimals,
        )
        .expect("initialize_mint2"),
    ];
    send(rpc, &instructions, authority, &[&mint])
        .await
        .expect("create a mint");
    mint.pubkey()
}

/// Mint `amount` of `mint` into `owner`'s associated token account,
/// creating it first if need be. `authority` is the mint authority and pays.
pub async fn mint_to(
    rpc: &RpcClient,
    authority: &Keypair,
    mint: &Pubkey,
    owner: &Pubkey,
    amount: u64,
) {
    let instructions = [
        spl_associated_token_account::instruction::create_associated_token_account_idempotent(
            &authority.pubkey(),
            owner,
            mint,
            &spl_token::id(),
        ),
        spl_token::instruction::mint_to(
            &spl_token::id(),
            mint,
            &spl_associated_token_account::get_associated_token_address(owner, mint),
            &authority.pubkey(),
            &[],
            amount,
        )
        .expect("mint_to"),
    ];
    send(rpc, &instructions, authority, &[])
        .await
        .expect("mint tokens");
}

/// An x402 `batch-settlement` payer on `payment-channels` (ADR 0074, issue
/// #1343): the client whose transactions a batch-settlement test stands in
/// for. It holds its own funding key and a separate session key that signs
/// its vouchers -- ADR 0074 decision 6 makes that the ordinary case -- and
/// builds every instruction with [`crate::batch::wire`], the same builders
/// the sponsor endpoint (issue #1346) will co-sign a client's `open` with.
pub struct BatchPayer {
    rpc: RpcClient,
    program_id: Pubkey,
    /// Funds deposits and signs `open`, `top_up` and `request_close`. Holds
    /// SOL only so it can pay its own `top_up` and `request_close` fees; its
    /// `open` is paid for by the sponsor.
    pub payer: Keypair,
    /// The channel's `authorized_signer`: signs vouchers, nothing else.
    pub session: Keypair,
    next_salt: std::sync::atomic::AtomicU64,
}

impl BatchPayer {
    /// A fresh payer on the chain at `rpc_url`, funded with SOL for its own
    /// fees, holding no tokens yet ([`mint_to`] gives it some).
    pub async fn new(rpc_url: &str, program_id: Pubkey) -> BatchPayer {
        let rpc = RpcClient::new_with_commitment(
            rpc_url.to_string(),
            solana_sdk::commitment_config::CommitmentConfig::confirmed(),
        );
        let payer = Keypair::new();
        fund(&rpc, &payer.pubkey()).await;
        BatchPayer {
            rpc,
            program_id,
            payer,
            session: Keypair::new(),
            next_salt: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// An `open` this node's sponsor would admit: `payee`, `rent_payer` and
    /// the sole 10000 bps recipient all `sponsor`, the voucher signer the
    /// session key, a salt no earlier `open` from this payer used, and the
    /// current slot as `open_slot`. A test breaks one rule by overriding one
    /// field.
    pub async fn admissible_open(
        &self,
        sponsor: &Pubkey,
        mint: &Pubkey,
        deposit: u64,
        grace_period: u32,
    ) -> crate::batch::wire::OpenChannel {
        let open_slot = self.rpc.get_slot().await.expect("current slot");
        crate::batch::wire::OpenChannel {
            payer: self.payer.pubkey(),
            rent_payer: *sponsor,
            payee: *sponsor,
            mint: *mint,
            token_program: spl_token::id(),
            authorized_signer: self.session.pubkey(),
            salt: self
                .next_salt
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst),
            deposit,
            grace_period,
            open_slot,
            recipients: crate::batch::wire::sole_recipient(sponsor).to_vec(),
        }
    }

    /// Submit `open`, co-signed by `rent_payer` -- which also pays the
    /// transaction fee, as an x402 sponsor does -- and return the channel.
    ///
    /// **The channel is prefunded with its rent first, in the same
    /// transaction.** The program is built on pinocchio 0.11, whose
    /// `Rent::try_minimum_balance` is `(128 + len) × lamports_per_byte` and
    /// ignores `exemption_threshold` -- correct on a cluster where SIMD-0194
    /// has set the threshold to 1, as mainnet-beta and a v3 test validator
    /// have, and exactly half the real figure on the v2.1.21
    /// `solana-test-validator` the Rust Workspace Gate pins, whose genesis
    /// still carries a threshold of 2. There `open` alone leaves the channel
    /// short of rent and the runtime refuses the transaction. The program
    /// tops up only the shortfall (PC `instructions/open.rs`: "prefund-tolerant
    /// PDA creation"), so a channel already holding the true minimum costs
    /// nothing more on either validator, and the rent still comes from
    /// `rent_payer`. A client on a real cluster does not need this.
    pub async fn open(
        &self,
        open: &crate::batch::wire::OpenChannel,
        rent_payer: &Keypair,
    ) -> Result<Pubkey, ClientError> {
        let channel = open.channel(&self.program_id);
        let rent = self
            .rpc
            .get_minimum_balance_for_rent_exemption(crate::batch::wire::CHANNEL_ACCOUNT_LEN)
            .await
            .map_err(Box::new)?;
        send(
            &self.rpc,
            &[
                solana_sdk::system_instruction::transfer(&rent_payer.pubkey(), &channel, rent),
                open.instruction(&self.program_id),
            ],
            rent_payer,
            &[&self.payer],
        )
        .await?;
        Ok(channel)
    }

    /// The payer's `top_up` of `amount` in `mint`.
    pub async fn top_up(
        &self,
        channel: &Pubkey,
        mint: &Pubkey,
        amount: u64,
    ) -> Result<Signature, ClientError> {
        let instruction = crate::batch::wire::top_up_instruction(
            &self.program_id,
            &self.payer.pubkey(),
            channel,
            mint,
            &spl_token::id(),
            amount,
        );
        send(&self.rpc, &[instruction], &self.payer, &[]).await
    }

    /// The payer's `request_close`, starting the grace period.
    pub async fn request_close(&self, channel: &Pubkey) -> Result<Signature, ClientError> {
        let instruction = crate::batch::wire::request_close_instruction(
            &self.program_id,
            &self.payer.pubkey(),
            channel,
        );
        send(&self.rpc, &[instruction], &self.payer, &[]).await
    }

    /// The session key's voucher on `channel` for `cumulative_amount`, with
    /// `expires_at` zero.
    pub fn sign(&self, channel: &Pubkey, cumulative_amount: u64) -> [u8; 64] {
        let message =
            connector_signer::solana_voucher_message(&channel.to_bytes(), cumulative_amount, 0);
        self.session
            .sign_message(&message)
            .as_ref()
            .try_into()
            .expect("an Ed25519 signature is 64 bytes")
    }
}

/// solana-foundation's `payment-channels` binary, as deployed on
/// mainnet-beta, which [`SolanaValidator::spawn`] loads into every
/// validator's genesis at its canonical id,
/// [`PAYMENT_CHANNELS_PROGRAM_ID`](crate::batch::wire::PAYMENT_CHANNELS_PROGRAM_ID)
/// (ADR 0074, issue #1343). The program is compiled against that id, so it
/// cannot be loaded anywhere else.
///
/// **Provenance.** Dumped on 2026-09-25 with
/// `solana program dump -u m CHNLxYvVA28MJP9PrFuDXccuoGXAx7jBacfLEkahyGsX`
/// from programdata `CghQXkmw2F6p1exMETiZdNeUx9QGraWsNZ4eom1Cuiw1`, last
/// deployed at slot 431447053 under upgrade authority
/// `DXtFpbPjcn2hxPnw79x1Pfoj35vXh5AsWBkS37YnXMVv` -- the deployment ADR 0074's
/// _Sources_ records. 66,240 bytes, SHA-256
/// [`PAYMENT_CHANNELS_FIXTURE_SHA256`]. It is the chain's binary rather than
/// a build of the pinned source (`3ffa4d67`) because the chain's binary is
/// what a node settles against, and the record could not establish that the
/// two are equal. The devnet deployment's dump differs (SHA-256
/// `acdb3abf…cf8b`): its `TREASURY_OWNER` is the placeholder a devnet build
/// ships with.
///
/// Committed rather than dumped at test time so the gate needs no network;
/// no key material is involved, only the program's public bytes.
pub fn payment_channels_fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/payment_channels.so")
}

/// The SHA-256 of [`payment_channels_fixture`], as dumped. A test pins the
/// committed file to it, so a replaced binary is a deliberate, reviewed
/// change to this constant rather than a silent one.
pub const PAYMENT_CHANNELS_FIXTURE_SHA256: &str =
    "e85f751cc886752d63d054c365bdd747d996f22dd48c30f3f1060532b2b25e17";

/// The Token program as mainnet-beta runs it -- p-token, the pinocchio
/// rewrite of SPL Token -- which [`SolanaValidator::spawn`] loads into every
/// validator's genesis at the SPL Token id, in place of the SPL Token the
/// validator bundles (issue #1358).
///
/// **Why.** `payment-channels`' `distribute` sends two or more payouts -- this
/// node's share and the payer's refund, the ordinary case -- as one SPL Token
/// `Batch` CPI. Only p-token implements `Batch`; the SPL Token bundled with
/// `solana-test-validator` (v2.1.21 and v3.1.12) refuses it as
/// `InvalidInstruction` (token error 12), so without this no local chain
/// could run the `distribute` a node settles with on mainnet-beta.
///
/// **Provenance.** Dumped on 2026-09-26 with
/// `solana program dump -u m TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA`
/// from programdata `3gvYRKWyXRR9xKWe1ZjPhLY5ZJRN7KDB4rFZFGoJfFk2`, last
/// deployed at slot 419472000 and holding no upgrade authority. 108,600
/// bytes, SHA-256 [`TOKEN_PROGRAM_FIXTURE_SHA256`]; the source paths in its
/// strings are pinocchio's. The same dump from devnet has the same hash.
///
/// Committed rather than dumped at test time so the gate needs no network;
/// no key material is involved, only the program's public bytes.
pub fn token_program_fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/p_token.so")
}

/// The SHA-256 of [`token_program_fixture`], as dumped, pinned by a test
/// exactly as [`PAYMENT_CHANNELS_FIXTURE_SHA256`] is.
pub const TOKEN_PROGRAM_FIXTURE_SHA256: &str =
    "8190d3f7ceb6cb7a7a8d8924bff89f9f611e15ce1f806f2b6237f3311a98f697";

/// The executable bytes the chain at `rpc` serves for the upgradeable
/// program `program_id`: its programdata account past the loader's header.
/// `None` if there is no such program there.
async fn served_program(rpc: &RpcClient, program_id: &Pubkey) -> Option<Vec<u8>> {
    use solana_sdk::bpf_loader_upgradeable::{self, UpgradeableLoaderState};
    let program = rpc.get_account(program_id).await.ok()?;
    if program.owner != bpf_loader_upgradeable::id() {
        return None;
    }
    let UpgradeableLoaderState::Program {
        programdata_address,
    } = bincode::deserialize(&program.data).ok()?
    else {
        return None;
    };
    let programdata = rpc.get_account(&programdata_address).await.ok()?;
    programdata
        .data
        .get(UpgradeableLoaderState::size_of_programdata_metadata()..)
        .map(<[u8]>::to_vec)
}

/// True if `solana-test-validator --version` runs successfully.
pub fn solana_test_validator_available() -> bool {
    Command::new("solana-test-validator")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

/// The Solana twin of `connector_settlement_evm::test_support::require_anvil`:
/// a real chain is genuinely under test here (ADR 0007), so a CI run
/// lacking `solana-test-validator` must fail loudly rather than silently
/// skip and report `passed`. A local run missing it still skips, since
/// requiring every contributor to install the Solana CLI just to run
/// `cargo test` is a real cost this crate doesn't need to impose.
///
/// Returns `true` when the caller should proceed with its real assertions,
/// `false` when the caller should return early (having already skipped
/// gracefully via a printed message).
pub fn require_solana_test_validator() -> bool {
    if solana_test_validator_available() {
        return true;
    }

    if std::env::var_os("CI").is_some() {
        panic!(
            "solana-test-validator is not on PATH, but CI is set -- the Rust Workspace Gate must \
             provide it before this crate's tests run. Refusing to silently skip and report \
             success here; see issue #567."
        );
    }

    eprintln!(
        "skipping: solana-test-validator is not on PATH (install the Solana CLI: \
         https://docs.anza.xyz/cli/install) -- this test needs a real chain and only skips \
         because this is not a CI run"
    );
    false
}

static NEXT_PORT_OFFSET: AtomicU16 = AtomicU16::new(0);

/// A freshly spawned `solana-test-validator` instance, with the committed
/// `payment-channels` binary ([`payment_channels_fixture`]) loaded into its
/// genesis at its canonical id and the committed
/// p-token ([`token_program_fixture`]) at the SPL Token id -- read back
/// before `spawn` returns -- killed (and its disposable ledger directory
/// removed) when dropped. Each instance gets its own ledger directory and
/// ports so tests spawning one concurrently don't collide.
pub struct SolanaValidator {
    child: Child,
    // Never read after construction -- kept only so its directory outlives
    // the validator process using it and is removed on drop.
    _ledger: tempfile::TempDir,
    pub rpc_url: String,
}

impl SolanaValidator {
    pub async fn spawn() -> Self {
        Self::spawn_with_args(&[]).await
    }

    /// [`spawn`](Self::spawn), with `extra` appended to
    /// `solana-test-validator`'s arguments: `["--warp-slot", "10000"]`, say,
    /// for a test that needs a slot a fresh ledger has not reached.
    pub async fn spawn_with_args(extra: &[&str]) -> Self {
        let offset = NEXT_PORT_OFFSET.fetch_add(1, Ordering::SeqCst);
        let rpc_port = 19_900u16
            .wrapping_add((std::process::id() as u16) % 500)
            .wrapping_add(offset.wrapping_mul(50));
        let rpc_url = format!("http://127.0.0.1:{rpc_port}");
        let ledger = tempfile::tempdir().expect("create disposable ledger dir");

        let child = Command::new("solana-test-validator")
            .args(["--ledger"])
            .arg(ledger.path())
            .args(["--rpc-port", &rpc_port.to_string()])
            .args(["--faucet-port", &(rpc_port + 1).to_string()])
            .args([
                "--dynamic-port-range",
                &format!("{}-{}", rpc_port + 2, rpc_port + 40),
            ])
            .args([
                "--bpf-program",
                crate::batch::wire::PAYMENT_CHANNELS_PROGRAM_ID,
            ])
            .arg(payment_channels_fixture())
            .args(["--bpf-program", &spl_token::id().to_string()])
            .arg(token_program_fixture())
            .args(["--reset", "--quiet"])
            .args(extra)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect(
                "spawn solana-test-validator (is it on PATH? see \
                 https://docs.anza.xyz/cli/install)",
            );

        let rpc = RpcClient::new(rpc_url.clone());
        let mut ready = false;
        for _ in 0..600 {
            if rpc.get_version().await.is_ok() {
                ready = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(
            ready,
            "solana-test-validator did not become ready at {rpc_url}"
        );
        let served = served_program(&rpc, &spl_token::id()).await;
        assert!(
            served.as_deref() == std::fs::read(token_program_fixture()).ok().as_deref(),
            "solana-test-validator at {rpc_url} does not serve the committed p-token fixture at \
             the SPL Token id; a two-payout `distribute` would meet the bundled SPL Token, which \
             refuses `Batch`"
        );

        Self {
            child,
            _ledger: ledger,
            rpc_url,
        }
    }
}

impl Drop for SolanaValidator {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
