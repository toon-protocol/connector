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

use std::path::{Path, PathBuf};
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

/// The fixed program id this crate's tests load `payment_channel.so`
/// under -- passed to `solana-test-validator --bpf-program` as a bare id
/// (see [`SolanaValidator::spawn`]), not resolved from any keypair file, so
/// no keypair for it is tracked in this repo (issue #922). Distinct from the
/// real, deployed `2aEVJ8koKD8LTZrLRSGtAtU7LBt4e7QjjCgf1kzQ7Rip` on public
/// devnet (`packages/solana-program/deployments/devnet-public.md`): this id
/// exists only inside a disposable local validator's genesis.
pub const LOCAL_TEST_PROGRAM_ID: &str = "HY4AYFNe5Vg5BkEwAURNsGY3uFAvGMNpAQPRtgoasJiR";

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

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("crates/connector-settlement-solana is two levels below the workspace root")
        .to_path_buf()
}

fn program_so_path() -> PathBuf {
    workspace_root().join("target/deploy/payment_channel.so")
}

/// Where [`program_build_record`] of the `payment_channel.so` this harness
/// last built is kept, next to the `.so` itself so the two travel together
/// through any `target/` cache.
fn program_record_path() -> PathBuf {
    workspace_root().join("target/deploy/payment_channel.so.buildrecord")
}

/// What this harness vouches for when it skips a build: the sources the
/// `.so` was built from, AND the bytes it wrote.
///
/// The second half is not redundant. `target/deploy` is a drop box no one
/// owns -- `tools/solana/build-sbf.sh`, `cargo test-sbf` via `make
/// solana-test` and a hand-run `cargo build-sbf` all write
/// `payment_channel.so` there, and the last of those can write a binary
/// from a different platform-tools line (measurably different bytes:
/// 112,680 against the pinned line's 109,416). Recording only the sources
/// meant any of them could replace the artifact under a record that still
/// matched, and this harness would load it and say nothing. Recording what
/// was written makes a foreign write a rebuild instead.
fn program_build_record(source_fingerprint: &str, so_bytes: &[u8]) -> String {
    format!("{source_fingerprint} {}", solana_sdk::hash::hash(so_bytes))
}

/// True when the `.so` at `so_path` is exactly the artifact this harness
/// last built, from exactly `source_fingerprint`. False -- meaning
/// "rebuild" -- if any of the three is missing or disagrees.
fn program_artifact_is_current(
    so_path: &Path,
    record_path: &Path,
    source_fingerprint: Option<&str>,
) -> bool {
    let Some(source_fingerprint) = source_fingerprint else {
        return false;
    };
    let Ok(so_bytes) = std::fs::read(so_path) else {
        return false;
    };
    std::fs::read_to_string(record_path).ok()
        == Some(program_build_record(source_fingerprint, &so_bytes))
}

/// A hash over every source `cargo build-sbf` compiles into
/// `payment_channel.so` -- `packages/solana-program`'s manifest and its
/// whole `src/` tree, each file's path hashed alongside its bytes so a
/// rename counts as a change.
///
/// This exists because "the `.so` is present" is not "the `.so` is the
/// program in this working tree". `target/` is a restored cache in the
/// Rust Workspace Gate, so a `.so` built from an *older* commit arrives
/// already present; reusing it silently tested the wrong program. That is
/// exactly how #1082's balance-proof change (ADR 0053) failed CI: the
/// client signed the new 96-byte message while the cached program still
/// expected the old 48-byte one and rejected every claim with
/// `InvalidSignature`. Comparing sources, not mere existence, is what
/// makes the rebuild happen.
///
/// `None` if the sources cannot be read at all, which callers treat as
/// "cannot vouch for the `.so`" and rebuild.
fn program_source_fingerprint() -> Option<String> {
    let program_dir = workspace_root().join("packages/solana-program");
    let mut inputs = vec![program_dir.join("Cargo.toml")];
    collect_program_sources(&program_dir.join("src"), &mut inputs)?;
    inputs.sort();

    let mut hashed = Vec::new();
    for path in &inputs {
        hashed.extend_from_slice(path.to_string_lossy().as_bytes());
        hashed.extend_from_slice(&std::fs::read(path).ok()?);
    }
    Some(solana_sdk::hash::hash(&hashed).to_string())
}

/// Every file under `dir`, recursively, appended to `out`. `None` if the
/// directory cannot be walked.
fn collect_program_sources(dir: &Path, out: &mut Vec<PathBuf>) -> Option<()> {
    for entry in std::fs::read_dir(dir).ok()? {
        let path = entry.ok()?.path();
        if path.is_dir() {
            collect_program_sources(&path, out)?;
        } else {
            out.push(path);
        }
    }
    Some(())
}

/// Build `packages/solana-program` for the SBF target with `cargo
/// build-sbf` unless the `.so` already on disk was built from exactly the
/// sources in this working tree, so a fresh checkout, a CI cache miss, or
/// an *edit to the program* all still produce something current to load --
/// mirrors `make solana-build`. Returns `false` (rather than panicking) if
/// the build tool is missing or the build itself fails, so callers can
/// fold that into the same "skip locally, fail loudly in CI" policy every
/// other gate in this harness already uses.
///
/// The freshness check is [`program_build_record`], recorded beside the
/// `.so` on each successful build; see its docs, and
/// [`program_source_fingerprint`]'s, for why neither the `.so`'s presence
/// nor the sources alone was enough.
///
/// `--tools-version v1.52` pins the platform-tools release rather than
/// taking whichever line the installed CLI defaults to: it is the same pin
/// CI's own `solana-program` job builds with
/// (`.github/workflows/ci.yml`), and the toolchain line the deployed
/// devnet program itself was built from -- a v1.52 build of this source
/// matches the live bytecode's exact size and is 99.7% byte-identical,
/// where the v2.1 CLI's default tools line produces a differently-sized
/// binary entirely (`packages/solana-program/deployments/devnet-public.md`,
/// "Reproducible-build comparison").
///
/// The pin is applied by shelling through `tools/solana/build-sbf.sh` rather
/// than spawning `cargo build-sbf` here, so this harness gets the same
/// cold-machine bootstrap every other caller does: on a checkout that has
/// never built the program, a bare pinned `cargo build-sbf` panics on a
/// missing `$HOME/.cache/solana` long before it reaches the network. That
/// script's header explains why, and it is also what refuses a build that
/// silently fell back to the CLI's built-in toolchain line.
fn ensure_program_built() -> bool {
    let fingerprint = program_source_fingerprint();
    if program_artifact_is_current(
        &program_so_path(),
        &program_record_path(),
        fingerprint.as_deref(),
    ) {
        return true;
    }
    let status = Command::new(workspace_root().join("tools/solana/build-sbf.sh"))
        .current_dir(workspace_root().join("packages/solana-program"))
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .status();
    let built = matches!(status, Ok(status) if status.success()) && program_so_path().exists();
    if built {
        if let (Some(fingerprint), Ok(so_bytes)) = (fingerprint, std::fs::read(program_so_path())) {
            // Written via a temporary and renamed: two tests in the same
            // binary reach this concurrently (cargo's own build lock
            // serializes their builds, not their bookkeeping), and a
            // half-written record would cost a needless rebuild.
            let temporary = program_record_path().with_extension("buildrecord.tmp");
            let record = program_build_record(&fingerprint, &so_bytes);
            if std::fs::write(&temporary, &record).is_ok() {
                let _ = std::fs::rename(&temporary, program_record_path());
            }
        }
    }
    built
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
/// lacking either `solana-test-validator` or a buildable
/// `payment_channel.so` must fail loudly rather than silently skip and
/// report `passed`. A local run missing either still skips, since
/// requiring every contributor to install the Solana CLI and SBF toolchain
/// just to run `cargo test` is a real cost this crate doesn't need to
/// impose.
///
/// Returns `true` when the caller should proceed with its real assertions,
/// `false` when the caller should return early (having already skipped
/// gracefully via a printed message).
pub fn require_solana_test_validator() -> bool {
    let validator_ok = solana_test_validator_available();
    let program_ok = ensure_program_built();

    if validator_ok && program_ok {
        return true;
    }

    if std::env::var_os("CI").is_some() {
        panic!(
            "solana-test-validator on PATH: {validator_ok}, packages/solana-program built (or \
             buildable via `cargo build-sbf`): {program_ok} -- the Rust Workspace Gate must \
             provide both before this crate's tests run. Refusing to silently skip and report \
             success here; see issue #567."
        );
    }

    eprintln!(
        "skipping: solana-test-validator on PATH: {validator_ok}, packages/solana-program built \
         (or buildable via `cargo build-sbf`, requires the Solana SBF toolchain: \
         https://docs.anza.xyz/cli/install): {program_ok} -- this test needs a real chain \
         running the real deployed program and only skips because this is not a CI run"
    );
    false
}

static NEXT_PORT_OFFSET: AtomicU16 = AtomicU16::new(0);

/// A freshly spawned `solana-test-validator` instance, with
/// `packages/solana-program`'s own built `.so` loaded into its genesis at
/// [`LOCAL_TEST_PROGRAM_ID`] and the committed `payment-channels` binary
/// ([`payment_channels_fixture`]) at its canonical id, killed (and its disposable ledger directory
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
        let offset = NEXT_PORT_OFFSET.fetch_add(1, Ordering::SeqCst);
        let rpc_port = 19_900u16
            .wrapping_add((std::process::id() as u16) % 500)
            .wrapping_add(offset.wrapping_mul(50));
        let rpc_url = format!("http://127.0.0.1:{rpc_port}");
        let ledger = tempfile::tempdir().expect("create disposable ledger dir");

        let so_path = program_so_path();
        assert!(
            so_path.exists(),
            "packages/solana-program's built artifact missing: {} -- \
             require_solana_test_validator() must be checked first",
            so_path.display()
        );

        let child = Command::new("solana-test-validator")
            .args(["--ledger"])
            .arg(ledger.path())
            .args(["--rpc-port", &rpc_port.to_string()])
            .args(["--faucet-port", &(rpc_port + 1).to_string()])
            .args([
                "--dynamic-port-range",
                &format!("{}-{}", rpc_port + 2, rpc_port + 40),
            ])
            .args(["--bpf-program", LOCAL_TEST_PROGRAM_ID])
            .arg(&so_path)
            .args([
                "--bpf-program",
                crate::batch::wire::PAYMENT_CHANNELS_PROGRAM_ID,
            ])
            .arg(payment_channels_fixture())
            .args(["--reset", "--quiet"])
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

#[cfg(test)]
mod tests {
    use super::{program_artifact_is_current, program_build_record};

    /// A `target/deploy` directory holding a `.so` and the record this
    /// harness would have written for it. Real files in a temporary
    /// directory, not a fake filesystem: what is under test is the
    /// conclusion [`program_artifact_is_current`] draws from bytes on disk.
    struct DropBox {
        _dir: tempfile::TempDir,
        so: std::path::PathBuf,
        record: std::path::PathBuf,
    }

    impl DropBox {
        fn built_from(sources: &str, so_bytes: &[u8]) -> Self {
            let dir = tempfile::tempdir().expect("temp dir");
            let so = dir.path().join("payment_channel.so");
            let record = dir.path().join("payment_channel.so.buildrecord");
            std::fs::write(&so, so_bytes).expect("write .so");
            std::fs::write(&record, program_build_record(sources, so_bytes)).expect("write record");
            Self {
                _dir: dir,
                so,
                record,
            }
        }

        fn is_current_for(&self, sources: &str) -> bool {
            program_artifact_is_current(&self.so, &self.record, Some(sources))
        }
    }

    #[test]
    fn the_artifact_this_harness_built_is_current() {
        let drop_box = DropBox::built_from("sources-a", b"pinned bytes");
        assert!(
            drop_box.is_current_for("sources-a"),
            "the .so and the record this harness itself wrote must not cost a rebuild"
        );
    }

    #[test]
    fn an_artifact_someone_else_wrote_is_not_current() {
        let drop_box = DropBox::built_from("sources-a", b"pinned bytes");
        // What a hand-run `cargo build-sbf` does: replaces the .so in
        // target/deploy with a build of the same sources from a different
        // platform-tools line, leaving this harness's record untouched.
        std::fs::write(&drop_box.so, b"some other toolchain line's bytes").expect("foreign write");

        assert!(
            !drop_box.is_current_for("sources-a"),
            "target/deploy/payment_channel.so is a drop box this harness does not own -- a .so it \
             did not write must be rebuilt, not loaded into a validator on the strength of a \
             record describing different bytes"
        );
    }

    #[test]
    fn an_artifact_built_from_other_sources_is_not_current() {
        let drop_box = DropBox::built_from("sources-a", b"pinned bytes");
        assert!(
            !drop_box.is_current_for("sources-b"),
            "an edit to packages/solana-program must rebuild -- issue #1082's failure was loading \
             a cached .so that predated the change under test"
        );
    }

    #[test]
    fn an_unreadable_artifact_or_record_is_not_current() {
        let drop_box = DropBox::built_from("sources-a", b"pinned bytes");

        std::fs::remove_file(&drop_box.record).expect("remove record");
        assert!(
            !drop_box.is_current_for("sources-a"),
            "no record means this harness cannot vouch for the .so"
        );

        std::fs::write(
            &drop_box.record,
            program_build_record("sources-a", b"pinned bytes"),
        )
        .expect("restore record");
        std::fs::remove_file(&drop_box.so).expect("remove .so");
        assert!(
            !drop_box.is_current_for("sources-a"),
            "no .so means there is nothing to vouch for"
        );

        assert!(
            !program_artifact_is_current(&drop_box.so, &drop_box.record, None),
            "sources that cannot be read at all must rebuild rather than trust the record"
        );
    }
}
