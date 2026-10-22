//! `connector init --devnet`: a runnable devnet node in one directory, from
//! nothing, in one command.
//!
//! The first-run test of 2026-10-13 (toon-protocol/website, `src/usability/`)
//! took a careful operator twenty minutes and four traps to get from the
//! guide to a paid request: a key the image could not read, a config the
//! client refused to describe because it named no settlement chain, a route
//! that could not be sent to without a destination, and a node that said
//! nothing when it was paid. Every one of those is a fact this binary
//! already knows. This verb writes them down so the operator does not have to:
//!
//! * `config/connector.toml` -- the client edge, a `[node]` address that is
//!   also a priced route (so `toon-client send` needs no destination), the
//!   Base Sepolia settlement block from `docs/guide/get-paid.md`, and an
//!   `[operator]` section so `GET /claims` and the dashboard work from the
//!   first boot;
//! * `data/signer.key`, `data/settlement.key`, `data/operator-write.key` --
//!   fresh 32-byte secrets, 64 hex characters each, mode `0600`;
//! * `data/operator-bearer-token`, `data/operator-write-keys` -- the two
//!   halves the operator surface refuses to start without (ADR 0008);
//! * `state/` -- the claim journal's home, created now so Docker does not
//!   create it root-owned later;
//! * `compose.yml` -- the connector and, behind the one route, the
//!   payment-oblivious `stub-app` this image already ships, both running
//!   **as the user who ran `init`**, which is what makes `0600` key files
//!   and a bind-mounted `state/` readable and writable without a `chown`.
//!
//! What it does not do: touch a chain, dial anything, or overwrite a file.
//! A directory that already holds a `config/connector.toml` is refused by
//! name, because a second run that silently rotated the signer key would
//! change the node's identity under a running peering.
//!
//! Only `--devnet` exists. A mainnet node settles real money in a token the
//! operator chooses, and every value in that block is theirs to read off the
//! chain (`docs/guide/get-paid.md`); nothing here should guess it for them.

use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use connector_signer::{derive_evm_address, to_hex, LocalSigner, Signer};
use rand::RngCore;

/// The public Base Sepolia devnet, as `docs/guide/get-paid.md` publishes it.
/// Copied, not chosen: a voucher resolves against one token, so every node
/// that might accept a devnet voucher has to name the same one.
pub const DEVNET_EVM_RPC_URL: &str = "https://base-sepolia-rpc.publicnode.com";
pub const DEVNET_EVM_TOKEN: &str = "0x0C996d7c934c79a6255254875607Fe69df25C0E1";
pub const DEVNET_EVM_DECIMALS: u8 = 6;
pub const DEVNET_EVM_EIP712_NAME: &str = "USDC";
pub const DEVNET_EVM_EIP712_VERSION: &str = "2";
/// TOON's own stock x402 facilitator on Base Sepolia. Payers relay their
/// deposits through it, so a wallet holding devnet USDC and no ETH can open
/// a channel; this node publishes the URL and never calls it (ADR 0076).
pub const DEVNET_FACILITATOR_URL: &str = "https://onboard.devnet.toonprotocol.dev";
/// The image `compose.yml` runs when `--image` names none. A floating tag,
/// fine for the scratch trial this verb exists for and announced as such in
/// the report; a node kept running is pinned to a release handle.
pub const DEFAULT_IMAGE: &str = "ghcr.io/toon-protocol/connector:rust-main";
/// What the one route charges: 0.001 devnet USDC, the devnet store's price.
pub const DEVNET_PRICE: u64 = 1000;

/// The port the stub app listens on inside the compose network. The same
/// port spaceturtle expects of any app, so a config written here reads the
/// same as one written there.
const APP_PORT: u16 = 3100;

/// What `connector init` was asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitOptions {
    /// Where to write. Created if missing.
    pub dir: PathBuf,
    /// The node's ILP address, which is also its one route's prefix. `None`
    /// derives `g.devnet.<six hex digits of the signer's public key>`.
    pub address: Option<String>,
    /// The image `compose.yml` names, for both services.
    pub image: String,
}

/// Why `init` wrote nothing (or stopped part way: every file it did write
/// is named in the error's `Io` path, and none is a secret that matters
/// until a node has run with it).
#[derive(Debug)]
pub enum InitError {
    /// The directory already holds a node. Nothing was touched.
    AlreadyInitialised { config: PathBuf },
    /// A file or directory could not be created.
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// The freshly generated settlement secret was refused by the signer
    /// (all zeros, or out of the curve's range -- vanishingly unlikely, and
    /// reported rather than retried so a broken random source is loud).
    Key(String),
    /// `--address` is not something a route prefix can be.
    Address(String),
}

impl fmt::Display for InitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InitError::AlreadyInitialised { config } => write!(
                f,
                "{} already exists: this directory holds a node. `init` never overwrites one, \
                 because rotating its signer key would change its identity under every peer \
                 that has it. Choose another --dir, or delete the directory yourself.",
                config.display()
            ),
            InitError::Io { path, source } => {
                write!(f, "could not write {}: {source}", path.display())
            }
            InitError::Key(reason) => write!(f, "could not derive a settlement key: {reason}"),
            InitError::Address(reason) => write!(f, "--address {reason}"),
        }
    }
}

impl std::error::Error for InitError {}

/// What `init` produced, for the report and for tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitReport {
    pub dir: PathBuf,
    pub address: String,
    /// The EVM address of `data/settlement.key`: where vouchers settle, and
    /// what has to hold a little Base Sepolia ETH for the node to land them.
    pub settlement_address: String,
    pub image: String,
    pub files: Vec<PathBuf>,
}

/// Write a devnet node into `options.dir`.
pub fn init(options: &InitOptions) -> Result<InitReport, InitError> {
    let dir = &options.dir;
    let config_path = dir.join("config").join("connector.toml");
    if config_path.exists() {
        return Err(InitError::AlreadyInitialised {
            config: config_path,
        });
    }
    if let Some(address) = &options.address {
        check_address(address)?;
    }

    for sub in ["config", "data", "state"] {
        let path = dir.join(sub);
        fs::create_dir_all(&path).map_err(|source| InitError::Io {
            path: path.clone(),
            source,
        })?;
    }

    let signer_secret = random_32();
    let settlement_secret = random_32();
    let operator_secret = random_32();
    let bearer = hex(&random_32());

    let signer = LocalSigner::from_secret_bytes("connector-signer", signer_secret)
        .map_err(|error| InitError::Key(error.to_string()))?;
    let signer_public = signer
        .public_key()
        .map_err(|error| InitError::Key(error.to_string()))?;
    let settlement = LocalSigner::from_secret_bytes("settlement", settlement_secret)
        .map_err(|error| InitError::Key(error.to_string()))?;
    let settlement_public = settlement
        .public_key()
        .map_err(|error| InitError::Key(error.to_string()))?;
    let settlement_address = to_hex(&derive_evm_address(&settlement_public));

    let operator_public = {
        let secret = ed25519_dalek::SecretKey::from_bytes(&operator_secret)
            .map_err(|error| InitError::Key(error.to_string()))?;
        ed25519_dalek::PublicKey::from(&secret).to_bytes()
    };

    let address = options
        .address
        .clone()
        .unwrap_or_else(|| format!("g.devnet.{}", hex(&signer_public[1..4])));

    let (uid, gid) = owner_of(dir);

    let mut files = Vec::new();
    let mut write = |relative: &str, contents: &str, secret: bool| -> Result<(), InitError> {
        let path = dir.join(relative);
        write_file(&path, contents.as_bytes(), secret)?;
        files.push(path);
        Ok(())
    };

    write("data/signer.key", &hex(&signer_secret), true)?;
    write("data/settlement.key", &hex(&settlement_secret), true)?;
    write("data/operator-write.key", &hex(&operator_secret), true)?;
    write("data/operator-bearer-token", &format!("{bearer}\n"), true)?;
    write(
        "data/operator-write-keys",
        &format!(
            "# ed25519 public keys allowed to sign operator writes, one per line (ADR 0008).\n\
             # This one belongs to data/operator-write.key.\n{}\n",
            hex(&operator_public)
        ),
        false,
    )?;
    write(
        "config/connector.toml",
        &config_toml(&address, &settlement_address),
        false,
    )?;
    write("compose.yml", &compose_yml(&options.image, uid, gid), false)?;

    Ok(InitReport {
        dir: dir.clone(),
        address,
        settlement_address,
        image: options.image.clone(),
        files,
    })
}

/// The report an operator reads once, with the three things they will need
/// next: how to start it, how to see it, and what to fund.
pub fn describe(report: &InitReport) -> String {
    let dir = report.dir.display();
    format!(
        "Wrote a devnet node to {dir}:\n\
         \n\
         \x20 config/connector.toml   one route, {address}, at {price} units (0.001 devnet USDC)\n\
         \x20 data/                   signer, settlement and operator keys, 0600, owned by you\n\
         \x20 state/                  the claim journal; keep it, it is the watermark\n\
         \x20 compose.yml             the connector and the stub app, as your own uid\n\
         \n\
         Start it, then ask it what it is:\n\
         \n\
         \x20 docker compose up -d\n\
         \x20 curl -s http://127.0.0.1:3000/ilp | jq .routes\n\
         \n\
         Pay it from another terminal (devnet USDC from the faucet, no ETH needed):\n\
         \n\
         \x20 npx toon-client faucet\n\
         \x20 npx toon-client channel open --connector http://127.0.0.1:3000 --deposit 100000\n\
         \x20 npx toon-client send --connector http://127.0.0.1:3000 --body 'hello'\n\
         \n\
         See that you were paid (the bearer token is data/operator-bearer-token):\n\
         \n\
         \x20 curl -s -H \"Authorization: Bearer $(cat data/operator-bearer-token)\" \
         http://127.0.0.1:3000/claims | jq\n\
         \n\
         Settlement address: {settlement}\n\
         Vouchers are landed on Base Sepolia from this key every ten minutes; give it a little \
         Base Sepolia ETH from a public faucet before you expect the sweep to succeed. Admission \
         works without it.\n\
         \n\
         compose.yml runs {image}. Pin a dated release handle (--image) for a node you keep.",
        address = report.address,
        price = DEVNET_PRICE,
        settlement = report.settlement_address,
        image = report.image,
    )
}

fn config_toml(address: &str, settlement_address: &str) -> String {
    format!(
        r#"# Written by `connector init --devnet`. One file, read once at boot; an
# unknown key refuses to load, by name (ADR 0009). Every key below is
# explained in docs/guide/ (run-a-node, get-paid, operator-surface).

client_edge_addr = "0.0.0.0:3000"
state_dir        = "/app/state"

# A loopback trial publishes a plain http:// endpoint, which a deployed node
# never does: a TOON node refuses plaintext peers. Remove this line when you
# put a TLS proxy in front and publish an https:// endpoint, or a .anyone
# address, which needs no opt-in (docs/guide/run-a-node.md, hidden services).
peer_allow_plaintext_endpoints = true

[signer]
key_file = "/app/data/signer.key"

# The node's own ILP address and where a client reaches it. The address is
# also the prefix of the one route below, so a caller can `toon-client send`
# to this node without naming a destination; the endpoint is what lets a
# client pick a carriage. Not dialable by peers until `peer_expose` is set
# (docs/guide/peering.md).
#
# `http_endpoint` is published as written and a payer dials it as written, so
# 127.0.0.1 only works for a payer on this same host. When the node is exposed,
# change it to the absolute URL a payer outside this host dials: a public
# https:// URL behind a TLS proxy, or a .anyone address. The node warns at boot
# while it names a loopback or container host.
[node]
addresses     = ["{address}"]
http_endpoint = "http://127.0.0.1:3000/ilp"

# The route: everything addressed to {address} is paid for, unsealed and
# handed to the app as a plain HTTP request. `price` counts the token's
# smallest unit: with 6 decimals, 1000 is 0.001 USDC.
[[routes]]
prefix      = "{address}"
handler_url = "http://app:{APP_PORT}/"
price       = {DEVNET_PRICE}

# Base Sepolia, the public devnet (docs/guide/get-paid.md). The token is
# Circle's devnet USDC; the facilitator relays a payer's deposit so they need
# no ETH to open a channel. This node settles to the key below, whose address
# is {settlement_address}.
[settlement.evm]
rpc_url              = "{DEVNET_EVM_RPC_URL}"
token_address        = "{DEVNET_EVM_TOKEN}"
decimals             = {DEVNET_EVM_DECIMALS}
asset_eip712_name    = "{DEVNET_EVM_EIP712_NAME}"
asset_eip712_version = "{DEVNET_EVM_EIP712_VERSION}"
facilitator_url      = "{DEVNET_FACILITATOR_URL}"

[settlement.evm.key]
key_file = "/app/data/settlement.key"

# The operator surface (docs/guide/operator-surface.md): bearer-token reads
# such as GET /claims, ed25519-signed writes, and GET /dashboard. Both files
# were written beside the keys. Without this section a paid node is silent.
[operator]
bearer_token_file = "/app/data/operator-bearer-token"
write_keys_file   = "/app/data/operator-write-keys"
"#
    )
}

fn compose_yml(image: &str, uid: u32, gid: u32) -> String {
    format!(
        r#"# Written by `connector init --devnet`. Both services run as the user who ran
# init ({uid}:{gid}), so the 0600 key files in data/ and the bind-mounted
# state/ need no chown. The image's own uid is 10001; this overrides it.
services:
  connector:
    image: {image}
    user: "{uid}:{gid}"
    command: ['/app/config/connector.toml']
    volumes:
      - ./config/connector.toml:/app/config/connector.toml:ro
      - ./data:/app/data:ro
      - ./state:/app/state
    ports:
      # Loopback. Put a TLS-terminating proxy in front before it faces the
      # world, or publish it as a hidden service (docs/guide/).
      - '127.0.0.1:3000:3000'
    restart: unless-stopped
    depends_on:
      - app
    healthcheck:
      # Free and unauthenticated. Answering means the config loaded, the
      # settlement backend connected, and the router is serving.
      test: ['CMD', 'wget', '-qO-', 'http://127.0.0.1:3000/ilp/identity']
      interval: 10s
      timeout: 3s
      retries: 5

  # The app behind the route: a payment-oblivious echo this image already
  # ships. Replace it with yours; keep the port, or change handler_url.
  app:
    image: {image}
    user: "{uid}:{gid}"
    entrypoint: ['/usr/local/bin/stub-app']
    command: ['0.0.0.0:{APP_PORT}']
    restart: unless-stopped
"#
    )
}

fn check_address(address: &str) -> Result<(), InitError> {
    let ok = !address.is_empty()
        && address.starts_with("g.")
        && address.len() > 2
        && !address.ends_with('.')
        && !address.contains("..")
        && address
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'~' || b == b'-');
    if ok {
        Ok(())
    } else {
        Err(InitError::Address(format!(
            "must be an ILP address such as g.example.node (letters, digits, '.', '_', '~', \
             '-'; starting with 'g.'), got {address:?}"
        )))
    }
}

fn random_32() -> [u8; 32] {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    bytes
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The owner of `dir`, which is whoever ran `init`: the uid `compose.yml`
/// runs the containers as. On a platform without unix ownership, the image's
/// own uid.
fn owner_of(dir: &Path) -> (u32, u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let Ok(meta) = fs::metadata(dir) {
            return (meta.uid(), meta.gid());
        }
    }
    (10001, 10001)
}

fn write_file(path: &Path, contents: &[u8], secret: bool) -> Result<(), InitError> {
    let io = |source: std::io::Error| InitError::Io {
        path: path.to_path_buf(),
        source,
    };
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(if secret { 0o600 } else { 0o644 });
    }
    let mut file = options.open(path).map_err(io)?;
    file.write_all(contents).map_err(io)?;
    file.flush().map_err(io)
}

#[cfg(test)]
mod tests {
    use super::*;
    use connector_config::Config;

    fn options(dir: &Path) -> InitOptions {
        InitOptions {
            dir: dir.to_path_buf(),
            address: None,
            image: DEFAULT_IMAGE.to_string(),
        }
    }

    #[test]
    fn writes_a_node_that_the_real_config_loader_accepts() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join("node");
        let report = init(&options(&dir)).expect("init");

        assert!(report.address.starts_with("g.devnet."));
        assert!(report.settlement_address.starts_with("0x"));
        assert_eq!(report.settlement_address.len(), 42);
        for relative in [
            "config/connector.toml",
            "compose.yml",
            "data/signer.key",
            "data/settlement.key",
            "data/operator-write.key",
            "data/operator-bearer-token",
            "data/operator-write-keys",
        ] {
            assert!(dir.join(relative).is_file(), "{relative} missing");
        }
        assert!(dir.join("state").is_dir());

        // The config names container paths. Point them at this directory and
        // hand the result to the loader every node boots through, so a key
        // this verb writes and the parser refuses can never ship.
        let written = fs::read_to_string(dir.join("config/connector.toml")).expect("read");
        let local = written.replace("/app/", &format!("{}/", dir.display()));
        let local_path = dir.join("local.toml");
        fs::write(&local_path, local).expect("write local config");
        let config = Config::load(&local_path).expect("the written config loads");
        assert_eq!(config.client_edge_addr().port(), 3000);
    }

    #[cfg(unix)]
    #[test]
    fn keys_are_private_and_the_compose_file_runs_as_the_caller() {
        use std::os::unix::fs::MetadataExt;
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().to_path_buf();
        init(&options(&dir)).expect("init");
        for key_file in [
            "data/signer.key",
            "data/settlement.key",
            "data/operator-write.key",
        ] {
            let mode = fs::metadata(dir.join(key_file)).expect("meta").mode() & 0o777;
            assert_eq!(mode, 0o600, "{key_file} mode {mode:o}");
            let text = fs::read_to_string(dir.join(key_file)).expect("read");
            assert_eq!(text.len(), 64, "{key_file} is 64 hex characters");
            assert!(text.bytes().all(|b| b.is_ascii_hexdigit()));
        }
        let me = fs::metadata(&dir).expect("meta").uid();
        let compose = fs::read_to_string(dir.join("compose.yml")).expect("read");
        assert!(compose.contains(&format!("user: \"{me}:")));
        assert!(compose.contains(DEFAULT_IMAGE));
        assert!(compose.contains("/usr/local/bin/stub-app"));
    }

    #[test]
    fn a_second_run_refuses_to_touch_an_existing_node() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().to_path_buf();
        init(&options(&dir)).expect("first init");
        let before = fs::read_to_string(dir.join("data/signer.key")).expect("read");
        let error = init(&options(&dir)).expect_err("second init is refused");
        assert!(matches!(error, InitError::AlreadyInitialised { .. }));
        let after = fs::read_to_string(dir.join("data/signer.key")).expect("read");
        assert_eq!(before, after, "the signer key was not rotated");
    }

    #[test]
    fn the_address_is_the_route_and_may_be_chosen() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().to_path_buf();
        let report = init(&InitOptions {
            address: Some("g.sam".to_string()),
            ..options(&dir)
        })
        .expect("init");
        assert_eq!(report.address, "g.sam");
        let config = fs::read_to_string(dir.join("config/connector.toml")).expect("read");
        assert!(config.contains("addresses     = [\"g.sam\"]"));
        assert!(config.contains("prefix      = \"g.sam\""));
    }

    #[test]
    fn an_address_that_is_not_one_is_refused_before_anything_is_written() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join("node");
        let error = init(&InitOptions {
            address: Some("not an address".to_string()),
            ..options(&dir)
        })
        .expect_err("refused");
        assert!(matches!(error, InitError::Address(_)));
        assert!(!dir.exists());
    }

    #[test]
    fn the_report_tells_the_operator_what_to_fund_and_what_to_run() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let report = init(&options(tmp.path())).expect("init");
        let text = describe(&report);
        assert!(text.contains(&report.settlement_address));
        assert!(text.contains("docker compose up -d"));
        assert!(text.contains("toon-client channel open"));
        assert!(text.contains("/claims"));
    }
}
