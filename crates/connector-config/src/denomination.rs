use std::collections::HashSet;
use std::time::Duration;

use chrono::TimeDelta;
use connector_domain::{AssetChain, AssetId, GuardOverride, Guards, MaxMove, Rate, Spread, Ttl};
use serde::Deserialize;

use crate::client_channel::{is_base58_32_bytes, parse_evm_address, to_hex};
use crate::error::ConfigError;
use crate::settlement::{SettlementChain, SettlementTables};

/// The most pools one token's quote path may name (ADR 0071 decision 3):
/// one for a numeraire-quoted token, two for a token whose only real venue
/// is quoted in an intermediate. A third leg is not a longer path, it is a
/// path nobody has thought about -- each leg is an independent
/// manipulation surface and the legs' errors compose.
const MAX_QUOTE_LEGS: usize = 2;

/// One `[[tokens]]` row as written in the config file (ADR 0071 decision
/// 3): a token this node deals, and optionally where its price comes from.
///
/// An array of tables rather than a keyed one, unlike
/// [`crate::settlement::RawKeyedSettlementConfig`], and for the reason that
/// one is keyed: a settlement table is keyed by chain because there is
/// exactly one per chain and the chain's own name is the natural key. A
/// token has no such key -- `evm:0x833589...` is the value, not a heading --
/// and a TOML table keyed by a contract address would put a 42-character
/// literal in a section header where a typo is hardest to see.
/// `deny_unknown_fields` for ADR 0009's standing reason.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawToken {
    asset: String,
    /// Whether this token is the one every quote path ends at. Exactly one
    /// row may set it; two rows setting it is the mixed numeraire ADR 0071
    /// decision 3 refuses at boot, because a node composing cross rates
    /// through two different pegs is doing peg arithmetic nobody declared.
    #[serde(default)]
    numeraire: bool,
    /// One or two operator-named AMM pools on this token's own settlement
    /// chain, ending at the numeraire. Absent means this token's pairs are
    /// priced by `[[rates]]` rows the operator tends by hand -- which is
    /// the only option for a thin token, a Solana-side pool, or no pool at
    /// all.
    #[serde(default)]
    quote: Option<Vec<RawQuoteLeg>>,
}

/// One leg of a token's quote path: which pool is read, which token it
/// quotes into, and over what window.
///
/// The window is per leg rather than per path because the legs are
/// genuinely different reads: a deep WETH/numeraire pool answers a short
/// window honestly where a thin long-tail pool does not, and a path forced
/// to one window would be tuned for its worse leg.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawQuoteLeg {
    pool: String,
    quote_token: String,
    twap_window_secs: u64,
}

/// One `[[rates]]` row: what the operator declares about one **ordered**
/// pair (ADR 0071 decision 3). Direction is the trade, so `from`/`to` are
/// never sorted and a pair is not its own reverse.
///
/// `rate` is optional, and its absence is the difference between the two
/// things a row can be. With one, the row is the pair's declared rate --
/// the only way to price a pair that cannot self-source, and a per-pair
/// override of a composed one where it can. Without one, the row says
/// nothing about price and only tightens the pair's guards over the node
/// defaults, which is how a thin pair is tightened without restating
/// policy everywhere (ADR 0071 decision 5).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawRateRow {
    from: String,
    to: String,
    #[serde(default)]
    rate: Option<RawFraction>,
    #[serde(default)]
    spread: Option<RawFraction>,
    #[serde(default)]
    ttl_secs: Option<u64>,
    #[serde(default)]
    max_move: Option<RawFraction>,
}

/// Every declared fraction an operator writes -- a `rate`, a `spread`, a
/// `max_move` -- in the one shape all three take: `{ numerator = ...,
/// denominator = ... }`.
///
/// A fraction rather than a percentage or a basis point, which is
/// `connector_domain::Spread`'s own choice and its reasoning: there are no
/// floats on this path, and an operator dealing at half a basis point
/// writes `1/20000` rather than watching it round to zero.
///
/// Deliberately **not** the domain types deserialized directly, even though
/// `Rate` reads exactly this shape. A type that refuses itself during
/// deserialization fails with serde's own message, which names a line in a
/// file and not the row the value is on; [`resolve_rate_rows`] instead
/// builds each one through its own constructor and hands the refusal to a
/// [`ConfigError`] that names the row. No check is reimplemented here --
/// a zero denominator is unconstructable in the domain either way, and
/// this only decides which words the operator reads.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawFraction {
    numerator: u64,
    denominator: u64,
}

/// The `[rate_guards]` table: this node's dealing policy, stated once
/// (ADR 0071 decision 5).
///
/// All three are required rather than defaulted, because no default is
/// safe to invent. A defaulted `spread` would be zero -- dealing at mid and
/// donating the risk, which `Spread::none()` says a node may choose but
/// nothing should choose for it -- and a defaulted `ttl` or `max_move`
/// would be a number this record does not name being enforced on
/// somebody's book. An operator who deals states the three; one who does
/// not writes no table at all.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawRateGuards {
    spread: RawFraction,
    ttl_secs: u64,
    max_move: RawFraction,
}

/// One leg of a validated quote path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuoteLeg {
    pool: String,
    quote_token: AssetId,
    twap_window: Duration,
}

impl QuoteLeg {
    /// The pool this leg reads, canonicalized the way every other address
    /// in this crate is: lowercase `0x` hex on EVM, byte-for-byte base58 on
    /// Solana. The one pool -- there is no discovery, and a pool the
    /// operator cannot name does not exist.
    pub fn pool(&self) -> &str {
        &self.pool
    }

    /// The token this leg quotes into: the intermediate on a two-leg path,
    /// and the numeraire on the last leg of any path.
    pub fn quote_token(&self) -> &AssetId {
        &self.quote_token
    }

    /// The TWAP window this leg is read over. Never zero -- a window of
    /// nothing is a spot read, and ADR 0071 decision 3 has none.
    pub fn twap_window(&self) -> Duration {
        self.twap_window
    }
}

/// A validated quote path: one or two legs, every one on the token's own
/// settlement chain, the last ending at the numeraire.
///
/// What a poller walks (issue #1294) and what a [`crate::Config`] hands it
/// without re-reading the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotePath {
    legs: Vec<QuoteLeg>,
}

impl QuotePath {
    /// The legs, in the order they compose: this token into the first leg's
    /// `quote_token`, that into the second's, which is the numeraire.
    pub fn legs(&self) -> &[QuoteLeg] {
        &self.legs
    }

    /// Which chain every leg of this path is read on -- the token's own,
    /// which is the one chain the peering already guarantees RPC for.
    pub fn chain(&self) -> AssetChain {
        // Non-empty by construction, and every leg shares one chain: both
        // are `resolve_tokens`'s refusals.
        self.legs[0].quote_token.chain()
    }
}

/// A validated `[[tokens]]` row: one token this node deals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredToken {
    asset: AssetId,
    numeraire: bool,
    quote: Option<QuotePath>,
}

impl DeclaredToken {
    /// Which token this row declares.
    pub fn asset(&self) -> &AssetId {
        &self.asset
    }

    /// Whether this is the node's one numeraire -- the token every quote
    /// path ends at and every cross rate composes through.
    pub fn is_numeraire(&self) -> bool {
        self.numeraire
    }

    /// Where this token's price is read from, or `None` for a token whose
    /// pairs are priced by `[[rates]]` rows alone.
    pub fn quote(&self) -> Option<&QuotePath> {
        self.quote.as_ref()
    }
}

/// A validated `[[rates]]` row: what this node declares about one ordered
/// pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RateRow {
    from: AssetId,
    to: AssetId,
    rate: Option<Rate>,
    guards: GuardOverride,
}

impl RateRow {
    /// The incoming token of the ordered pair.
    pub fn from(&self) -> &AssetId {
        &self.from
    }

    /// The outgoing token of the ordered pair.
    pub fn to(&self) -> &AssetId {
        &self.to
    }

    /// The rate declared for the pair, or `None` for a row that only
    /// tightens the pair's guards.
    pub fn rate(&self) -> Option<Rate> {
        self.rate
    }

    /// What this row says differently from the node's `[rate_guards]`,
    /// already in the shape `Guards::overridden_by` takes -- every field
    /// unset is a pair that takes the node's policy whole.
    /// [`DenominationConfig::guards_for`] is the resolved answer; this is
    /// what the operator actually wrote.
    pub fn guard_override(&self) -> GuardOverride {
        self.guards
    }
}

/// Everything this node declares about denomination (ADR 0071 decisions 3
/// and 5): the tokens it deals, which one is its numeraire, the rates and
/// guards it has written down for ordered pairs, and its node-wide dealing
/// policy.
///
/// **Empty is the default and is a whole answer**: a node that declares
/// none of this holds the default value, resolves no token, has no rate for
/// any pair, and behaves exactly as it did before this section existed.
/// Nothing downstream needs to ask whether the operator meant to deal --
/// [`DenominationConfig::declares_tokens`] is the question, and `false` is
/// every node that predates ADR 0071.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DenominationConfig {
    tokens: Vec<DeclaredToken>,
    numeraire: Option<AssetId>,
    rates: Vec<RateRow>,
    guards: Option<Guards>,
}

impl DenominationConfig {
    /// Every token this node deals, in the order declared.
    pub fn tokens(&self) -> &[DeclaredToken] {
        &self.tokens
    }

    /// Whether this node declares any token at all -- the one question a
    /// caller asks before any other, since `false` means every ADR 0071
    /// path is inert and this node forwards exactly as it did before.
    pub fn declares_tokens(&self) -> bool {
        !self.tokens.is_empty()
    }

    /// The row declaring `asset`, or `None` when this node does not deal
    /// it. The lookup a peering's token is resolved through (issue #1292),
    /// and a pure function of loaded config -- no chain read, no I/O.
    pub fn token(&self, asset: &AssetId) -> Option<&DeclaredToken> {
        self.tokens.iter().find(|token| &token.asset == asset)
    }

    /// This node's one numeraire, or `None` when it declared none. Always
    /// one of [`DenominationConfig::tokens`]: the numeraire is a token this
    /// node deals, flagged on its own row, never a fourth place to name a
    /// contract.
    pub fn numeraire(&self) -> Option<&AssetId> {
        self.numeraire.as_ref()
    }

    /// Every token with a quote path, as `(token, path)` -- what a poller
    /// walks to keep the table fresh (issue #1294), already validated
    /// against the numeraire and against this node's settlement tables, so
    /// nothing re-parses anything.
    pub fn quoted_tokens(&self) -> impl Iterator<Item = (&AssetId, &QuotePath)> {
        self.tokens
            .iter()
            .filter_map(|token| Some((&token.asset, token.quote.as_ref()?)))
    }

    /// Every `[[rates]]` row, in the order declared.
    pub fn rates(&self) -> &[RateRow] {
        &self.rates
    }

    /// The rate declared for one **ordered** pair, or `None` when the
    /// operator declared none for it. `None` is not 1:1 and never becomes
    /// it: ADR 0071 decision 2 makes an absent rate a refused forward.
    pub fn rate(&self, from: &AssetId, to: &AssetId) -> Option<Rate> {
        self.row(from, to).and_then(RateRow::rate)
    }

    /// This node's node-wide dealing policy, or `None` when it declared no
    /// rate and no quote and therefore never deals. The same
    /// `connector_domain::Guards` a rate table is built with, already
    /// validated -- there is nothing left for a caller to check or convert.
    pub fn guards(&self) -> Option<Guards> {
        self.guards
    }

    /// The three guards in force for one ordered pair: the node defaults,
    /// with this pair's own `[[rates]]` row laid over them field by field
    /// (`Guards::overridden_by`, so the rule has one implementation).
    /// `None` only when this node declared no guards at all.
    pub fn guards_for(&self, from: &AssetId, to: &AssetId) -> Option<Guards> {
        let defaults = self.guards?;
        match self.row(from, to) {
            None => Some(defaults),
            Some(row) => Some(defaults.overridden_by(&row.guards)),
        }
    }

    fn row(&self, from: &AssetId, to: &AssetId) -> Option<&RateRow> {
        self.rates
            .iter()
            .find(|row| &row.from == from && &row.to == to)
    }
}

/// Which chains this node has a `[settlement.<chain>]` table -- and
/// therefore an RPC endpoint -- for. ADR 0071 decision 3 puts a token's
/// quote on its own settlement chain precisely because that is the one
/// chain a peering already guarantees RPC for, so a quote on a chain with
/// no table is a poller that could never take its first reading.
fn settles_on(tables: SettlementTables<'_>, chain: AssetChain) -> bool {
    match SettlementChain::from(chain) {
        SettlementChain::Evm => tables.evm(),
        SettlementChain::Solana => tables.solana_program_id().is_some(),
    }
}

/// A pool address in the one spelling its chain has: lowercase `0x` hex for
/// an EVM contract, base58 for a Solana account. `None` for text that is
/// neither, which is refused by name -- a pool this node cannot address is
/// a rate it can never read, and ADR 0009 turns that into a boot refusal
/// rather than a poller logging a failure nobody reads.
fn canonical_pool(chain: AssetChain, pool: &str) -> Option<String> {
    match chain {
        AssetChain::Evm => parse_evm_address(pool).map(|address| to_hex(&address)),
        AssetChain::Solana => is_base58_32_bytes(pool).then(|| pool.to_string()),
    }
}

/// One declared fraction through its own domain constructor, with the
/// refusal wrapped so it names the row it was written on. `row` is the
/// `[rate_guards]` table, or the ordered pair whose `[[rates]]` row
/// overrode it.
fn guard<T>(
    row: &str,
    fraction: RawFraction,
    build: fn(u64, u64) -> Result<T, connector_domain::GuardError>,
) -> Result<T, ConfigError> {
    build(fraction.numerator, fraction.denominator).map_err(|source| {
        ConfigError::RateGuardInvalid {
            row: row.to_string(),
            source,
        }
    })
}

/// A `ttl_secs` as the domain's [`Ttl`]. Seconds in the file, a
/// `TimeDelta` in the type, because that is what a freshness answer is
/// computed against -- and the conversion happens here, once, rather than
/// wherever a table is built.
fn guard_ttl(row: &str, ttl_secs: u64) -> Result<Ttl, ConfigError> {
    Ttl::new(TimeDelta::seconds(
        i64::try_from(ttl_secs).unwrap_or(i64::MAX),
    ))
    .map_err(|source| ConfigError::RateGuardInvalid {
        row: row.to_string(),
        source,
    })
}

/// What one `[[rates]]` row says differently from the node defaults, every
/// value validated by the same constructor the node table's own goes
/// through.
fn resolve_guard_override(row: &str, raw: &mut RawRateRow) -> Result<GuardOverride, ConfigError> {
    Ok(GuardOverride {
        spread: raw
            .spread
            .take()
            .map(|fraction| guard(row, fraction, Spread::fraction))
            .transpose()?,
        ttl: raw.ttl_secs.map(|secs| guard_ttl(row, secs)).transpose()?,
        max_move: raw
            .max_move
            .take()
            .map(|fraction| guard(row, fraction, MaxMove::fraction))
            .transpose()?,
    })
}

/// Validate every `[[tokens]]` row (ADR 0071 decision 3), and find the
/// node's numeraire.
///
/// The numeraire is settled **first**, in its own pass, because every
/// quote-path check below is stated against it: a quote that ends at the
/// wrong token can only be named once there is a right one, and a node with
/// two numeraires has no right one at all.
fn resolve_tokens(
    raw: Vec<RawToken>,
    tables: SettlementTables<'_>,
) -> Result<(Vec<DeclaredToken>, Option<AssetId>), ConfigError> {
    let mut assets = Vec::with_capacity(raw.len());
    let mut seen = HashSet::with_capacity(raw.len());
    for token in &raw {
        let asset =
            token
                .asset
                .parse::<AssetId>()
                .map_err(|source| ConfigError::TokenAssetInvalid {
                    value: token.asset.clone(),
                    source,
                })?;
        if !seen.insert(asset.clone()) {
            return Err(ConfigError::DuplicateToken { asset });
        }
        assets.push(asset);
    }

    let mut numeraire: Option<AssetId> = None;
    for (token, asset) in raw.iter().zip(&assets) {
        if !token.numeraire {
            continue;
        }
        if let Some(first) = numeraire {
            return Err(ConfigError::MixedNumeraire {
                first,
                second: asset.clone(),
            });
        }
        numeraire = Some(asset.clone());
    }

    let mut tokens = Vec::with_capacity(raw.len());
    for (token, asset) in raw.into_iter().zip(assets) {
        // Before the path is walked, not after: a numeraire row quoting
        // into anything at all is this refusal, and walking the path first
        // would answer a numeraire quoted in WETH with a complaint about
        // where the path ends -- which is true, and not the problem.
        if token.numeraire && token.quote.is_some() {
            return Err(ConfigError::NumeraireQuoted { asset });
        }
        let quote = match token.quote {
            None => None,
            Some(legs) => Some(resolve_quote(&asset, legs, numeraire.as_ref(), tables)?),
        };
        tokens.push(DeclaredToken {
            asset,
            numeraire: token.numeraire,
            quote,
        });
    }

    Ok((tokens, numeraire))
}

/// Validate one token's quote path against the node's numeraire and its
/// settlement tables.
fn resolve_quote(
    asset: &AssetId,
    legs: Vec<RawQuoteLeg>,
    numeraire: Option<&AssetId>,
    tables: SettlementTables<'_>,
) -> Result<QuotePath, ConfigError> {
    if legs.is_empty() {
        return Err(ConfigError::TokenQuoteEmpty {
            asset: asset.clone(),
        });
    }
    if legs.len() > MAX_QUOTE_LEGS {
        return Err(ConfigError::TokenQuoteTooLong {
            asset: asset.clone(),
            legs: legs.len(),
        });
    }
    // Before every per-leg check, because it is the one refusal that is
    // about the node rather than about the row: an operator whose pools are
    // all fine but whose node has no RPC for their chain needs to read that
    // sentence, not a complaint about the first pool address.
    if !settles_on(tables, asset.chain()) {
        return Err(ConfigError::TokenQuoteWithoutSettlement {
            asset: asset.clone(),
            chain: asset.chain(),
        });
    }

    let mut resolved = Vec::with_capacity(legs.len());
    for leg in legs {
        let quote_token = leg.quote_token.parse::<AssetId>().map_err(|source| {
            ConfigError::TokenQuoteTokenInvalid {
                asset: asset.clone(),
                value: leg.quote_token.clone(),
                source,
            }
        })?;
        if quote_token.chain() != asset.chain() {
            return Err(ConfigError::TokenQuoteOffChain {
                asset: asset.clone(),
                quote_token,
            });
        }
        let pool = canonical_pool(asset.chain(), &leg.pool).ok_or_else(|| {
            ConfigError::TokenQuotePoolInvalid {
                asset: asset.clone(),
                value: leg.pool.clone(),
            }
        })?;
        if leg.twap_window_secs == 0 {
            return Err(ConfigError::TokenQuoteZeroWindow {
                asset: asset.clone(),
            });
        }
        resolved.push(QuoteLeg {
            pool,
            quote_token,
            twap_window: Duration::from_secs(leg.twap_window_secs),
        });
    }

    let Some(numeraire) = numeraire else {
        return Err(ConfigError::QuoteWithoutNumeraire {
            asset: asset.clone(),
        });
    };
    // The last leg is the one that has to land on the numeraire; the first
    // leg of a two-leg path lands wherever the operator's real venue quotes
    // (WETH, for the pair ADR 0071 names), and that token needs no
    // declaration of its own because this node never deals it.
    let ends_at = &resolved[resolved.len() - 1].quote_token;
    if ends_at != numeraire {
        return Err(ConfigError::TokenQuoteDoesNotEndAtNumeraire {
            asset: asset.clone(),
            ends_at: ends_at.clone(),
            numeraire: numeraire.clone(),
        });
    }

    Ok(QuotePath { legs: resolved })
}

/// Validate every `[[rates]]` row against the declared tokens.
fn resolve_rate_rows(
    raw: Vec<RawRateRow>,
    tokens: &[DeclaredToken],
) -> Result<Vec<RateRow>, ConfigError> {
    let declared = |asset: &AssetId| tokens.iter().any(|token| &token.asset == asset);
    let mut seen = HashSet::with_capacity(raw.len());
    let mut rows = Vec::with_capacity(raw.len());

    for row in raw {
        let from =
            row.from
                .parse::<AssetId>()
                .map_err(|source| ConfigError::RateRowAssetInvalid {
                    field: "from",
                    value: row.from.clone(),
                    source,
                })?;
        let to = row
            .to
            .parse::<AssetId>()
            .map_err(|source| ConfigError::RateRowAssetInvalid {
                field: "to",
                value: row.to.clone(),
                source,
            })?;
        if from == to {
            return Err(ConfigError::RateRowSelfPair { asset: from });
        }
        // Both sides, `from` first, so a row with two typos names the one
        // an operator reads first.
        for unknown in [&from, &to] {
            if !declared(unknown) {
                return Err(ConfigError::RateRowUnknownToken {
                    from: from.clone(),
                    to: to.clone(),
                    unknown: unknown.clone(),
                });
            }
        }
        if !seen.insert((from.clone(), to.clone())) {
            return Err(ConfigError::DuplicateRateRow { from, to });
        }

        let pair = format!("{from} -> {to}");
        let mut row = row;
        let guards = resolve_guard_override(&pair, &mut row)?;

        let rate = match row.rate {
            None => None,
            Some(fraction) => Some(Rate::new(fraction.numerator, fraction.denominator).map_err(
                |source| ConfigError::RateRowInvalid {
                    from: from.clone(),
                    to: to.clone(),
                    source,
                },
            )?),
        };

        rows.push(RateRow {
            from,
            to,
            rate,
            guards,
        });
    }

    Ok(rows)
}

/// Validate everything ADR 0071 decisions 3 and 5 let an operator declare:
/// `[[tokens]]`, `[[rates]]` and `[rate_guards]`.
///
/// All three absent is the whole of a node that does not deal, and returns
/// the default value without checking anything -- which is what "a node
/// that declares none of it behaves exactly as it does today" means at this
/// layer.
pub(crate) fn resolve_denomination(
    raw_tokens: Vec<RawToken>,
    raw_rates: Vec<RawRateRow>,
    raw_guards: Option<RawRateGuards>,
    tables: SettlementTables<'_>,
) -> Result<DenominationConfig, ConfigError> {
    if raw_tokens.is_empty() && raw_rates.is_empty() && raw_guards.is_none() {
        return Ok(DenominationConfig::default());
    }

    let (tokens, numeraire) = resolve_tokens(raw_tokens, tables)?;
    let rates = resolve_rate_rows(raw_rates, &tokens)?;

    let guards = match raw_guards {
        None => None,
        Some(declared) => {
            const ROW: &str = "[rate_guards]";
            Some(Guards::new(
                guard(ROW, declared.spread, Spread::fraction)?,
                guard_ttl(ROW, declared.ttl_secs)?,
                guard(ROW, declared.max_move, MaxMove::fraction)?,
            ))
        }
    };

    // A rate with no guards is a rate with no ttl, and a rate with no ttl
    // is one this node would deal on forever after its source died --
    // exactly the quiet draining ADR 0071 decision 5 exists to prevent. So
    // the node-wide table is required as soon as anything can produce a
    // rate, and required only then: declaring tokens alone (which is what a
    // same-asset cross-chain hop does) produces none and needs none.
    if guards.is_none() && (!rates.is_empty() || tokens.iter().any(|token| token.quote.is_some())) {
        return Err(ConfigError::RateGuardsMissing);
    }

    Ok(DenominationConfig {
        tokens,
        numeraire,
        rates,
        guards,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// USDC on Base, the numeraire throughout: 6 decimals.
    const USDC: &str = "evm:0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913";
    /// ANYONE on Base, the dealt token: 18 decimals, WETH-quoted.
    const ANYONE: &str = "evm:0x9ff58f4fFB29fA2266Ab25e75e2A8b3503311656";
    /// WETH on Base -- an intermediate, never a token this node deals.
    const WETH: &str = "evm:0x4200000000000000000000000000000000000006";
    /// USDC on Solana, for the same-asset cross-chain pair.
    const USDC_SOLANA: &str = "solana:EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
    const POOL_ANYONE_WETH: &str = "0x1111111111111111111111111111111111111111";
    const POOL_WETH_USDC: &str = "0x2222222222222222222222222222222222222222";

    /// A node with both settlement tables, which is what `local/solo`
    /// runs; the per-chain refusals below narrow it.
    fn both_chains() -> SettlementTables<'static> {
        SettlementTables::for_tests(true, Some("2aEVJ8koKD8LTZrLRSGtAtU7LBt4e7QjjCgf1kzQ7Rip"))
    }

    fn asset(text: &str) -> AssetId {
        text.parse::<AssetId>().expect("a declared asset")
    }

    fn tokens_from(text: &str) -> Vec<RawToken> {
        #[derive(Deserialize)]
        struct Doc {
            #[serde(default)]
            tokens: Vec<RawToken>,
        }
        let doc: Doc = toml::from_str(text).expect("valid TOML");
        doc.tokens
    }

    fn rates_from(text: &str) -> Vec<RawRateRow> {
        #[derive(Deserialize)]
        struct Doc {
            #[serde(default)]
            rates: Vec<RawRateRow>,
        }
        let doc: Doc = toml::from_str(text).expect("valid TOML");
        doc.rates
    }

    fn fraction(numerator: u64, denominator: u64) -> RawFraction {
        RawFraction {
            numerator,
            denominator,
        }
    }

    /// The node policy every case below deals under: 30 bps kept, a
    /// five-minute freshness window, and a 5% bound on one refresh.
    fn guards() -> Option<RawRateGuards> {
        Some(RawRateGuards {
            spread: fraction(30, 10_000),
            ttl_secs: 300,
            max_move: fraction(5, 100),
        })
    }

    /// The whole shape at once: two dealt tokens, a numeraire, a two-leg
    /// quote, a static row for the pair the quote cannot source, and node
    /// guards one row overrides.
    fn full_declaration() -> (Vec<RawToken>, Vec<RawRateRow>) {
        let tokens = tokens_from(&format!(
            r#"
[[tokens]]
asset = "{USDC}"
numeraire = true

[[tokens]]
asset = "{ANYONE}"
quote = [
  {{ pool = "{POOL_ANYONE_WETH}", quote_token = "{WETH}", twap_window_secs = 1800 }},
  {{ pool = "{POOL_WETH_USDC}", quote_token = "{USDC}", twap_window_secs = 900 }},
]

[[tokens]]
asset = "{USDC_SOLANA}"
"#
        ));
        let rates = rates_from(&format!(
            r#"
[[rates]]
from = "{USDC_SOLANA}"
to = "{ANYONE}"
rate = {{ numerator = 4000000000000, denominator = 1 }}
spread = {{ numerator = 75, denominator = 10000 }}
"#
        ));
        (tokens, rates)
    }

    #[test]
    fn a_node_that_declares_nothing_resolves_to_the_empty_declaration() {
        let resolved = resolve_denomination(vec![], vec![], None, both_chains()).expect("resolve");
        assert_eq!(resolved, DenominationConfig::default());
        assert!(!resolved.declares_tokens());
        assert_eq!(resolved.numeraire(), None);
        assert_eq!(resolved.guards(), None);
        assert_eq!(resolved.rate(&asset(USDC), &asset(ANYONE)), None);
    }

    #[test]
    fn a_full_declaration_is_reachable_through_typed_accessors() {
        let (tokens, rates) = full_declaration();
        let resolved =
            resolve_denomination(tokens, rates, guards(), both_chains()).expect("resolve");

        assert!(resolved.declares_tokens());
        assert_eq!(resolved.numeraire(), Some(&asset(USDC)));
        assert_eq!(resolved.tokens().len(), 3);
        assert!(resolved.token(&asset(ANYONE)).is_some());
        assert!(resolved.token(&asset(WETH)).is_none());

        let quoted: Vec<_> = resolved.quoted_tokens().collect();
        assert_eq!(quoted.len(), 1);
        let (token, path) = quoted[0];
        assert_eq!(token, &asset(ANYONE));
        assert_eq!(path.chain(), AssetChain::Evm);
        assert_eq!(path.legs().len(), 2);
        assert_eq!(path.legs()[0].quote_token(), &asset(WETH));
        assert_eq!(path.legs()[0].twap_window(), Duration::from_secs(1800));
        // Canonicalized the same way every other address in this crate is.
        assert_eq!(path.legs()[0].pool(), POOL_ANYONE_WETH);
        assert_eq!(path.legs()[1].quote_token(), &asset(USDC));

        assert_eq!(
            resolved.rate(&asset(USDC_SOLANA), &asset(ANYONE)),
            Some(Rate::new(4_000_000_000_000, 1).expect("a rate"))
        );
        // Direction is the trade: the reverse pair is a different row, and
        // this config does not declare it.
        assert_eq!(resolved.rate(&asset(ANYONE), &asset(USDC_SOLANA)), None);
    }

    #[test]
    fn a_per_pair_guard_overrides_the_node_default() {
        let (tokens, rates) = full_declaration();
        let resolved =
            resolve_denomination(tokens, rates, guards(), both_chains()).expect("resolve");

        let overridden = resolved
            .guards_for(&asset(USDC_SOLANA), &asset(ANYONE))
            .expect("the node declares guards");
        assert_eq!(
            overridden.spread(),
            Spread::fraction(75, 10_000).expect("a spread below one")
        );
        // Untouched by the row, so still the node default -- an operator
        // tightening one pair's spread has not dropped the node's ttl.
        assert_eq!(
            overridden.ttl(),
            Ttl::new(TimeDelta::seconds(300)).expect("a positive span")
        );
        assert_eq!(
            overridden.max_move(),
            MaxMove::fraction(5, 100).expect("a fraction with a whole")
        );

        let defaults = resolved
            .guards_for(&asset(USDC), &asset(ANYONE))
            .expect("a pair with no row of its own still has the node defaults");
        assert_eq!(
            defaults.spread(),
            Spread::fraction(30, 10_000).expect("a spread below one")
        );
    }

    #[test]
    fn two_numeraires_are_refused_by_name() {
        let tokens = tokens_from(&format!(
            r#"
[[tokens]]
asset = "{USDC}"
numeraire = true

[[tokens]]
asset = "{ANYONE}"
numeraire = true
"#
        ));
        let error = resolve_denomination(tokens, vec![], guards(), both_chains())
            .expect_err("a node has one numeraire");
        assert!(
            matches!(&error, ConfigError::MixedNumeraire { first, second }
                if first == &asset(USDC) && second == &asset(ANYONE)),
            "got: {error}"
        );
    }

    #[test]
    fn a_zero_denominator_is_refused_and_names_the_row() {
        let rates = rates_from(&format!(
            r#"
[[rates]]
from = "{USDC}"
to = "{ANYONE}"
rate = {{ numerator = 3, denominator = 0 }}
"#
        ));
        let (tokens, _) = full_declaration();
        let error = resolve_denomination(tokens, rates, guards(), both_chains())
            .expect_err("there is no rational over zero");
        assert!(
            matches!(&error, ConfigError::RateRowInvalid { from, to, .. }
                if from == &asset(USDC) && to == &asset(ANYONE)),
            "got: {error}"
        );
        // The domain's own words, unmodified -- this layer names the row
        // and reimplements nothing.
        assert!(error.to_string().contains("denominator"), "got: {error}");
    }

    #[test]
    fn a_rate_row_naming_an_undeclared_token_is_refused_by_name() {
        let rates = rates_from(&format!(
            r#"
[[rates]]
from = "{USDC}"
to = "{WETH}"
rate = {{ numerator = 1, denominator = 2 }}
"#
        ));
        let (tokens, _) = full_declaration();
        let error = resolve_denomination(tokens, rates, guards(), both_chains())
            .expect_err("a pair is between two tokens this node deals");
        assert!(
            matches!(&error, ConfigError::RateRowUnknownToken { unknown, .. }
                if unknown == &asset(WETH)),
            "got: {error}"
        );
    }

    #[test]
    fn a_quote_that_does_not_end_at_the_numeraire_is_refused_by_name() {
        let tokens = tokens_from(&format!(
            r#"
[[tokens]]
asset = "{USDC}"
numeraire = true

[[tokens]]
asset = "{ANYONE}"
quote = [
  {{ pool = "{POOL_ANYONE_WETH}", quote_token = "{WETH}", twap_window_secs = 1800 }},
]
"#
        ));
        let error = resolve_denomination(tokens, vec![], guards(), both_chains())
            .expect_err("a quote path ends at the numeraire");
        assert!(
            matches!(&error, ConfigError::TokenQuoteDoesNotEndAtNumeraire { asset: a, ends_at, numeraire }
                if a == &asset(ANYONE) && ends_at == &asset(WETH) && numeraire == &asset(USDC)),
            "got: {error}"
        );
    }

    #[test]
    fn a_quote_on_a_chain_with_no_settlement_table_is_refused_by_name() {
        let tokens = tokens_from(&format!(
            r#"
[[tokens]]
asset = "{USDC}"
numeraire = true

[[tokens]]
asset = "{ANYONE}"
quote = [
  {{ pool = "{POOL_WETH_USDC}", quote_token = "{USDC}", twap_window_secs = 1800 }},
]
"#
        ));
        // A Solana-only node: it has RPC for no EVM chain, so it can never
        // take this reading.
        let solana_only = SettlementTables::for_tests(
            false,
            Some("2aEVJ8koKD8LTZrLRSGtAtU7LBt4e7QjjCgf1kzQ7Rip"),
        );
        let error = resolve_denomination(tokens, vec![], guards(), solana_only)
            .expect_err("a quote needs RPC for its own chain");
        assert!(
            matches!(&error, ConfigError::TokenQuoteWithoutSettlement { asset: a, chain }
                if a == &asset(ANYONE) && *chain == AssetChain::Evm),
            "got: {error}"
        );
    }

    #[test]
    fn a_quote_with_no_numeraire_declared_is_refused() {
        let tokens = tokens_from(&format!(
            r#"
[[tokens]]
asset = "{ANYONE}"
quote = [
  {{ pool = "{POOL_WETH_USDC}", quote_token = "{USDC}", twap_window_secs = 1800 }},
]
"#
        ));
        let error = resolve_denomination(tokens, vec![], guards(), both_chains())
            .expect_err("a quote path has nowhere to end");
        assert!(
            matches!(&error, ConfigError::QuoteWithoutNumeraire { asset: a } if a == &asset(ANYONE)),
            "got: {error}"
        );
    }

    #[test]
    fn a_third_quote_leg_is_refused() {
        let tokens = tokens_from(&format!(
            r#"
[[tokens]]
asset = "{USDC}"
numeraire = true

[[tokens]]
asset = "{ANYONE}"
quote = [
  {{ pool = "{POOL_ANYONE_WETH}", quote_token = "{WETH}", twap_window_secs = 60 }},
  {{ pool = "{POOL_ANYONE_WETH}", quote_token = "{WETH}", twap_window_secs = 60 }},
  {{ pool = "{POOL_WETH_USDC}", quote_token = "{USDC}", twap_window_secs = 60 }},
]
"#
        ));
        let error = resolve_denomination(tokens, vec![], guards(), both_chains())
            .expect_err("one or two pools, never three");
        assert!(
            matches!(&error, ConfigError::TokenQuoteTooLong { legs, .. } if *legs == 3),
            "got: {error}"
        );
    }

    #[test]
    fn a_quote_leg_on_another_chain_is_refused() {
        let tokens = tokens_from(&format!(
            r#"
[[tokens]]
asset = "{USDC}"
numeraire = true

[[tokens]]
asset = "{ANYONE}"
quote = [
  {{ pool = "{POOL_ANYONE_WETH}", quote_token = "{USDC_SOLANA}", twap_window_secs = 60 }},
]
"#
        ));
        let error = resolve_denomination(tokens, vec![], guards(), both_chains())
            .expect_err("a quote path stays on the token's own chain");
        assert!(
            matches!(&error, ConfigError::TokenQuoteOffChain { .. }),
            "got: {error}"
        );
    }

    #[test]
    fn a_zero_twap_window_is_refused() {
        let tokens = tokens_from(&format!(
            r#"
[[tokens]]
asset = "{USDC}"
numeraire = true

[[tokens]]
asset = "{ANYONE}"
quote = [
  {{ pool = "{POOL_WETH_USDC}", quote_token = "{USDC}", twap_window_secs = 0 }},
]
"#
        ));
        let error = resolve_denomination(tokens, vec![], guards(), both_chains())
            .expect_err("a window of nothing is a spot read");
        assert!(
            matches!(&error, ConfigError::TokenQuoteZeroWindow { .. }),
            "got: {error}"
        );
    }

    #[test]
    fn a_pool_that_is_not_an_address_on_its_chain_is_refused() {
        let tokens = tokens_from(&format!(
            r#"
[[tokens]]
asset = "{USDC}"
numeraire = true

[[tokens]]
asset = "{ANYONE}"
quote = [
  {{ pool = "the-anyone-weth-pool", quote_token = "{USDC}", twap_window_secs = 60 }},
]
"#
        ));
        let error = resolve_denomination(tokens, vec![], guards(), both_chains())
            .expect_err("a pool this node cannot address is a rate it cannot read");
        assert!(
            matches!(&error, ConfigError::TokenQuotePoolInvalid { value, .. }
                if value == "the-anyone-weth-pool"),
            "got: {error}"
        );
    }

    #[test]
    fn the_numeraire_may_not_quote_itself() {
        let tokens = tokens_from(&format!(
            r#"
[[tokens]]
asset = "{USDC}"
numeraire = true
quote = [
  {{ pool = "{POOL_WETH_USDC}", quote_token = "{USDC}", twap_window_secs = 60 }},
]
"#
        ));
        let error = resolve_denomination(tokens, vec![], guards(), both_chains())
            .expect_err("the numeraire is what everything else is quoted in");
        assert!(
            matches!(&error, ConfigError::NumeraireQuoted { .. }),
            "got: {error}"
        );
    }

    #[test]
    fn a_duplicate_token_row_is_refused() {
        let lowercase = USDC.to_ascii_lowercase();
        let tokens = tokens_from(&format!(
            r#"
[[tokens]]
asset = "{USDC}"

[[tokens]]
asset = "{lowercase}"
"#
        ));
        let error = resolve_denomination(tokens, vec![], None, both_chains())
            .expect_err("one token, one row");
        // The checksummed spelling and the lowercase one are one token --
        // which is the whole reason `AssetId` canonicalizes.
        assert!(
            matches!(&error, ConfigError::DuplicateToken { asset: declared } if declared == &asset(USDC)),
            "got: {error}"
        );
    }

    #[test]
    fn a_pair_with_itself_is_refused() {
        let rates = rates_from(&format!(
            r#"
[[rates]]
from = "{USDC}"
to = "{USDC}"
rate = {{ numerator = 1, denominator = 1 }}
"#
        ));
        let (tokens, _) = full_declaration();
        let error = resolve_denomination(tokens, rates, guards(), both_chains())
            .expect_err("a token is not a denomination boundary with itself");
        assert!(
            matches!(&error, ConfigError::RateRowSelfPair { .. }),
            "got: {error}"
        );
    }

    #[test]
    fn a_duplicate_ordered_pair_is_refused() {
        let rates = rates_from(&format!(
            r#"
[[rates]]
from = "{USDC}"
to = "{ANYONE}"
rate = {{ numerator = 4000000000000, denominator = 1 }}

[[rates]]
from = "{USDC}"
to = "{ANYONE}"
rate = {{ numerator = 5000000000000, denominator = 1 }}
"#
        ));
        let (tokens, _) = full_declaration();
        let error = resolve_denomination(tokens, rates, guards(), both_chains())
            .expect_err("one ordered pair, one row");
        assert!(
            matches!(&error, ConfigError::DuplicateRateRow { .. }),
            "got: {error}"
        );
    }

    #[test]
    fn the_reverse_pair_is_a_different_row_and_loads() {
        let rates = rates_from(&format!(
            r#"
[[rates]]
from = "{USDC}"
to = "{ANYONE}"
rate = {{ numerator = 4000000000000, denominator = 1 }}

[[rates]]
from = "{ANYONE}"
to = "{USDC}"
rate = {{ numerator = 1, denominator = 4200000000000 }}
"#
        ));
        let (tokens, _) = full_declaration();
        let resolved =
            resolve_denomination(tokens, rates, guards(), both_chains()).expect("resolve");
        assert_ne!(
            resolved.rate(&asset(USDC), &asset(ANYONE)),
            resolved
                .rate(&asset(ANYONE), &asset(USDC))
                .map(|reverse| reverse.inverted())
        );
    }

    #[test]
    fn a_spread_of_the_whole_mid_is_refused_in_the_domains_own_words() {
        let error = resolve_denomination(
            vec![],
            vec![],
            Some(RawRateGuards {
                spread: fraction(1, 1),
                ttl_secs: 300,
                max_move: fraction(5, 100),
            }),
            both_chains(),
        )
        .expect_err("a pair dealt at its own mid forwards nothing");
        assert!(
            matches!(&error, ConfigError::RateGuardInvalid { row, .. } if row == "[rate_guards]"),
            "got: {error}"
        );
        assert!(error.to_string().contains("spread"), "got: {error}");
    }

    #[test]
    fn a_zero_ttl_is_refused() {
        let error = resolve_denomination(
            vec![],
            vec![],
            Some(RawRateGuards {
                spread: fraction(30, 10_000),
                ttl_secs: 0,
                max_move: fraction(5, 100),
            }),
            both_chains(),
        )
        .expect_err("a rate that is dead on arrival is not a rate");
        assert!(
            matches!(&error, ConfigError::RateGuardInvalid { .. }),
            "got: {error}"
        );
        assert!(error.to_string().contains("ttl"), "got: {error}");
    }

    /// The domain calls a zero `max_move` *pinned* -- only the same value is
    /// ever accepted, which is a coherent thing to declare about a par pair
    /// -- so config must not refuse it. This is the case where a config-side
    /// re-implementation of the guard rules would have disagreed with the
    /// rules themselves.
    #[test]
    fn a_pinned_max_move_is_a_declaration_rather_than_a_mistake() {
        let resolved = resolve_denomination(
            vec![],
            vec![],
            Some(RawRateGuards {
                spread: fraction(0, 1),
                ttl_secs: 300,
                max_move: fraction(0, 1),
            }),
            both_chains(),
        )
        .expect("a pinned pair dealt at mid is a policy, not a typo");
        let guards = resolved.guards().expect("declared");
        assert!(guards.spread().is_none());
        assert_eq!(guards.max_move().numerator(), 0);
    }

    #[test]
    fn a_per_pair_guard_is_checked_the_same_way_the_node_default_is() {
        let rates = rates_from(&format!(
            r#"
[[rates]]
from = "{USDC}"
to = "{ANYONE}"
ttl_secs = 0
"#
        ));
        let (tokens, _) = full_declaration();
        let error = resolve_denomination(tokens, rates, guards(), both_chains())
            .expect_err("an override is a guard too");
        assert!(
            matches!(&error, ConfigError::RateGuardInvalid { row, .. } if row.contains("->")),
            "got: {error}"
        );
    }

    #[test]
    fn a_declared_rate_without_node_guards_is_refused() {
        let (tokens, rates) = full_declaration();
        let error = resolve_denomination(tokens, rates, None, both_chains())
            .expect_err("a rate with no ttl is one this node deals on forever");
        assert!(
            matches!(&error, ConfigError::RateGuardsMissing),
            "got: {error}"
        );
    }

    /// The same-asset cross-chain hop `local/mixed-chain` runs: it declares
    /// the tokens its peerings hold so a later ticket can resolve them, and
    /// it declares no rate at all, because crossing a chain is not a
    /// conversion (ADR 0071 decision 2).
    #[test]
    fn declaring_tokens_alone_needs_no_guards() {
        let tokens = tokens_from(&format!(
            r#"
[[tokens]]
asset = "{USDC}"

[[tokens]]
asset = "{USDC_SOLANA}"
"#
        ));
        let resolved = resolve_denomination(tokens, vec![], None, both_chains()).expect("resolve");
        assert!(resolved.declares_tokens());
        assert_eq!(resolved.guards(), None);
        assert_eq!(resolved.guards_for(&asset(USDC), &asset(USDC_SOLANA)), None);
    }

    #[test]
    fn a_mistyped_key_inside_a_token_row_is_refused_by_name() {
        let error = toml::from_str::<RawToken>(&format!(
            r#"
asset = "{USDC}"
numeraire_token = true
"#
        ))
        .expect_err("an unknown key is refused");
        assert!(
            error.to_string().contains("numeraire_token"),
            "got: {error}"
        );
    }

    #[test]
    fn a_mistyped_key_inside_a_rate_row_is_refused_by_name() {
        let error = toml::from_str::<RawRateRow>(&format!(
            r#"
from = "{USDC}"
to = "{ANYONE}"
spread_bps = 30
"#
        ))
        .expect_err("an unknown key is refused");
        assert!(error.to_string().contains("spread_bps"), "got: {error}");
    }

    #[test]
    fn a_mistyped_key_inside_the_guards_table_is_refused_by_name() {
        let error = toml::from_str::<RawRateGuards>(
            r#"
spread = { numerator = 30, denominator = 10000 }
ttl_seconds = 300
max_move = { numerator = 5, denominator = 100 }
"#,
        )
        .expect_err("an unknown key is refused");
        assert!(error.to_string().contains("ttl_seconds"), "got: {error}");
    }

    #[test]
    fn a_mistyped_key_inside_a_quote_leg_is_refused_by_name() {
        let error = toml::from_str::<RawQuoteLeg>(&format!(
            r#"
pool = "{POOL_WETH_USDC}"
quote_token = "{USDC}"
window_secs = 60
"#
        ))
        .expect_err("an unknown key is refused");
        assert!(error.to_string().contains("window_secs"), "got: {error}");
    }

    #[test]
    fn an_asset_naming_a_chain_this_connector_has_no_backend_for_is_refused() {
        let tokens = tokens_from(
            r#"
[[tokens]]
asset = "mina:B62qFoo"
"#,
        );
        let error = resolve_denomination(tokens, vec![], None, both_chains())
            .expect_err("mina is gone from this repository");
        assert!(
            matches!(&error, ConfigError::TokenAssetInvalid { .. }),
            "got: {error}"
        );
    }
}
