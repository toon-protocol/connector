//! `[settlement.evm.batch_settlement]` and `[settlement.solana.batch_settlement]`:
//! whether this node accepts x402 `batch-settlement` channels on that chain,
//! and on what terms (ADR 0074 decisions 1 and 5).

//!
//! **Off unless configured.** Writing the table is the opt-in; a node whose
//! `[settlement.<chain>]` table has no `batch_settlement` sub-table offers no
//! `batch-settlement` entry on that chain and refuses a voucher by name. There
//! is no `enabled` key: presence already says it, and a second spelling of
//! "on" is a second place for the two to disagree.
//!
//! **What is not here.** Everything ADR 0074 decision 2 _fixes_ about an
//! admissible channel is read from the enclosing settlement table and never
//! declared again (CF-26): the receiver is that table's settlement key, and
//! the token or mint is its `token_address`. These tables hold only the
//! terms that are this node's to choose.

use serde::Deserialize;

use crate::client_channel::is_base58_32_bytes;
use crate::error::ConfigError;
use crate::settlement::parse_evm_address;

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

/// `x402BatchSettlement`, at the one address x402 deploys it on both Base
/// Sepolia and Base mainnet: `0x4020074e9dF2ce1deE5A9C1b5c3f541D02a10003`
/// (ADR 0074, _Sources_).
pub const DEFAULT_EVM_BATCH_SETTLEMENT_CONTRACT: [u8; 20] = [
    0x40, 0x20, 0x07, 0x4e, 0x9d, 0xf2, 0xce, 0x1d, 0xee, 0x5a, 0x9c, 0x1b, 0x5c, 0x3f, 0x54, 0x1d,
    0x02, 0xa1, 0x00, 0x03,
];

/// solana-foundation's `payment-channels` program, at the one id it is
/// deployed under on both devnet and mainnet-beta (ADR 0074, _Sources_).
/// Unrelated to `[settlement.solana] program_id`, which is TOON's own
/// payment-channel program.
pub const DEFAULT_SOLANA_BATCH_SETTLEMENT_PROGRAM: &str =
    "CHNLxYvVA28MJP9PrFuDXccuoGXAx7jBacfLEkahyGsX";

/// `[settlement.evm.batch_settlement]` as written. Every key is optional:
/// writing the table is the opt-in (ADR 0074 decision 1), and each value it
/// omits takes the default the record chose.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawEvmBatchSettlementTable {
    #[serde(default)]
    min_withdraw_delay_secs: Option<u64>,
    #[serde(default)]
    contract_address: Option<String>,
}

/// `[settlement.solana.batch_settlement]` as written. `min_sponsored_deposit`
/// is required: it bounds a public endpoint that spends this node's lamports
/// (ADR 0074 decision 9), and no one number is a safe default for every mint.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawSolanaBatchSettlementTable {
    #[serde(default)]
    min_grace_period_secs: Option<u64>,
    min_sponsored_deposit: u64,
    #[serde(default)]
    program_id: Option<String>,
}

/// A validated `[settlement.evm.batch_settlement]`: this node accepts x402
/// `batch-settlement` channels on EVM (ADR 0074).
///
/// It names only the terms that are this node's to choose. `receiver` and
/// `receiverAuthorizer` must both be the enclosing `[settlement.evm]` table's
/// settlement address, and `token` its `token_address`; neither is declared
/// here a second time (CF-26).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvmBatchSettlementConfig {
    min_withdraw_delay_secs: u64,
    contract_address: [u8; 20],
}

impl EvmBatchSettlementConfig {
    /// The shortest `withdrawDelay` a channel may carry and still be
    /// admitted, in seconds, and the figure the greeting publishes (ADR 0074
    /// decision 8). Between 900 and thirty days inclusive; one day unless
    /// the table says otherwise.
    pub fn min_withdraw_delay_secs(&self) -> u64 {
        self.min_withdraw_delay_secs
    }

    /// The `x402BatchSettlement` contract: the verifying contract of every
    /// voucher's EIP-712 domain, and the contract `claim` is sent to. Read
    /// from here and never from a voucher (ADR 0074 decision 4).
    pub fn contract_address(&self) -> [u8; 20] {
        self.contract_address
    }
}

/// A validated `[settlement.solana.batch_settlement]`: this node accepts,
/// and sponsors the opening of, x402 `batch-settlement` channels on Solana
/// (ADR 0074).
///
/// As on EVM, what the record fixes comes from the enclosing
/// `[settlement.solana]` table: the sponsor key (`payee` and `rent_payer`) is
/// its settlement key, and `mint` must be its `token_address`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SolanaBatchSettlementConfig {
    min_grace_period_secs: u64,
    min_sponsored_deposit: u64,
    program_id: String,
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

    /// The `payment-channels` program, base58: the program every voucher's
    /// channel account must belong to, read from here and never from a
    /// voucher (ADR 0074 decision 4).
    pub fn program_id(&self) -> &str {
        &self.program_id
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
    let contract_address = match raw.contract_address {
        None => DEFAULT_EVM_BATCH_SETTLEMENT_CONTRACT,
        Some(value) => match parse_evm_address(&value) {
            Some(address) => address,
            None => return Err(ConfigError::BatchSettlementInvalidContractAddress { value }),
        },
    };
    Ok(EvmBatchSettlementConfig {
        min_withdraw_delay_secs,
        contract_address,
    })
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
    let program_id = match raw.program_id {
        None => DEFAULT_SOLANA_BATCH_SETTLEMENT_PROGRAM.to_string(),
        Some(value) if is_base58_32_bytes(&value) => value,
        Some(value) => return Err(ConfigError::BatchSettlementInvalidProgramId { value }),
    };
    Ok(SolanaBatchSettlementConfig {
        min_grace_period_secs,
        min_sponsored_deposit: raw.min_sponsored_deposit,
        program_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evm(text: &str) -> Result<EvmBatchSettlementConfig, ConfigError> {
        resolve_evm_batch_settlement(toml::from_str(text).expect("valid TOML"))
    }

    fn solana(text: &str) -> Result<SolanaBatchSettlementConfig, ConfigError> {
        resolve_solana_batch_settlement(toml::from_str(text).expect("valid TOML"))
    }

    const ONE_DAY: u64 = 86_400;

    // -- EVM --

    /// Writing the table is the opt-in, and everything in it has a default
    /// the record chose: one day, and the contract x402 deploys.
    #[test]
    fn an_empty_evm_table_takes_the_recorded_defaults() {
        let config = evm("").expect("an empty table is a complete opt-in");

        assert_eq!(config.min_withdraw_delay_secs(), ONE_DAY);
        assert_eq!(
            config.contract_address(),
            DEFAULT_EVM_BATCH_SETTLEMENT_CONTRACT
        );
    }

    /// The default is the address the record cites, spelled the way it
    /// cites it -- not a byte array nobody can check by eye.
    #[test]
    fn the_default_evm_contract_is_the_deployed_x402_batch_settlement() {
        assert_eq!(
            parse_evm_address("0x4020074e9dF2ce1deE5A9C1b5c3f541D02a10003"),
            Some(DEFAULT_EVM_BATCH_SETTLEMENT_CONTRACT)
        );
        assert!(is_base58_32_bytes(DEFAULT_SOLANA_BATCH_SETTLEMENT_PROGRAM));
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

    #[test]
    fn an_evm_contract_address_may_be_named() {
        let config = evm(r#"contract_address = "0x1234567890123456789012345678901234567890""#)
            .expect("an explicit contract");
        assert_eq!(
            config.contract_address(),
            [
                0x12, 0x34, 0x56, 0x78, 0x90, 0x12, 0x34, 0x56, 0x78, 0x90, 0x12, 0x34, 0x56, 0x78,
                0x90, 0x12, 0x34, 0x56, 0x78, 0x90
            ]
        );
    }

    #[test]
    fn a_malformed_evm_contract_address_is_refused_by_name() {
        let error = evm(r#"contract_address = "0x1234""#).expect_err("not an address");
        assert!(matches!(
            error,
            ConfigError::BatchSettlementInvalidContractAddress { ref value } if value == "0x1234"
        ));
    }

    #[test]
    fn an_unknown_key_in_the_evm_table_is_refused() {
        let error = toml::from_str::<RawEvmBatchSettlementTable>("enabled = true")
            .expect_err("deny_unknown_fields");
        assert!(error.to_string().contains("unknown field"));
    }

    /// The Solana-only key is not accepted on the EVM table: each table
    /// takes its own chain's terms and nothing else.
    #[test]
    fn a_solana_key_in_the_evm_table_is_refused() {
        assert!(
            toml::from_str::<RawEvmBatchSettlementTable>("min_grace_period_secs = 900").is_err()
        );
    }

    // -- Solana --

    #[test]
    fn a_solana_table_takes_the_recorded_defaults_for_what_it_omits() {
        let config = solana("min_sponsored_deposit = 1000000").expect("a complete opt-in");

        assert_eq!(config.min_grace_period_secs(), ONE_DAY);
        assert_eq!(config.min_sponsored_deposit(), 1_000_000);
        assert_eq!(config.program_id(), DEFAULT_SOLANA_BATCH_SETTLEMENT_PROGRAM);
    }

    /// Decision 5: the sponsor endpoint is public and spends lamports, so
    /// the bound on it has no safe default. Omitting it is refused by the
    /// key's own name.
    #[test]
    fn a_solana_table_without_a_minimum_sponsored_deposit_is_refused_by_name() {
        let error = toml::from_str::<RawSolanaBatchSettlementTable>("min_grace_period_secs = 900")
            .expect_err("the minimum deposit is required");
        assert!(
            error.to_string().contains("min_sponsored_deposit"),
            "got: {error}"
        );
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

    #[test]
    fn a_malformed_solana_program_id_is_refused_by_name() {
        let error = solana("min_sponsored_deposit = 1\nprogram_id = \"not-base58-0OIl\"")
            .expect_err("not a program id");
        assert!(matches!(
            error,
            ConfigError::BatchSettlementInvalidProgramId { ref value } if value == "not-base58-0OIl"
        ));
    }

    #[test]
    fn an_unknown_key_in_the_solana_table_is_refused() {
        assert!(toml::from_str::<RawSolanaBatchSettlementTable>(
            "min_sponsored_deposit = 1\nmin_withdraw_delay_secs = 900"
        )
        .is_err());
    }
}
