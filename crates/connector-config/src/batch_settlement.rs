//! The x402 `batch-settlement` terms of `[settlement.evm]` and
//! `[settlement.solana]`: on what terms this node accepts, and opens, x402
//! channels on that chain (ADR 0074 decisions 1 and 5, ADR 0075 decision 9).
//!
//! **Always on.** Every channel is an x402 channel (ADR 0075), so every
//! settlement table carries these terms. They used to be an opt-in
//! `[settlement.<chain>.batch_settlement]` sub-table; its keys now sit in the
//! settlement table itself, keeping their bounds, and the sub-table is
//! refused by name (`crate::settlement`).
//!
//! **What is not here.** Everything ADR 0074 decision 2 _fixes_ about an
//! admissible channel is read from the enclosing settlement table and never
//! declared again (CF-26): the receiver is that table's settlement key, and
//! the token or mint is its `token_address`. Nor is where the channels live:
//! the record fixes `x402BatchSettlement` at one address and
//! `payment-channels` at one program id (its _Sources_, and decision 4's EVM
//! domain), so each is a single constant in the crate that binds to it --
//! `connector_signer::X402_BATCH_SETTLEMENT_ADDRESS` and
//! `connector_settlement_solana::batch::wire::PAYMENT_CHANNELS_PROGRAM_ID`
//! -- and no key here can name another. These terms are only the ones that
//! are this node's to choose.

#[cfg(test)]
use serde::Deserialize;

use connector_domain::x402::X402AssetTransferMethod;

use crate::error::ConfigError;

/// The floor under both published minimums, in seconds: x402's own 900
/// (ADR 0074 decision 5). On EVM it is the contract's `MIN_WITHDRAW_DELAY`,
/// fifteen minutes; on Solana the program's only bound is one second, and
/// 900 is the x402 SVM scheme's.
pub const BATCH_SETTLEMENT_DELAY_FLOOR_SECS: u64 = 900;

/// The default for both published minimums: one day (ADR 0074 decision 5).
/// It is the window a censored or delayed `claim` or `settle_and_seal` still
/// has to land in.
pub const DEFAULT_BATCH_SETTLEMENT_MIN_DELAY_SECS: u64 = 86_400;

/// `x402BatchSettlement`'s `MAX_WITHDRAW_DELAY`, thirty days. The contract
/// refuses a deposit into a channel whose `withdrawDelay` exceeds it, so a
/// published minimum above it would admit no channel at all.
pub const EVM_BATCH_SETTLEMENT_MAX_WITHDRAW_DELAY_SECS: u64 = 30 * 86_400;

/// The EVM terms as `[settlement.evm]` wrote them. `asset_eip712_name` and
/// `asset_eip712_version` are required (issue #1345, ADR 0075 decision 9);
/// an omitted `min_withdraw_delay_secs` takes the default the record chose,
/// an omitted `asset_transfer_method` is x402's own `eip3009`, and an
/// omitted `facilitator_url` names none (toon-client#695).
#[derive(Debug)]
#[cfg_attr(test, derive(Deserialize))]
pub(crate) struct RawEvmBatchSettlementTable {
    #[cfg_attr(test, serde(default))]
    pub(crate) min_withdraw_delay_secs: Option<u64>,
    pub(crate) asset_eip712_name: String,
    pub(crate) asset_eip712_version: String,
    #[cfg_attr(test, serde(default))]
    pub(crate) asset_transfer_method: Option<String>,
    #[cfg_attr(test, serde(default))]
    pub(crate) facilitator_url: Option<String>,
}

/// The Solana terms as `[settlement.solana]` wrote them.
/// `min_sponsored_deposit` is required: it bounds a public endpoint that
/// spends this node's lamports (ADR 0074 decision 9), and no one number is a
/// safe default for every mint.
#[derive(Debug)]
#[cfg_attr(test, derive(Deserialize))]
pub(crate) struct RawSolanaBatchSettlementTable {
    #[cfg_attr(test, serde(default))]
    pub(crate) min_grace_period_secs: Option<u64>,
    pub(crate) min_sponsored_deposit: u64,
}

/// This node's validated x402 `batch-settlement` terms on EVM (ADR 0074,
/// ADR 0075 decision 9).
///
/// It names only the terms that are this node's to choose. `receiver` and
/// `receiverAuthorizer` must both be the enclosing `[settlement.evm]` table's
/// settlement address, and `token` its `token_address`; neither is declared
/// here a second time (CF-26).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvmBatchSettlementConfig {
    min_withdraw_delay_secs: u64,
    asset_eip712_name: String,
    asset_eip712_version: String,
    asset_transfer_method: X402AssetTransferMethod,
    facilitator_url: Option<String>,
}

impl EvmBatchSettlementConfig {
    /// The shortest `withdrawDelay` a channel may carry and still be
    /// admitted, in seconds, and the figure the greeting publishes (ADR 0074
    /// decision 8). Between 900 and thirty days inclusive; one day unless
    /// the table says otherwise.
    pub fn min_withdraw_delay_secs(&self) -> u64 {
        self.min_withdraw_delay_secs
    }

    /// The EIP-712 domain `name` of the asset this node accepts a deposit
    /// in (`[settlement.evm] token_address`) -- `"USDC"` for the devnet's
    /// Circle FiatToken v2.2, and the figure the greeting's
    /// `accepts[].extra.name` publishes (issue #1345). Configured, not read
    /// off the chain: see this table's own module doc for why.
    pub fn asset_eip712_name(&self) -> &str {
        &self.asset_eip712_name
    }

    /// The EIP-712 domain `version` of the same asset -- `"2"` for the
    /// devnet's Circle FiatToken v2.2, published as `accepts[].extra.version`.
    pub fn asset_eip712_version(&self) -> &str {
        &self.asset_eip712_version
    }

    /// How a payer's deposit moves the asset -- ERC-3009 or a Permit2
    /// witness transfer -- published as x402's own
    /// `accepts[].extra.assetTransferMethod`, always, even at its default
    /// `eip3009` (toon-client#695). Only the spelling is judged here; boot
    /// refuses `eip3009` on a token without ERC-3009, and under it an
    /// asset EIP-712 domain the token disagrees with (issue #1422), since
    /// that needs the chain.
    pub fn asset_transfer_method(&self) -> X402AssetTransferMethod {
        self.asset_transfer_method
    }

    /// The x402 facilitator this operator relays deposits through and pays
    /// the gas of, published as `accepts[].extra.facilitator` verbatim when
    /// set (toon-client#695). An `http`/`https` URL. This node never calls
    /// it: a payer does, because its deposit precedes the channel.
    pub fn facilitator_url(&self) -> Option<&str> {
        self.facilitator_url.as_deref()
    }
}

/// This node's validated x402 `batch-settlement` terms on Solana: it accepts,
/// and sponsors the opening of, x402 channels (ADR 0074, ADR 0075).
///
/// As on EVM, what the record fixes comes from the enclosing
/// `[settlement.solana]` table: the sponsor key (`payee` and `rent_payer`) is
/// its settlement key, and `mint` must be its `token_address`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SolanaBatchSettlementConfig {
    min_grace_period_secs: u64,
    min_sponsored_deposit: u64,
}

impl SolanaBatchSettlementConfig {
    /// The shortest `grace_period` a channel may carry and still be
    /// admitted, in seconds, and the figure the greeting publishes. At least
    /// 900; one day unless the table says otherwise.
    pub fn min_grace_period_secs(&self) -> u64 {
        self.min_grace_period_secs
    }

    /// The smallest opening deposit, in the mint's base units, this node
    /// will co-sign an `open` for as sponsor (ADR 0074 decision 5). Never
    /// zero.
    pub fn min_sponsored_deposit(&self) -> u64 {
        self.min_sponsored_deposit
    }
}

fn delay_at_or_above_floor(
    table: &'static str,
    key: &'static str,
    value: u64,
) -> Result<u64, ConfigError> {
    if value < BATCH_SETTLEMENT_DELAY_FLOOR_SECS {
        return Err(ConfigError::BatchSettlementDelayBelowFloor { table, key, value });
    }
    Ok(value)
}

pub(crate) fn resolve_evm_batch_settlement(
    raw: RawEvmBatchSettlementTable,
) -> Result<EvmBatchSettlementConfig, ConfigError> {
    let min_withdraw_delay_secs = delay_at_or_above_floor(
        "evm",
        "min_withdraw_delay_secs",
        raw.min_withdraw_delay_secs
            .unwrap_or(DEFAULT_BATCH_SETTLEMENT_MIN_DELAY_SECS),
    )?;
    if min_withdraw_delay_secs > EVM_BATCH_SETTLEMENT_MAX_WITHDRAW_DELAY_SECS {
        return Err(
            ConfigError::BatchSettlementWithdrawDelayAboveContractMaximum {
                value: min_withdraw_delay_secs,
            },
        );
    }
    if raw.asset_eip712_name.trim().is_empty() {
        return Err(ConfigError::BatchSettlementEmptyAssetEip712Field {
            key: "asset_eip712_name",
        });
    }
    if raw.asset_eip712_version.trim().is_empty() {
        return Err(ConfigError::BatchSettlementEmptyAssetEip712Field {
            key: "asset_eip712_version",
        });
    }
    let asset_transfer_method = match raw.asset_transfer_method {
        None => X402AssetTransferMethod::default(),
        Some(value) => X402AssetTransferMethod::from_name(&value)
            .ok_or(ConfigError::BatchSettlementUnknownAssetTransferMethod { value })?,
    };
    let facilitator_url = raw.facilitator_url.map(facilitator_url).transpose()?;
    Ok(EvmBatchSettlementConfig {
        min_withdraw_delay_secs,
        asset_eip712_name: raw.asset_eip712_name,
        asset_eip712_version: raw.asset_eip712_version,
        asset_transfer_method,
        facilitator_url,
    })
}

/// `facilitator_url` must be an absolute `http`/`https` URL: a payer POSTs
/// its deposit there. Kept verbatim -- the greeting publishes exactly what
/// the operator wrote, as it does `asset_eip712_name`.
fn facilitator_url(value: String) -> Result<String, ConfigError> {
    let url = match url::Url::parse(&value) {
        Ok(url) => url,
        Err(source) => {
            return Err(ConfigError::BatchSettlementInvalidFacilitatorUrl { value, source })
        }
    };
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(ConfigError::BatchSettlementUnsupportedFacilitatorScheme { value });
    }
    Ok(value)
}

pub(crate) fn resolve_solana_batch_settlement(
    raw: RawSolanaBatchSettlementTable,
) -> Result<SolanaBatchSettlementConfig, ConfigError> {
    let min_grace_period_secs = delay_at_or_above_floor(
        "solana",
        "min_grace_period_secs",
        raw.min_grace_period_secs
            .unwrap_or(DEFAULT_BATCH_SETTLEMENT_MIN_DELAY_SECS),
    )?;
    if raw.min_sponsored_deposit == 0 {
        return Err(ConfigError::BatchSettlementZeroMinimumSponsoredDeposit);
    }
    Ok(SolanaBatchSettlementConfig {
        min_grace_period_secs,
        min_sponsored_deposit: raw.min_sponsored_deposit,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `asset_eip712_name`/`asset_eip712_version` are required as soon as
    /// the table exists (issue #1345), so every test below that is not
    /// itself about those two keys writes a table missing only what it is
    /// testing -- the two are supplied here once rather than in every
    /// fixture.
    fn evm(text: &str) -> Result<EvmBatchSettlementConfig, ConfigError> {
        let text = format!("asset_eip712_name = \"USDC\"\nasset_eip712_version = \"2\"\n{text}");
        resolve_evm_batch_settlement(toml::from_str(&text).expect("valid TOML"))
    }

    fn solana(text: &str) -> Result<SolanaBatchSettlementConfig, ConfigError> {
        resolve_solana_batch_settlement(toml::from_str(text).expect("valid TOML"))
    }

    const ONE_DAY: u64 = 86_400;

    // -- EVM --

    /// Writing the table is the opt-in, and everything but the required
    /// EIP-712 domain fields has a default the record chose: one day.
    #[test]
    fn an_empty_evm_table_takes_the_recorded_defaults() {
        let config = evm("").expect("the two required keys alone are a complete opt-in");

        assert_eq!(config.min_withdraw_delay_secs(), ONE_DAY);
    }

    #[test]
    fn an_evm_minimum_at_the_floor_is_accepted() {
        let config = evm("min_withdraw_delay_secs = 900").expect("the floor itself is legal");
        assert_eq!(config.min_withdraw_delay_secs(), 900);
    }

    /// Decision 5: the connector's minimum may be no lower than the
    /// contract's own fifteen minutes, and the refusal says which key and
    /// which floor.
    #[test]
    fn an_evm_minimum_below_the_floor_is_refused_by_name() {
        let error = evm("min_withdraw_delay_secs = 899").expect_err("below the floor");

        assert!(matches!(
            error,
            ConfigError::BatchSettlementDelayBelowFloor {
                table: "evm",
                key: "min_withdraw_delay_secs",
                value: 899,
            }
        ));
        let message = error.to_string();
        assert!(
            message.contains("min_withdraw_delay_secs") && message.contains("900"),
            "the message must name the key and the floor, got: {message}"
        );
    }

    /// The contract refuses a `withdrawDelay` over thirty days at deposit,
    /// so a minimum above that admits no channel at all -- an opt-in that
    /// can never be used is refused rather than loaded.
    #[test]
    fn an_evm_minimum_no_channel_can_meet_is_refused_by_name() {
        let error = evm("min_withdraw_delay_secs = 2592001").expect_err("above 30 days");
        assert!(matches!(
            error,
            ConfigError::BatchSettlementWithdrawDelayAboveContractMaximum { value: 2_592_001 }
        ));

        evm("min_withdraw_delay_secs = 2592000").expect("thirty days exactly is the maximum");
    }

    /// A table that writes the two keys empty is refused by this module's
    /// own check, not merely accepted with a useless value published.
    #[test]
    fn an_evm_table_with_an_empty_eip712_domain_field_is_refused_by_name() {
        let error = resolve_evm_batch_settlement(
            toml::from_str("asset_eip712_name = \"\"\nasset_eip712_version = \"2\"")
                .expect("valid TOML"),
        )
        .expect_err("an empty name publishes a useless domain");
        assert!(matches!(
            error,
            ConfigError::BatchSettlementEmptyAssetEip712Field {
                key: "asset_eip712_name"
            }
        ));

        let error = resolve_evm_batch_settlement(
            toml::from_str("asset_eip712_name = \"USDC\"\nasset_eip712_version = \"\"")
                .expect("valid TOML"),
        )
        .expect_err("an empty version publishes a useless domain");
        assert!(matches!(
            error,
            ConfigError::BatchSettlementEmptyAssetEip712Field {
                key: "asset_eip712_version"
            }
        ));
    }

    /// The two values ride through verbatim -- the greeting publishes
    /// exactly what the operator wrote, never a normalized or defaulted
    /// spelling.
    #[test]
    fn the_eip712_domain_fields_are_read_back_verbatim() {
        let config = evm("").expect("the helper's own USDC/2 fixture");
        assert_eq!(config.asset_eip712_name(), "USDC");
        assert_eq!(config.asset_eip712_version(), "2");
    }

    // -- EVM deposit terms (toon-client#695) --

    /// An operator who writes neither key deposits by ERC-3009 -- x402's
    /// own default -- and names no facilitator.
    #[test]
    fn an_evm_table_without_deposit_keys_takes_eip3009_and_no_facilitator() {
        let config = evm("").expect("both keys are optional");
        assert_eq!(
            config.asset_transfer_method(),
            X402AssetTransferMethod::Eip3009
        );
        assert_eq!(config.facilitator_url(), None);
    }

    #[test]
    fn both_x402_asset_transfer_methods_are_accepted() {
        let config = evm("asset_transfer_method = \"eip3009\"").expect("x402's default");
        assert_eq!(
            config.asset_transfer_method(),
            X402AssetTransferMethod::Eip3009
        );
        let config = evm("asset_transfer_method = \"permit2\"").expect("x402's other method");
        assert_eq!(
            config.asset_transfer_method(),
            X402AssetTransferMethod::Permit2
        );
    }

    /// Only x402's two spellings: anything else would publish a method no
    /// client can deposit by, so it is refused by name, never defaulted.
    #[test]
    fn an_unknown_asset_transfer_method_is_refused_by_name() {
        for value in ["eip2612", "Permit2", "EIP3009", ""] {
            let error = evm(&format!("asset_transfer_method = \"{value}\""))
                .expect_err("not an x402 asset transfer method");
            assert!(
                matches!(
                    &error,
                    ConfigError::BatchSettlementUnknownAssetTransferMethod { value: written }
                        if written == value
                ),
                "got {error:?}"
            );
            let message = error.to_string();
            assert!(
                message.contains("asset_transfer_method")
                    && message.contains("eip3009")
                    && message.contains("permit2"),
                "the message must name the key and both legal values, got: {message}"
            );
        }
    }

    /// The facilitator URL is published verbatim, as the operator wrote it.
    #[test]
    fn an_http_or_https_facilitator_url_is_accepted_verbatim() {
        let config =
            evm("facilitator_url = \"https://facilitator.example/x402\"").expect("an https URL");
        assert_eq!(
            config.facilitator_url(),
            Some("https://facilitator.example/x402")
        );
        let config = evm("facilitator_url = \"http://127.0.0.1:4022\"").expect("a local http URL");
        assert_eq!(config.facilitator_url(), Some("http://127.0.0.1:4022"));
    }

    /// A value that is not a URL at all, or is written empty, is refused
    /// by name rather than published for a payer to fail against.
    #[test]
    fn an_unparseable_facilitator_url_is_refused_by_name() {
        for value in ["", "not a url", "facilitator.example/x402"] {
            let error = evm(&format!("facilitator_url = \"{value}\"")).expect_err("not a URL");
            assert!(
                matches!(
                    &error,
                    ConfigError::BatchSettlementInvalidFacilitatorUrl { value: written, .. }
                        if written == value
                ),
                "got {error:?}"
            );
            assert!(error.to_string().contains("facilitator_url"));
        }
    }

    /// A payer calls it over HTTP(S); any other scheme is refused by name.
    #[test]
    fn a_non_http_facilitator_url_is_refused_by_name() {
        for value in ["ws://facilitator.example", "ftp://facilitator.example/x402"] {
            let error = evm(&format!("facilitator_url = \"{value}\"")).expect_err("not http(s)");
            assert!(
                matches!(
                    &error,
                    ConfigError::BatchSettlementUnsupportedFacilitatorScheme { value: written }
                        if written == value
                ),
                "got {error:?}"
            );
            let message = error.to_string();
            assert!(message.contains("facilitator_url") && message.contains("https"));
        }
    }

    // -- Solana --

    #[test]
    fn a_solana_table_takes_the_recorded_defaults_for_what_it_omits() {
        let config = solana("min_sponsored_deposit = 1000000").expect("a complete opt-in");

        assert_eq!(config.min_grace_period_secs(), ONE_DAY);
        assert_eq!(config.min_sponsored_deposit(), 1_000_000);
    }

    #[test]
    fn a_zero_minimum_sponsored_deposit_is_refused_by_name() {
        let error = solana("min_sponsored_deposit = 0").expect_err("zero bounds nothing");
        assert!(matches!(
            error,
            ConfigError::BatchSettlementZeroMinimumSponsoredDeposit
        ));
        assert!(error.to_string().contains("min_sponsored_deposit"));
    }

    #[test]
    fn a_solana_minimum_grace_period_below_the_floor_is_refused_by_name() {
        let error = solana("min_sponsored_deposit = 1\nmin_grace_period_secs = 899")
            .expect_err("below x402's 900 seconds");
        assert!(matches!(
            error,
            ConfigError::BatchSettlementDelayBelowFloor {
                table: "solana",
                key: "min_grace_period_secs",
                value: 899,
            }
        ));

        let config = solana("min_sponsored_deposit = 1\nmin_grace_period_secs = 900")
            .expect("the floor itself is legal");
        assert_eq!(config.min_grace_period_secs(), 900);
    }

    /// The program has no upper bound on `grace_period`, so neither does
    /// the minimum.
    #[test]
    fn a_long_solana_minimum_grace_period_is_accepted() {
        let config =
            solana("min_sponsored_deposit = 1\nmin_grace_period_secs = 31536000").expect("a year");
        assert_eq!(config.min_grace_period_secs(), 31_536_000);
    }
}
