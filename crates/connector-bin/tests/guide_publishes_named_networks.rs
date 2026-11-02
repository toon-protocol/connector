//! Holds the operator's guide to `connector_cli::NAMED_NETWORKS` (#1647, story 33).
//!
//! An operator configuring a node by hand copies these values, and a voucher
//! resolves against one token, so a stale address in the guide is a node nobody
//! can pay. The table is the one definition of each named network; this file
//! reads what the guide publishes and compares it. It dials nothing.
//!
//! Each lookup below panics when its heading, table or row is gone, rather than
//! passing over it: a gate that quietly stops reading the guide is worse than
//! none. As in `documented_config_keys.rs`, the guide's own rows are the input.

use std::path::Path;

use connector_cli::{
    named_network, Chain, EvmRow, NamedNetwork, RowSettlement, SolanaRow, Tier, NAMED_NETWORKS,
};

const RUN_A_NODE: &str = include_str!("../../../docs/guide/run-a-node.md");
const GET_PAID: &str = include_str!("../../../docs/guide/get-paid.md");

/// One markdown table: its header cells, and its data rows as cells.
struct Table {
    header: Vec<String>,
    rows: Vec<Vec<String>>,
}

impl Table {
    /// The first table after `heading`, within that heading's section.
    fn under(doc: &str, heading: &str) -> Table {
        let start = doc
            .lines()
            .position(|line| line == heading)
            .unwrap_or_else(|| panic!("the guide has no {heading:?} heading: repoint this test"));
        let level = heading.len() - heading.trim_start_matches('#').len();
        // A `#` inside a fenced TOML example is a comment, not a heading.
        let mut fenced = false;
        let mut in_section = |line: &&str| {
            if line.starts_with("```") {
                fenced = !fenced;
            }
            let hashes = line.len() - line.trim_start_matches('#').len();
            fenced || !(1..=level).contains(&hashes) || !line[hashes..].starts_with(' ')
        };
        let lines: Vec<&str> = doc
            .lines()
            .skip(start + 1)
            .take_while(|line| in_section(line))
            .skip_while(|line| !line.starts_with('|'))
            .take_while(|line| line.starts_with('|'))
            .collect();
        assert!(lines.len() > 2, "no table under {heading:?}");
        let cells = |line: &str| -> Vec<String> {
            line.trim()
                .trim_matches('|')
                .split('|')
                .map(|cell| cell.trim().to_string())
                .collect()
        };
        Table {
            header: cells(lines[0]),
            rows: lines[2..].iter().map(|line| cells(line)).collect(),
        }
    }

    fn column(&self, name: &str) -> usize {
        self.header
            .iter()
            .position(|cell| cell == name)
            .unwrap_or_else(|| panic!("no {name:?} column in {:?}", self.header))
    }

    /// The cell in `column` of the row whose first cell is `attribute`.
    fn cell(&self, attribute: &str, column: &str) -> &str {
        let index = self.column(column);
        let row = self
            .rows
            .iter()
            .find(|row| row[0] == attribute)
            .unwrap_or_else(|| panic!("no {attribute:?} row"));
        &row[index]
    }
}

/// The backticked spans of a cell, in order: `USDC` and `2` in "`USDC` / `2`".
fn spans(cell: &str) -> Vec<&str> {
    cell.split('`').skip(1).step_by(2).collect()
}

fn evm_row(chain: Chain, tier: Tier) -> (&'static NamedNetwork, &'static EvmRow) {
    let row = named_network(chain, tier);
    match &row.settlement {
        RowSettlement::Evm(evm) => (row, evm),
        RowSettlement::Solana(_) => panic!("{} is not an EVM row", row.name),
    }
}

fn solana_row(tier: Tier) -> (&'static NamedNetwork, &'static SolanaRow) {
    let row = named_network(Chain::Solana, tier);
    match &row.settlement {
        RowSettlement::Solana(solana) => (row, solana),
        RowSettlement::Evm(_) => panic!("{} is not a Solana row", row.name),
    }
}

/// A value in a fenced TOML example: `key` in the table headed `table`.
fn example_value<'a>(doc: &'a str, table: &str, key: &str) -> &'a str {
    let header = format!("[{table}]");
    let mut lines = doc.lines().skip_while(|line| line.trim() != header);
    assert!(lines.next().is_some(), "no {header} example in the guide");
    lines
        .take_while(|line| !line.starts_with('[') && !line.starts_with("```"))
        .filter_map(|line| line.split_once('='))
        .find(|(name, _)| name.trim() == key)
        .map(|(_, value)| {
            let value = value.split('#').next().expect("split yields one").trim();
            value.trim_matches('"')
        })
        .unwrap_or_else(|| panic!("{header} example has no {key}"))
}

/// Story 22 and the spec's guide section: `init`'s section lists all six
/// combinations, each with the values that row writes.
#[test]
fn the_init_section_lists_every_named_network() {
    let table = Table::under(RUN_A_NODE, "## The fastest start: `connector init`");
    let flags = table.column("Flags");
    assert_eq!(table.rows.len(), NAMED_NETWORKS.len());

    for network in &NAMED_NETWORKS {
        let tier = match network.tier {
            Tier::Devnet => "--devnet",
            Tier::Mainnet => "--mainnet",
        };
        let wanted = format!("{tier} --chain {}", network.chain.name());
        let found = table
            .rows
            .iter()
            .find(|row| spans(&row[flags]).contains(&wanted.as_str()))
            .unwrap_or_else(|| panic!("no row for `{wanted}`"));

        let (token_name, token) = match &network.settlement {
            RowSettlement::Evm(evm) => (evm.token_name, evm.token),
            RowSettlement::Solana(solana) => (solana.token_name, solana.mint),
        };
        assert_eq!(found[table.column("Network")], network.name, "{wanted}");
        assert_eq!(found[table.column("Token")], token_name, "{wanted}");
        assert_eq!(spans(&found[table.column("Address")]), [token], "{wanted}");
        assert_eq!(
            spans(&found[table.column("Default RPC")]),
            [network.rpc_url],
            "{wanted}"
        );
    }
}

/// Stories 22-23: Robinhood Chain's table, on both networks.
#[test]
fn the_robinhood_table_matches_its_rows() {
    let table = Table::under(GET_PAID, "## Robinhood Chain");
    for (column, tier) in [("Testnet", Tier::Devnet), ("Mainnet", Tier::Mainnet)] {
        let (network, evm) = evm_row(Chain::Robinhood, tier);
        let chain_id = evm.chain_id.to_string();
        let decimals = evm.decimals.to_string();

        assert_eq!(spans(table.cell("Chain id", column)), [chain_id.as_str()]);
        assert_eq!(spans(table.cell("RPC", column)), [network.rpc_url]);
        assert_eq!(spans(table.cell("Token", column)), [evm.token]);
        assert!(table.cell("Token", column).contains(evm.token_symbol));
        assert_eq!(spans(table.cell("Decimals", column)), [decimals.as_str()]);
        assert_eq!(
            spans(table.cell("EIP-712 domain", column)),
            [evm.eip712_name, evm.eip712_version]
        );
        assert!(table.cell("Funding the key", column).contains("ETH"));
        assert!(table
            .cell("Funding the key", column)
            .contains(evm.token_symbol));
    }
}

/// The faucet hosts named in `text`, as links or bare, sorted.
fn faucet_hosts(text: &str) -> Vec<&str> {
    let mut hosts: Vec<&str> = text
        .split(|c: char| c.is_whitespace() || "()[]".contains(c))
        .map(|word| word.strip_prefix("https://").unwrap_or(word))
        .map(|word| word.trim_end_matches(['.', ',', ';', ':']))
        .filter(|word| word.starts_with("faucet."))
        .collect();
    hosts.sort_unstable();
    hosts
}

/// Story 14: the report's funding pointer is the table's. An operator on the
/// testnet reads the report first, so it names the same two faucets, and says
/// no more about Robinhood's than the guide does: a launch post names it.
#[test]
fn the_robinhood_funding_note_points_where_the_table_does() {
    let table = Table::under(GET_PAID, "## Robinhood Chain");
    let (_, evm) = evm_row(Chain::Robinhood, Tier::Devnet);
    let note = evm
        .funding_note
        .expect("the testnet row says where funds come from");
    let cell = table.cell("Funding the key", "Testnet");

    assert_eq!(faucet_hosts(cell).len(), 2, "{cell}");
    assert_eq!(faucet_hosts(note), faucet_hosts(cell), "{note}");
    let attribution = "Arbitrum's launch post";
    assert!(cell.contains(attribution), "{cell}");
    assert!(note.contains(attribution), "{note}");

    // The mainnet cell says nothing funds a node, so the report names no faucet.
    let (_, evm) = evm_row(Chain::Robinhood, Tier::Mainnet);
    assert_eq!(evm.funding_note, None);
    assert!(faucet_hosts(table.cell("Funding the key", "Mainnet")).is_empty());
}

/// The table the guide already published for the two devnets, which is where
/// the first rows came from.
#[test]
fn the_devnet_table_matches_its_rows() {
    let table = Table::under(GET_PAID, "# 3. Get paid");

    let (network, evm) = evm_row(Chain::Base, Tier::Devnet);
    assert_eq!(
        spans(table.cell("Chain", "EVM")),
        [evm.chain_id.to_string().as_str()]
    );
    assert_eq!(spans(table.cell("RPC", "EVM")), [network.rpc_url]);
    assert_eq!(spans(table.cell("Token", "EVM"))[0], evm.token);

    let (network, solana) = solana_row(Tier::Devnet);
    assert_eq!(spans(table.cell("RPC", "Solana")), [network.rpc_url]);
    assert_eq!(spans(table.cell("Token", "Solana"))[0], solana.mint);
}

/// The TOML examples an operator copies: Base Sepolia's, Robinhood Chain
/// mainnet's and Solana devnet's.
#[test]
fn the_settlement_examples_match_their_rows() {
    for (chain, tier) in [
        (Chain::Base, Tier::Devnet),
        (Chain::Robinhood, Tier::Mainnet),
    ] {
        let (network, evm) = evm_row(chain, tier);
        let table = format!("settlement.eip155.{}", evm.chain_id);
        let value = |key| example_value(GET_PAID, &table, key);
        assert_eq!(value("rpc_url"), network.rpc_url);
        assert_eq!(value("token_address"), evm.token);
        assert_eq!(value("decimals"), evm.decimals.to_string());
        assert_eq!(value("asset_eip712_name"), evm.eip712_name);
        assert_eq!(value("asset_eip712_version"), evm.eip712_version);
    }

    let (network, solana) = solana_row(Tier::Devnet);
    let value = |key| example_value(GET_PAID, "settlement.solana", key);
    assert_eq!(value("rpc_url"), network.rpc_url);
    assert_eq!(value("token_address"), solana.mint);
    assert_eq!(value("decimals"), solana.decimals.to_string());
    assert_eq!(
        value("min_sponsored_deposit"),
        solana.min_sponsored_deposit.to_string()
    );
}

/// Base mainnet's domain is what catches an operator copying the devnet's, and
/// the guide states it in prose rather than a table.
#[test]
fn the_guide_states_base_mainnets_domain() {
    let (_, evm) = evm_row(Chain::Base, Tier::Mainnet);
    let stated = format!(
        "**`\"{}\"`** / `\"{}\"`",
        evm.eip712_name, evm.eip712_version
    );
    assert!(GET_PAID.contains(&stated), "get-paid.md lacks {stated}");
}

/// ADR 0086: the binary refuses this table by name, so an example printing it
/// is a node that does not start. Every guide page, including one added later.
#[test]
fn no_guide_page_prints_the_table_the_binary_refuses() {
    let guide = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/guide");
    let mut pages = 0;
    for entry in std::fs::read_dir(&guide).expect("docs/guide exists") {
        let path = entry.expect("a directory entry").path();
        if path.extension().is_some_and(|extension| extension == "md") {
            pages += 1;
            let text = std::fs::read_to_string(&path).expect("read a guide page");
            for (number, line) in text.lines().enumerate() {
                assert!(
                    !line.contains("settlement.evm"),
                    "{}:{} prints a table the binary refuses by name: {line}",
                    path.display(),
                    number + 1,
                );
            }
        }
    }
    assert!(pages >= 5, "found only {pages} guide pages in {guide:?}");
}
