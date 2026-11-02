//! `connector init`: a runnable node on a named network, in one directory, from
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
//!   settlement table of the [`NamedNetwork`] asked for, and an
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
//! A mainnet row exists only where a record names its token (ADR 0088), and the
//! operator must say both `--mainnet` and `--chain`, so nothing
//! is guessed. A named network is scaffolding, not a gate: the binary holds no
//! chain allowlist, a hand-written table for any EVM chain keeps working, and
//! the node still checks every value here against the chain when it boots.

use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use connector_signer::{derive_evm_address, to_hex, LocalSigner, Signer};
use rand::RngCore;

/// A settlement chain `init` can scaffold, as `--chain` names it. Not the
/// binary's list of chains it will settle on: it has none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Chain {
    Base,
    Solana,
    Robinhood,
}

impl Chain {
    const ALL: [Chain; 3] = [Chain::Base, Chain::Solana, Chain::Robinhood];

    /// What `--chain` calls it.
    pub fn name(self) -> &'static str {
        match self {
            Chain::Base => "base",
            Chain::Solana => "solana",
            Chain::Robinhood => "robinhood",
        }
    }

    /// The chain `--chain <name>` means, or a refusal listing the known names.
    pub(crate) fn from_name(name: &str) -> Result<Chain, String> {
        Chain::ALL
            .into_iter()
            .find(|chain| chain.name() == name)
            .ok_or_else(|| {
                let known: Vec<&str> = Chain::ALL.iter().map(|chain| chain.name()).collect();
                format!(
                    "unknown --chain {name:?}: known chains are {}",
                    known.join(", ")
                )
            })
    }
}

/// Which of a chain's two networks: `--devnet` or `--mainnet`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    Devnet,
    Mainnet,
}

/// One network `init` can scaffold: the one definition of its values, keyed by
/// (`chain`, `tier`).
#[derive(Debug, PartialEq, Eq)]
pub struct NamedNetwork {
    pub chain: Chain,
    pub tier: Tier,
    /// What the report calls it.
    pub name: &'static str,
    /// The RPC written unless `--rpc-url` names another.
    pub rpc_url: &'static str,
    /// The default ILP address is this, then six hex digits of the signer's
    /// public key.
    pub address_prefix: &'static str,
    pub settlement: RowSettlement,
}

/// The settlement table a row writes, in the family that row needs.
#[derive(Debug, PartialEq, Eq)]
pub enum RowSettlement {
    Evm(EvmRow),
    Solana(SolanaRow),
}

/// What a `[settlement.eip155.<chainId>]` table needs beyond its RPC.
#[derive(Debug, PartialEq, Eq)]
pub struct EvmRow {
    /// The table key, asserted against `eth_chainId` at boot (ADR 0086).
    pub chain_id: u64,
    pub token: &'static str,
    /// The token as a report names it: `devnet USDC`.
    pub token_name: &'static str,
    /// The token's unit: `USDC`.
    pub token_symbol: &'static str,
    pub decimals: u8,
    pub eip712_name: &'static str,
    pub eip712_version: &'static str,
    /// The stock x402 facilitator payers relay deposits through, so a wallet
    /// with no ETH can open a channel. This node publishes the URL and never
    /// calls it (ADR 0076).
    pub facilitator: Option<&'static str>,
    /// Where this network's funds come from, when the report should say.
    pub funding_note: Option<&'static str>,
}

/// What a `[settlement.solana]` table needs beyond its RPC.
#[derive(Debug, PartialEq, Eq)]
pub struct SolanaRow {
    pub mint: &'static str,
    /// The mint as a report names it: `mock USDC`.
    pub token_name: &'static str,
    pub decimals: u8,
    /// Bounds what sponsored opens spend of this node's lamports (ADR 0074
    /// decision 9).
    pub min_sponsored_deposit: u64,
    /// Where this network's funds come from, when the report should say.
    pub funding_note: Option<&'static str>,
}

/// Every network `init` can scaffold. The devnet rows are the values
/// `docs/guide/get-paid.md` publishes; the rest are from #1647, read off the
/// live chains on 2026-11-02. Copied, not chosen: a voucher resolves against
/// one token, so every node that might accept one has to name the same one.
pub static NAMED_NETWORKS: [NamedNetwork; 6] = [
    NamedNetwork {
        chain: Chain::Base,
        tier: Tier::Devnet,
        name: "Base Sepolia",
        rpc_url: "https://base-sepolia-rpc.publicnode.com",
        address_prefix: "g.devnet.",
        settlement: RowSettlement::Evm(EvmRow {
            chain_id: 84532,
            token: "0x0C996d7c934c79a6255254875607Fe69df25C0E1",
            token_name: "devnet USDC",
            token_symbol: "USDC",
            decimals: 6,
            eip712_name: "USDC",
            eip712_version: "2",
            facilitator: Some("https://onboard.devnet.toonprotocol.dev"),
            funding_note: None,
        }),
    },
    NamedNetwork {
        chain: Chain::Base,
        tier: Tier::Mainnet,
        name: "Base mainnet",
        rpc_url: "https://mainnet.base.org",
        address_prefix: "g.",
        settlement: RowSettlement::Evm(EvmRow {
            chain_id: 8453,
            token: "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
            token_name: "USDC",
            token_symbol: "USDC",
            decimals: 6,
            eip712_name: "USD Coin",
            eip712_version: "2",
            facilitator: None,
            funding_note: None,
        }),
    },
    NamedNetwork {
        chain: Chain::Robinhood,
        tier: Tier::Devnet,
        name: "Robinhood Chain testnet",
        rpc_url: "https://rpc.testnet.chain.robinhood.com",
        address_prefix: "g.devnet.",
        settlement: RowSettlement::Evm(EvmRow {
            chain_id: 46630,
            token: "0x7E955252E15c84f5768B83c41a71F9eba181802F",
            token_name: "USDG",
            token_symbol: "USDG",
            decimals: 6,
            eip712_name: "Global Dollar",
            eip712_version: "1",
            facilitator: None,
            funding_note: Some(
                "Testnet ETH comes from Robinhood's own testnet faucet, and testnet USDG is \
                 Paxos's (docs.paxos.com/guides/stablecoin/usdg/testnet); TOON's Base Sepolia \
                 faucet does not serve this chain.",
            ),
        }),
    },
    NamedNetwork {
        chain: Chain::Robinhood,
        tier: Tier::Mainnet,
        name: "Robinhood Chain mainnet",
        rpc_url: "https://rpc.mainnet.chain.robinhood.com",
        address_prefix: "g.",
        settlement: RowSettlement::Evm(EvmRow {
            chain_id: 4663,
            token: "0x5fc5360D0400a0Fd4f2af552ADD042D716F1d168",
            token_name: "USDG",
            token_symbol: "USDG",
            decimals: 6,
            eip712_name: "Global Dollar",
            eip712_version: "1",
            facilitator: None,
            funding_note: None,
        }),
    },
    NamedNetwork {
        chain: Chain::Solana,
        tier: Tier::Devnet,
        name: "Solana devnet",
        rpc_url: "https://api.devnet.solana.com",
        address_prefix: "g.devnet.",
        settlement: RowSettlement::Solana(SolanaRow {
            mint: "34eSxY7qxQ4GzyhDJ8GpUcTz1WWzruGbJbR8q6TtxfQU",
            token_name: "mock USDC",
            decimals: 6,
            min_sponsored_deposit: 1_000_000,
            funding_note: Some(
                "Devnet SOL comes from `solana airdrop 1 <address> -u devnet`; mock USDC from \
                 `npx toon-client faucet`.",
            ),
        }),
    },
    NamedNetwork {
        chain: Chain::Solana,
        tier: Tier::Mainnet,
        name: "Solana mainnet-beta",
        rpc_url: "https://api.mainnet-beta.solana.com",
        address_prefix: "g.",
        settlement: RowSettlement::Solana(SolanaRow {
            mint: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
            token_name: "USDC",
            decimals: 6,
            min_sponsored_deposit: 1_000_000,
            funding_note: None,
        }),
    },
];

/// The row for (`chain`, `tier`). Every pair has one.
pub(crate) fn named_network(chain: Chain, tier: Tier) -> &'static NamedNetwork {
    NAMED_NETWORKS
        .iter()
        .find(|row| row.chain == chain && row.tier == tier)
        .expect("every chain has a row on both networks")
}

impl NamedNetwork {
    /// The row `init --devnet` has always written. Its words are older than
    /// the table, and existing instructions rely on them, so they stay.
    fn is_the_original(&self) -> bool {
        self.chain == Chain::Base && self.tier == Tier::Devnet
    }

    /// The flags that select this row, as a written file's first line quotes
    /// them.
    fn flags(&self) -> String {
        match (self.is_the_original(), self.tier) {
            (true, _) => "--devnet".to_string(),
            (false, Tier::Devnet) => format!("--devnet --chain {}", self.chain.name()),
            (false, Tier::Mainnet) => format!("--mainnet --chain {}", self.chain.name()),
        }
    }
}

/// The image `compose.yml` runs when `--image` names none. A floating tag,
/// fine for the scratch trial this verb exists for and announced as such in
/// the report; a node kept running is pinned to a release handle.
pub const DEFAULT_IMAGE: &str = "ghcr.io/toon-protocol/connector:rust-main";
/// What the one route charges on every row: 0.001 of a 6-decimal token, the
/// devnet store's price.
const PRICE: u64 = 1000;

/// The port the stub app listens on inside the compose network. The same
/// port spaceturtle expects of any app, so a config written here reads the
/// same as one written there.
const APP_PORT: u16 = 3100;

/// What `connector init` was asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitOptions {
    /// Where to write. Created if missing.
    pub dir: PathBuf,
    /// The network to scaffold.
    pub network: &'static NamedNetwork,
    /// Replaces the row's default RPC in the written config.
    pub rpc_url: Option<String>,
    /// The node's ILP address, which is also its one route's prefix. `None`
    /// derives the row's prefix plus six hex digits of the signer's public key.
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
    /// `--rpc-url` is not a URL a settlement table can dial.
    RpcUrl(String),
    /// The row is in the table, but `init` cannot write its table yet.
    NotYet { chain: &'static str },
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
            InitError::RpcUrl(reason) => write!(f, "--rpc-url {reason}"),
            InitError::NotYet { chain } => write!(
                f,
                "--chain {chain} is not yet something `init` can write: its settlement table \
                 is still to come (#1647). Choose base or robinhood, or write the table by hand \
                 (docs/guide/get-paid.md)."
            ),
        }
    }
}

impl std::error::Error for InitError {}

/// What `init` produced, for the report and for tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitReport {
    pub dir: PathBuf,
    pub network: &'static NamedNetwork,
    /// Whether the written config names the row's public RPC rather than the
    /// operator's own.
    pub default_rpc: bool,
    pub address: String,
    /// The EVM address of `data/settlement.key`: where vouchers settle, and
    /// what has to hold the network's ETH for the node to land them.
    pub settlement_address: String,
    pub image: String,
    pub files: Vec<PathBuf>,
}

/// Write a node on `options.network` into `options.dir`. Every refusal comes
/// before the first write.
pub fn init(options: &InitOptions) -> Result<InitReport, InitError> {
    let network = options.network;
    let evm = match &network.settlement {
        RowSettlement::Evm(evm) => evm,
        RowSettlement::Solana(_) => {
            return Err(InitError::NotYet {
                chain: network.chain.name(),
            })
        }
    };
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
    if let Some(rpc_url) = &options.rpc_url {
        check_rpc_url(rpc_url)?;
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
        .unwrap_or_else(|| format!("{}{}", network.address_prefix, hex(&signer_public[1..4])));

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
        &config_toml(
            network,
            evm,
            options.rpc_url.as_deref().unwrap_or(network.rpc_url),
            &address,
            &settlement_address,
        ),
        false,
    )?;
    write(
        "compose.yml",
        &compose_yml(network, &options.image, uid, gid),
        false,
    )?;

    Ok(InitReport {
        dir: dir.clone(),
        network,
        default_rpc: options.rpc_url.is_none(),
        address,
        settlement_address,
        image: options.image.clone(),
        files,
    })
}

/// The report an operator reads once, with the three things they will need
/// next: how to start it, how to see it, and what to fund.
pub fn describe(report: &InitReport) -> String {
    let network = report.network;
    let evm = match &network.settlement {
        RowSettlement::Evm(evm) => evm,
        RowSettlement::Solana(_) => unreachable!("`init` refuses to write a Solana row"),
    };
    let dir = report.dir.display();
    let (node, pay, fund) = if network.is_the_original() {
        (
            "devnet",
            original_pay_section(),
            original_fund_section(report),
        )
    } else {
        (network.name, pay_section(evm), fund_section(report, evm))
    };
    format!(
        "Wrote a {node} node to {dir}:\n\
         \n\
         \x20 config/connector.toml   one route, {address}, at {PRICE} units (0.001 {token})\n\
         \x20 data/                   signer, settlement and operator keys, 0600, owned by you\n\
         \x20 state/                  the claim journal; keep it, it is the watermark\n\
         \x20 compose.yml             the connector and the stub app, as your own uid\n\
         \n\
         Start it, then ask it what it is:\n\
         \n\
         \x20 docker compose up -d\n\
         \x20 curl -s http://127.0.0.1:3000/ilp | jq .routes\n\
         \n\
         {pay}\n\
         \n\
         See that you were paid (the bearer token is data/operator-bearer-token):\n\
         \n\
         \x20 curl -s -H \"Authorization: Bearer $(cat data/operator-bearer-token)\" \
         http://127.0.0.1:3000/claims | jq\n\
         \n\
         {fund}\n\
         \n\
         compose.yml runs {image}. Pin a dated release handle (--image) for a node you keep.",
        address = report.address,
        token = evm.token_name,
        image = report.image,
    )
}

const CHANNEL_AND_SEND: &str = "\x20 npx toon-client channel open --connector \
     http://127.0.0.1:3000 --deposit 100000\n\
     \x20 npx toon-client send --connector http://127.0.0.1:3000 --body 'hello'";

fn original_pay_section() -> String {
    format!(
        "Pay it from another terminal (devnet USDC from the faucet, no ETH needed):\n\
         \n\
         \x20 npx toon-client faucet\n\
         {CHANNEL_AND_SEND}"
    )
}

fn original_fund_section(report: &InitReport) -> String {
    format!(
        "Settlement address: {settlement}\n\
         Vouchers are landed on Base Sepolia from this key every ten minutes; give it a little \
         Base Sepolia ETH from a public faucet before you expect the sweep to succeed. Admission \
         works without it.",
        settlement = report.settlement_address,
    )
}

/// What a payer needs on a row no facilitator serves.
fn pay_section(evm: &EvmRow) -> String {
    format!(
        "Pay it from another terminal. The payer needs {symbol} and its own ETH: nothing relays a \
         deposit on this network, so it pays the gas to open a channel itself.\n\
         \n\
         {CHANNEL_AND_SEND}",
        symbol = evm.token_symbol,
    )
}

fn fund_section(report: &InitReport, evm: &EvmRow) -> String {
    let network = report.network;
    let symbol = evm.token_symbol;
    let mut text = format!(
        "Settlement address: {settlement}\n\
         This key must hold ETH for gas and {symbol} on {name}. Vouchers are landed from it every \
         ten minutes; admission works without them.\n\
         Prices count {symbol}'s base units: with {decimals} decimals, {PRICE} is 0.001 {symbol}.",
        settlement = report.settlement_address,
        name = network.name,
        decimals = evm.decimals,
    );
    if let Some(note) = evm.funding_note {
        text.push('\n');
        text.push_str(note);
    }
    if network.tier == Tier::Mainnet {
        text.push_str("\n\nThis node settles real money.");
        if report.default_rpc {
            text.push_str(&format!(
                " Its RPC, {rpc}, is a public, rate-limited default and is not meant for \
                 production: change rpc_url in config/connector.toml before you rely on it.",
                rpc = network.rpc_url,
            ));
        }
        text.push_str(
            " This scaffold is a loopback trial: before you expose it, remove \
             peer_allow_plaintext_endpoints from config/connector.toml and publish an https:// \
             or .anyone endpoint.",
        );
    }
    text
}

fn config_toml(
    network: &NamedNetwork,
    evm: &EvmRow,
    rpc_url: &str,
    address: &str,
    settlement_address: &str,
) -> String {
    let flags = network.flags();
    let token_symbol = evm.token_symbol;
    let decimals = evm.decimals;
    let chain_id = evm.chain_id;
    let token = evm.token;
    let eip712_name = evm.eip712_name;
    let eip712_version = evm.eip712_version;
    let about_the_table = settlement_comment(network, evm, settlement_address);
    let facilitator = evm
        .facilitator
        .map(|url| format!("facilitator_url      = \"{url}\"\n"))
        .unwrap_or_default();
    format!(
        r#"# Written by `connector init {flags}`. One file, read once at boot; an
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
# smallest unit: with {decimals} decimals, {PRICE} is 0.001 {token_symbol}.
[[routes]]
prefix      = "{address}"
handler_url = "http://app:{APP_PORT}/"
price       = {PRICE}

{about_the_table}
[settlement.eip155.{chain_id}]
rpc_url              = "{rpc_url}"
token_address        = "{token}"
decimals             = {decimals}
asset_eip712_name    = "{eip712_name}"
asset_eip712_version = "{eip712_version}"
{facilitator}
[settlement.eip155.{chain_id}.key]
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

/// The comment above the settlement table.
fn settlement_comment(network: &NamedNetwork, evm: &EvmRow, settlement_address: &str) -> String {
    if network.is_the_original() {
        return format!(
            "# Base Sepolia, the public devnet (docs/guide/get-paid.md). The token is\n\
             # Circle's devnet USDC; the facilitator relays a payer's deposit so they need\n\
             # no ETH to open a channel. This node settles to the key below, whose address\n\
             # is {settlement_address}."
        );
    }
    let real_money = match network.tier {
        Tier::Mainnet => "\n# This node settles real money.",
        Tier::Devnet => "",
    };
    format!(
        "# {name} (docs/guide/get-paid.md). The token is {token}.{real_money}\n\
         # Nothing relays a payer's deposit here, so a payer needs its own ETH to open a\n\
         # channel. This node settles to the key below, whose address\n\
         # is {settlement_address}.",
        name = network.name,
        token = evm.token_name,
    )
}

fn compose_yml(network: &NamedNetwork, image: &str, uid: u32, gid: u32) -> String {
    let flags = network.flags();
    format!(
        r#"# Written by `connector init {flags}`. Both services run as the user who ran
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

/// The shape the loader will accept, checked now so a malformed URL is refused
/// before anything exists. Stricter than the loader on one point: `Url` silently
/// drops a tab or newline, and this value lands inside a quoted TOML string, so
/// anything that would need escaping there is refused rather than escaped.
fn check_rpc_url(rpc_url: &str) -> Result<(), InitError> {
    let refused = || {
        InitError::RpcUrl(format!(
            "must be an http:// or https:// URL, got {rpc_url:?}"
        ))
    };
    if rpc_url
        .chars()
        .any(|c| c.is_control() || c.is_whitespace() || c == '"' || c == '\\')
    {
        return Err(refused());
    }
    let url = url::Url::parse(rpc_url).map_err(|_| refused())?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(refused());
    }
    Ok(())
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
            network: named_network(Chain::Base, Tier::Devnet),
            rpc_url: None,
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

    /// The verb as the operator drives it: `connector init <args>`, through
    /// the same parser and the same `run` the binary calls. `Ok` is the report.
    fn connector_init(args: &[&str]) -> Result<String, crate::CliError> {
        let argv: Vec<&str> = ["connector", "init"].iter().chain(args).copied().collect();
        let command = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime")
            .block_on(crate::run(&argv))?;
        match command {
            crate::Command::Finished { summary } => Ok(summary),
            crate::Command::Serve(_) => panic!("init does not serve"),
        }
    }

    /// Replace the three values a run draws or inherits with the placeholders
    /// the pinned files were normalised to.
    fn normalised(text: &str, dir: &Path, report: &str) -> String {
        let after = |marker: &str| -> String {
            let start = report.find(marker).expect(marker) + marker.len();
            report[start..]
                .split(|c: char| c == ',' || c.is_whitespace())
                .next()
                .expect("a value")
                .to_string()
        };
        let owner = {
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                let meta = fs::metadata(dir).expect("meta");
                format!("{}:{}", meta.uid(), meta.gid())
            }
            #[cfg(not(unix))]
            {
                "10001:10001".to_string()
            }
        };
        text.replace(&dir.display().to_string(), "<DIR>")
            .replace(&after("one route, "), "<ADDRESS>")
            .replace(&after("Settlement address: "), "<SETTLEMENT>")
            .replace(&owner, "<OWNER>")
    }

    /// Story 12. These three files are what the binary printed and wrote
    /// before `init` had a second network, normalised and committed before the
    /// refactor: existing instructions rely on this exact node.
    #[test]
    fn a_bare_devnet_node_is_byte_for_byte_what_it_always_was() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join("node");
        let report = connector_init(&["--devnet", "--dir", dir.to_str().unwrap()]).expect("init");

        assert_eq!(
            normalised(&report, &dir, &report),
            include_str!("../fixtures/init-devnet/report.txt"),
        );
        let config = fs::read_to_string(dir.join("config/connector.toml")).expect("read");
        assert_eq!(
            normalised(&config, &dir, &report),
            include_str!("../fixtures/init-devnet/connector.toml"),
        );
        let compose = fs::read_to_string(dir.join("compose.yml")).expect("read");
        assert_eq!(
            normalised(&compose, &dir, &report),
            include_str!("../fixtures/init-devnet/compose.yml"),
        );
    }

    /// What a table must resolve to through the loader. The figures are the
    /// spec's, typed here rather than read off the table, so the two can
    /// disagree.
    struct Expected {
        args: &'static [&'static str],
        chain_id: u64,
        rpc_url: &'static str,
        token: &'static str,
        eip712_name: &'static str,
        eip712_version: &'static str,
        facilitator: Option<&'static str>,
    }

    const EVM_ROWS: [Expected; 5] = [
        Expected {
            args: &["--devnet"],
            chain_id: 84532,
            rpc_url: "https://base-sepolia-rpc.publicnode.com",
            token: "0c996d7c934c79a6255254875607fe69df25c0e1",
            eip712_name: "USDC",
            eip712_version: "2",
            facilitator: Some("https://onboard.devnet.toonprotocol.dev"),
        },
        Expected {
            args: &["--devnet", "--chain", "base"],
            chain_id: 84532,
            rpc_url: "https://base-sepolia-rpc.publicnode.com",
            token: "0c996d7c934c79a6255254875607fe69df25c0e1",
            eip712_name: "USDC",
            eip712_version: "2",
            facilitator: Some("https://onboard.devnet.toonprotocol.dev"),
        },
        Expected {
            args: &["--mainnet", "--chain", "base"],
            chain_id: 8453,
            rpc_url: "https://mainnet.base.org",
            token: "833589fcd6edb6e08f4c7c32d4f71b54bda02913",
            eip712_name: "USD Coin",
            eip712_version: "2",
            facilitator: None,
        },
        Expected {
            args: &["--devnet", "--chain", "robinhood"],
            chain_id: 46630,
            rpc_url: "https://rpc.testnet.chain.robinhood.com",
            token: "7e955252e15c84f5768b83c41a71f9eba181802f",
            eip712_name: "Global Dollar",
            eip712_version: "1",
            facilitator: None,
        },
        Expected {
            args: &["--mainnet", "--chain", "robinhood"],
            chain_id: 4663,
            rpc_url: "https://rpc.mainnet.chain.robinhood.com",
            token: "5fc5360d0400a0fd4f2af552add042d716f1d168",
            eip712_name: "Global Dollar",
            eip712_version: "1",
            facilitator: None,
        },
    ];

    fn init_args<'a>(args: &[&'a str], dir: &'a Path) -> Vec<&'a str> {
        // `--dir` first, so a flag missing its value at the end stays missing it.
        let mut all = vec!["--dir", dir.to_str().expect("utf-8 tempdir")];
        all.extend(args);
        all
    }

    /// The written config, its container paths pointed at this directory,
    /// through the loader every node boots through.
    fn load_written(dir: &Path) -> Config {
        let written = fs::read_to_string(dir.join("config/connector.toml")).expect("read");
        let local = written.replace("/app/", &format!("{}/", dir.display()));
        let local_path = dir.join("local.toml");
        fs::write(&local_path, local).expect("write local config");
        Config::load(&local_path).expect("the written config loads")
    }

    fn the_evm_table(config: &Config) -> &connector_config::EvmSettlementConfig {
        match config.settlements() {
            [connector_config::SettlementConfig::Evm(table)] => table,
            other => panic!("expected one EVM table, got {other:?}"),
        }
    }

    /// Stories 1, 2, 2b, 2d, 3, 4, 5, 15 and 31: every EVM row loads, and
    /// resolves to that network's own token and domain.
    #[test]
    fn each_evm_row_loads_and_resolves_to_its_own_network_token_and_domain() {
        for expected in &EVM_ROWS {
            let tmp = tempfile::tempdir().expect("tempdir");
            let dir = tmp.path().join("node");
            connector_init(&init_args(expected.args, &dir))
                .unwrap_or_else(|error| panic!("{:?}: {error}", expected.args));
            let config = load_written(&dir);
            let table = the_evm_table(&config);

            assert_eq!(table.chain_id(), expected.chain_id, "{:?}", expected.args);
            assert_eq!(
                table.network().to_string(),
                format!("eip155:{}", expected.chain_id)
            );
            assert_eq!(table.rpc_url(), expected.rpc_url);
            assert_eq!(hex(&table.token_address()), expected.token);
            assert_eq!(table.decimals(), 6);
            let terms = table.batch_settlement();
            assert_eq!(terms.asset_eip712_name(), expected.eip712_name);
            assert_eq!(terms.asset_eip712_version(), expected.eip712_version);
            assert_eq!(terms.facilitator_url(), expected.facilitator);
            assert_eq!(config.routes().len(), 1);
            assert_eq!(
                config.routes()[0].price(),
                connector_domain::Price::flat(1000)
            );
        }
    }

    /// Story 18: a mainnet node's default address does not claim to be on
    /// devnet, and a devnet one still says so.
    #[test]
    fn the_default_address_follows_the_row() {
        for expected in &EVM_ROWS {
            let tmp = tempfile::tempdir().expect("tempdir");
            let dir = tmp.path().join("node");
            connector_init(&init_args(expected.args, &dir)).expect("init");
            let config = load_written(&dir);
            let prefix = config.routes()[0].prefix().to_string();
            let tail = if expected.args[0] == "--mainnet" {
                assert!(!prefix.contains("devnet"), "{prefix}");
                prefix.strip_prefix("g.").expect("g.")
            } else {
                prefix.strip_prefix("g.devnet.").expect("g.devnet.")
            };
            assert_eq!(tail.len(), 6, "{prefix}");
            assert!(tail.bytes().all(|b| b.is_ascii_hexdigit()), "{prefix}");
        }
    }

    /// Stories 8-11 and 34's grammar. Every refusal names its cause and leaves
    /// no trace, not even the directory.
    #[test]
    fn every_refusal_names_its_cause_and_writes_nothing() {
        let cases: &[(&[&str], &[&str])] = &[
            (&[], &["--devnet", "--mainnet"]),
            (&["--chain", "base"], &["--devnet", "--mainnet"]),
            (&["--mainnet"], &["--mainnet", "must name a chain"]),
            (
                &["--devnet", "--mainnet"],
                &["--devnet", "--mainnet", "not both"],
            ),
            (&["--devnet", "--mainnet", "--chain", "base"], &["not both"]),
            (
                &["--devnet", "--chain", "polygon"],
                &["polygon", "base", "solana", "robinhood"],
            ),
            (
                &["--mainnet", "--chain", "xrpl"],
                &["xrpl", "base", "solana", "robinhood"],
            ),
            (&["--devnet", "--chain"], &["--chain needs a value"]),
            (&["--devnet", "--rpc-url"], &["--rpc-url needs a value"]),
            (&["--devnet", "--network"], &["--network"]),
            (&["--devnet", "--chain", "solana"], &["solana", "not yet"]),
            (&["--mainnet", "--chain", "solana"], &["solana", "not yet"]),
            (&["--devnet", "--address", "nope"], &["--address"]),
            (
                &["--devnet", "--rpc-url", "not a url"],
                &["--rpc-url", "not a url"],
            ),
            (
                &[
                    "--mainnet",
                    "--chain",
                    "base",
                    "--rpc-url",
                    "ftp://rpc.example",
                ],
                &["--rpc-url", "http"],
            ),
            (
                &["--mainnet", "--chain", "base", "--rpc-url", "https://"],
                &["--rpc-url"],
            ),
            // `Url` drops a newline and a tab on its own, and this value lands
            // inside a quoted TOML string: neither may reach the file.
            (
                &["--devnet", "--rpc-url", "https://rpc.example/\nfoo"],
                &["--rpc-url"],
            ),
            (
                &["--devnet", "--rpc-url", "https://rpc.example/a\"b"],
                &["--rpc-url"],
            ),
        ];
        for (args, names) in cases {
            let tmp = tempfile::tempdir().expect("tempdir");
            let dir = tmp.path().join("node");
            let error = match connector_init(&init_args(args, &dir)) {
                Ok(_) => panic!("{args:?} should be refused"),
                Err(error) => error.to_string(),
            };
            for name in *names {
                assert!(error.contains(name), "{args:?} names {name}: {error}");
            }
            assert!(!dir.exists(), "{args:?} wrote something");
        }
    }

    /// Story 6: the URL lands as written, on any row, and a loader that reads
    /// it back finds the operator's.
    #[test]
    fn rpc_url_replaces_the_default_in_the_written_config() {
        for args in [
            ["--devnet", "--chain", "base"],
            ["--mainnet", "--chain", "robinhood"],
        ] {
            let tmp = tempfile::tempdir().expect("tempdir");
            let dir = tmp.path().join("node");
            let mut all = init_args(&args, &dir);
            all.extend(["--rpc-url", "https://rpc.example.org/v1/abc?x=1"]);
            connector_init(&all).expect("init");
            let config = load_written(&dir);
            assert_eq!(
                the_evm_table(&config).rpc_url(),
                "https://rpc.example.org/v1/abc?x=1"
            );
        }
    }

    /// Story 21, on a row that is not the original.
    #[cfg(unix)]
    #[test]
    fn a_new_row_writes_its_keys_with_the_same_private_modes() {
        use std::os::unix::fs::MetadataExt;
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join("node");
        connector_init(&init_args(&["--mainnet", "--chain", "robinhood"], &dir)).expect("init");
        for secret in [
            "data/signer.key",
            "data/settlement.key",
            "data/operator-write.key",
            "data/operator-bearer-token",
        ] {
            let mode = fs::metadata(dir.join(secret)).expect("meta").mode() & 0o777;
            assert_eq!(mode, 0o600, "{secret} mode {mode:o}");
        }
    }

    /// Story 20, on a row that is not the original, and across rows: choosing
    /// another chain is no way around it.
    #[test]
    fn a_repeat_init_on_a_new_row_is_refused_and_rotates_nothing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join("node");
        let first = ["--mainnet", "--chain", "robinhood"];
        connector_init(&init_args(&first, &dir)).expect("first init");
        let before = fs::read_to_string(dir.join("data/signer.key")).expect("read");
        for again in [&first[..], &["--devnet"][..]] {
            let error = match connector_init(&init_args(again, &dir)) {
                Ok(_) => panic!("{again:?} should be refused"),
                Err(error) => error.to_string(),
            };
            assert!(error.contains("already exists"), "{error}");
        }
        let after = fs::read_to_string(dir.join("data/signer.key")).expect("read");
        assert_eq!(before, after, "the signer key was not rotated");
    }

    fn report_for(args: &[&str]) -> String {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join("node");
        connector_init(&init_args(args, &dir)).expect("init")
    }

    /// Stories 13-16: Robinhood Chain's reports name USDG, not USDC, both
    /// assets to hold, and no faucet of TOON's.
    #[test]
    fn a_robinhood_report_names_its_token_and_what_to_hold() {
        for args in [
            ["--devnet", "--chain", "robinhood"],
            ["--mainnet", "--chain", "robinhood"],
        ] {
            let text = report_for(&args);
            assert!(!text.contains("USDC"), "{args:?}: {text}");
            assert!(text.contains("ETH"), "{text}");
            assert!(text.contains("USDG"), "{text}");
            assert!(text.contains("Settlement address: 0x"), "{text}");
            assert!(text.contains("6 decimals"), "{text}");
            assert!(text.contains("0.001 USDG"), "{text}");
            assert!(!text.contains("toon-client faucet"), "{text}");
            // No facilitator serves these rows, so a payer needs its own gas.
            assert!(text.contains("own ETH"), "{text}");
        }
        let testnet = report_for(&["--devnet", "--chain", "robinhood"]);
        assert!(
            testnet.contains("Robinhood's own testnet faucet"),
            "{testnet}"
        );
        assert!(testnet.contains("TOON's"), "{testnet}");
    }

    /// Stories 7, 17 and 19.
    #[test]
    fn a_mainnet_report_says_real_money_public_rpc_and_plaintext() {
        for args in [
            ["--mainnet", "--chain", "base"],
            ["--mainnet", "--chain", "robinhood"],
        ] {
            let text = report_for(&args);
            assert!(text.contains("real money"), "{text}");
            assert!(text.contains("not meant for production"), "{text}");
            assert!(text.contains("peer_allow_plaintext_endpoints"), "{text}");
            assert!(text.contains("https://"), "{text}");
            assert!(text.contains(".anyone"), "{text}");
            assert!(!text.contains("toon-client faucet"), "{text}");
        }
        assert!(report_for(&["--mainnet", "--chain", "base"]).contains("0.001 USDC"));
        // A devnet report carries none of this.
        for args in [&["--devnet"][..], &["--devnet", "--chain", "robinhood"][..]] {
            let text = report_for(args);
            assert!(!text.contains("real money"), "{args:?}: {text}");
        }
    }

    /// Named your own endpoint: the warning about the default is not yours.
    #[test]
    fn a_mainnet_report_with_its_own_rpc_drops_only_the_rpc_warning() {
        let text = report_for(&[
            "--mainnet",
            "--chain",
            "base",
            "--rpc-url",
            "https://rpc.example.org",
        ]);
        assert!(!text.contains("not meant for production"), "{text}");
        assert!(text.contains("real money"), "{text}");
    }

    /// Stories 15 and 2d, in the file itself: its comments do not call a
    /// mainnet token a devnet one, or USDG USDC.
    #[test]
    fn the_written_config_names_the_rows_own_token_and_invocation() {
        let read = |args: &[&str]| {
            let tmp = tempfile::tempdir().expect("tempdir");
            let dir = tmp.path().join("node");
            connector_init(&init_args(args, &dir)).expect("init");
            (
                fs::read_to_string(dir.join("config/connector.toml")).expect("read"),
                fs::read_to_string(dir.join("compose.yml")).expect("read"),
            )
        };
        let (config, compose) = read(&["--mainnet", "--chain", "robinhood"]);
        assert!(config.contains("connector init --mainnet --chain robinhood"));
        assert!(compose.contains("connector init --mainnet --chain robinhood"));
        assert!(config.contains("USDG"), "{config}");
        assert!(!config.contains("USDC"), "{config}");
        assert!(!config.contains("Base Sepolia"), "{config}");
        assert!(!config.contains("facilitator"), "{config}");
        assert!(config.contains("peer_allow_plaintext_endpoints = true"));

        let (config, _) = read(&["--mainnet", "--chain", "base"]);
        assert!(config.contains("USDC"), "{config}");
        assert!(!config.contains("devnet"), "{config}");
    }

    /// `--devnet --chain base` is the same node as bare `--devnet`.
    #[test]
    fn naming_base_on_devnet_changes_nothing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join("node");
        let report =
            connector_init(&init_args(&["--devnet", "--chain", "base"], &dir)).expect("init");
        assert_eq!(
            normalised(&report, &dir, &report),
            include_str!("../fixtures/init-devnet/report.txt"),
        );
        let config = fs::read_to_string(dir.join("config/connector.toml")).expect("read");
        assert_eq!(
            normalised(&config, &dir, &report),
            include_str!("../fixtures/init-devnet/connector.toml"),
        );
    }

    /// Reachable from another crate: a later drift test reads the guide
    /// against this table from `connector-bin`'s tests. Holds all six rows,
    /// Solana's included, with the spec's Solana values.
    #[test]
    fn the_table_holds_all_six_rows_once_each() {
        let mut keys: Vec<(Chain, Tier)> = crate::NAMED_NETWORKS
            .iter()
            .map(|row| (row.chain, row.tier))
            .collect();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), 6);
        assert_eq!(crate::NAMED_NETWORKS.len(), 6);

        let solana = |tier: Tier| match crate::NAMED_NETWORKS
            .iter()
            .find(|row| row.chain == Chain::Solana && row.tier == tier)
            .expect("a Solana row")
        {
            NamedNetwork {
                rpc_url,
                settlement: RowSettlement::Solana(row),
                ..
            } => (*rpc_url, row.mint, row.decimals, row.min_sponsored_deposit),
            other => panic!("a Solana row holds {other:?}"),
        };
        assert_eq!(
            solana(Tier::Devnet),
            (
                "https://api.devnet.solana.com",
                "34eSxY7qxQ4GzyhDJ8GpUcTz1WWzruGbJbR8q6TtxfQU",
                6,
                1_000_000
            )
        );
        assert_eq!(
            solana(Tier::Mainnet),
            (
                "https://api.mainnet-beta.solana.com",
                "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
                6,
                1_000_000
            )
        );
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
