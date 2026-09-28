//! `[[peer_channels]]`: the **inbound** half of a config-declared peering
//! (ADR 0075 decisions 4, 5 and 9, issue #1380).
//!
//! Under ADR 0075 a peering is two one-way x402 channels, and the one the
//! peer pays this node on is **admitted, not configured**: the peer opens it,
//! and this node admits its vouchers by exactly the rules it admits a
//! client's by. What makes that channel the *peer's* rather than a client's
//! is its **voucher signer** -- EVM `payerAuthorizer`, Solana
//! `authorized_signer`, always as the chain records it -- being a key bound to
//! the peering. A runtime peering binds the key the peer's self-description
//! publishes (#1378, #1379). A config-declared peering has no
//! self-description in hand at boot, so its row names the key:
//!
//! ```toml
//! [[peer_channels]]
//! peer_id        = "store"
//! voucher_signer = "0x…"   # EVM address, or a base58 Solana key
//! # inbound_channel = "0x…" # optional: the one channel this signer proves the peer role on
//! ```
//!
//! A voucher on a channel that signer signs for, or -- for a packet that moves
//! no value -- the claim-state challenge signed by it, is what gives an
//! interaction role `peer` (`peer-carriage-spec.md` §1.2, ADR 0060 as ADR
//! 0075 decision 5 amends it). Without an `inbound_channel`, every channel the
//! signer's vouchers verify on is the peer's, the same as a runtime peering's;
//! with one, only that channel is.
//!
//! The outbound half -- the channel this node pays the peer on -- is
//! `[[pay_channels]]`'s (`crate::pay_channel`).
//!
//! # What a `toon-channel` row wrote, and why each field is refused by name
//!
//! This table used to name a TOON channel and the key a `toon-channel` claim
//! on it was verified against: an EVM `channel_id` derived from the two
//! participants (ADR 0059) with the `chain_id`/`token_network` EIP-712 domain
//! it was signed under (ADR 0024), or a Solana `channel_account` of TOON's
//! own program, and in both cases a `counterparty_key`. ADR 0075 retires all
//! of it, and every one of those fields is still parsed so that a file which
//! writes it is refused naming it (ADR 0009) rather than failing the row's
//! shape: [`ConfigError::PeerChannelToonFieldRemoved`].

use std::collections::HashSet;

use serde::Deserialize;

use crate::error::ConfigError;
use crate::settlement::{SettlementChain, SettlementTables};
use crate::x402_row::{parse_voucher_signer, parse_x402_channel, RemovedToonFields};

/// One `[[peer_channels]]` entry as written in the config file.
///
/// `deny_unknown_fields`: a dropped `voucher_signer` would be a dropped
/// authorization decision. The `toon-channel` fields are `toml::Value` so a
/// value of any type is named as the removed key.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawPeerChannel {
    peer_id: String,
    #[serde(default)]
    voucher_signer: Option<String>,
    #[serde(default)]
    inbound_channel: Option<String>,
    #[serde(default)]
    channel_id: Option<toml::Value>,
    #[serde(default)]
    channel_account: Option<toml::Value>,
    #[serde(default)]
    chain_id: Option<toml::Value>,
    #[serde(default)]
    token_network: Option<toml::Value>,
    #[serde(default)]
    counterparty_key: Option<toml::Value>,
    #[serde(default)]
    program_id: Option<toml::Value>,
}

impl RawPeerChannel {
    fn removed(&self) -> RemovedToonFields {
        RemovedToonFields {
            channel_id: self.channel_id.is_some(),
            channel_account: self.channel_account.is_some(),
            chain_id: self.chain_id.is_some(),
            token_network: self.token_network.is_some(),
            counterparty_key: self.counterparty_key.is_some(),
            program_id: self.program_id.is_some(),
        }
    }
}

/// A fully validated `[[peer_channels]]` entry. Constructed only by
/// [`resolve_peer_channels`], so a value that exists names a signer in one
/// chain's spelling, on a chain this node takes x402 vouchers on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerChannelConfig {
    peer_id: String,
    chain: SettlementChain,
    voucher_signer: String,
    inbound_channel: Option<String>,
}

impl PeerChannelConfig {
    /// The peering relation this row binds -- a `[[peers]]` entry's `id`. A
    /// row naming an id no `[[peers]]` entry configures is
    /// [`ConfigError::PeerChannelOrphaned`].
    pub fn peer_id(&self) -> &str {
        &self.peer_id
    }

    /// The chain the voucher signer, and so the peer's channel, is on --
    /// read off the signer's own spelling.
    pub fn chain(&self) -> SettlementChain {
        self.chain
    }

    /// The key whose vouchers prove this peering: an EVM address as
    /// lower-case `0x` hex, or a Solana public key in base58. Checked
    /// against the signer **the chain records** for a channel, never
    /// against anything a voucher declares about itself.
    pub fn voucher_signer(&self) -> &str {
        &self.voucher_signer
    }

    /// The one channel [`Self::voucher_signer`] proves this peering on,
    /// when the row names one: an EVM channel id as lower-case `0x` hex, or
    /// a Solana channel account in base58. `None` -- the default -- is
    /// every channel that signer's vouchers verify on.
    pub fn inbound_channel(&self) -> Option<&str> {
        self.inbound_channel.as_deref()
    }
}

fn resolve_peer_channel(
    raw: RawPeerChannel,
    tables: SettlementTables,
) -> Result<PeerChannelConfig, ConfigError> {
    // First, because "you wrote a key that no longer exists" explains the
    // file better than any other complaint when both are true, and because
    // this is the branch that must never fall through to a silent ignore
    // (ADR 0009).
    if let Some(field) = raw.removed().first() {
        return Err(ConfigError::PeerChannelToonFieldRemoved {
            peer_id: raw.peer_id,
            field,
        });
    }
    let Some(written) = raw.voucher_signer else {
        return Err(ConfigError::PeerChannelVoucherSignerMissing {
            peer_id: raw.peer_id,
        });
    };
    let Some(signer) = parse_voucher_signer(&written) else {
        return Err(ConfigError::PeerChannelInvalidVoucherSigner {
            peer_id: raw.peer_id,
            value: written,
        });
    };
    let inbound_channel = match raw.inbound_channel {
        None => None,
        Some(written) => match parse_x402_channel(&written) {
            Some(channel) if channel.chain == signer.chain => Some(channel.value),
            _ => {
                return Err(ConfigError::PeerChannelInvalidInboundChannel {
                    peer_id: raw.peer_id,
                    value: written,
                    chain: signer.chain.name(),
                })
            }
        },
    };
    if !tables.x402(signer.chain) {
        return Err(ConfigError::PeerChannelWithoutX402 {
            peer_id: raw.peer_id,
            chain: signer.chain.name(),
        });
    }
    Ok(PeerChannelConfig {
        peer_id: raw.peer_id,
        chain: signer.chain,
        voucher_signer: signer.value,
        inbound_channel,
    })
}

pub(crate) fn resolve_peer_channels(
    raw: Vec<RawPeerChannel>,
    tables: SettlementTables,
) -> Result<Vec<PeerChannelConfig>, ConfigError> {
    let mut seen_signers = HashSet::with_capacity(raw.len());
    let mut seen_channels = HashSet::with_capacity(raw.len());
    let mut channels = Vec::with_capacity(raw.len());

    for row in raw {
        let row = resolve_peer_channel(row, tables)?;
        // One signer proves one relation: a signer on two rows would make
        // "which peering does this voucher prove?" depend on file order
        // (`connector_runtime::VoucherSignerBindings` refuses the same at
        // runtime). Two rows naming one signer for one peer are refused
        // too -- the second says nothing the first did not.
        if !seen_signers.insert(row.voucher_signer.clone()) {
            return Err(ConfigError::PeerChannelDuplicate {
                value: row.voucher_signer,
            });
        }
        if let Some(channel) = &row.inbound_channel {
            if !seen_channels.insert(channel.clone()) {
                return Err(ConfigError::PeerChannelDuplicate {
                    value: channel.clone(),
                });
            }
        }
        channels.push(row);
    }

    Ok(channels)
}

#[cfg(test)]
mod tests {
    use super::*;

    const EVM_SIGNER: &str = "0x2222222222222222222222222222222222222222";
    const CHANNEL: &str = "0xaaaabbbbccccddddeeeeffff00001111aaaabbbbccccddddeeeeffff00001111";
    const SOLANA_SIGNER: &str = "8pM1DN3RiT8vbom5u1sNryaNT1nyL8CTTW3b5PwWXRBH";
    const SOLANA_CHANNEL: &str = "4vJ9JU1bJJE96FWSJKvHsmmFADCg4gpZQff4P3bkLKi";

    fn parse(text: &str) -> RawPeerChannel {
        toml::from_str(text).expect("the row parses")
    }

    fn resolve(rows: &[&str]) -> Result<Vec<PeerChannelConfig>, ConfigError> {
        resolve_peer_channels(
            rows.iter().map(|text| parse(text)).collect(),
            SettlementTables::for_tests(true, true),
        )
    }

    fn row(signer: &str) -> String {
        format!("peer_id = \"store\"\nvoucher_signer = \"{signer}\"")
    }

    #[test]
    fn an_evm_row_names_a_voucher_signer_and_is_canonicalized() {
        let channels = resolve(&[&format!(
            "peer_id = \"store\"\nvoucher_signer = \"{}\"\ninbound_channel = \"{}\"",
            EVM_SIGNER.to_uppercase().replace("0X", "0x"),
            CHANNEL.to_uppercase().replace("0X", "0x"),
        )])
        .expect("valid");

        assert_eq!(channels[0].peer_id(), "store");
        assert_eq!(channels[0].chain(), SettlementChain::Evm);
        assert_eq!(channels[0].voucher_signer(), EVM_SIGNER);
        assert_eq!(channels[0].inbound_channel(), Some(CHANNEL));
    }

    #[test]
    fn a_solana_row_names_a_base58_signer_and_needs_no_channel() {
        let channels = resolve(&[&row(SOLANA_SIGNER)]).expect("valid");

        assert_eq!(channels[0].chain(), SettlementChain::Solana);
        assert_eq!(channels[0].voucher_signer(), SOLANA_SIGNER);
        assert_eq!(channels[0].inbound_channel(), None);
    }

    /// ADR 0009 and issue #1380's acceptance criterion: every field a
    /// `toon-channel` row wrote is refused **by name**, whatever else the
    /// row says and whatever type the value is.
    #[test]
    fn every_toon_channel_field_is_refused_by_name() {
        for (field, value) in [
            ("channel_id", format!("\"{CHANNEL}\"")),
            ("channel_account", format!("\"{SOLANA_CHANNEL}\"")),
            ("chain_id", "31337".to_string()),
            ("token_network", format!("\"{EVM_SIGNER}\"")),
            ("counterparty_key", format!("\"{EVM_SIGNER}\"")),
            ("program_id", "5".to_string()),
        ] {
            let text = format!("{}\n{field} = {value}", row(EVM_SIGNER));
            let error = resolve(&[&text]).expect_err(field);
            assert!(
                matches!(
                    &error,
                    ConfigError::PeerChannelToonFieldRemoved { peer_id, field: named }
                        if peer_id == "store" && *named == field
                ),
                "{field}: {error:?}"
            );
            let message = error.to_string();
            assert!(
                message.contains(field) && message.contains("ADR 0075"),
                "{message}"
            );
        }
    }

    /// A whole `toon-channel` row, as `local/` and the fleet wrote one
    /// before ADR 0075, is refused naming its first removed field rather
    /// than "missing field voucher_signer".
    #[test]
    fn a_whole_toon_channel_row_is_refused_naming_the_channel_it_derived() {
        let error = resolve(&[&format!(
            "peer_id = \"store\"\nchannel_id = \"{CHANNEL}\"\ncounterparty_key = \
             \"{EVM_SIGNER}\"\nchain_id = 8453\ntoken_network = \"{EVM_SIGNER}\""
        )])
        .unwrap_err();
        assert!(matches!(
            error,
            ConfigError::PeerChannelToonFieldRemoved {
                field: "channel_id",
                ..
            }
        ));

        let error = resolve(&[&format!(
            "peer_id = \"store\"\nchannel_account = \"{SOLANA_CHANNEL}\"\ncounterparty_key = \
             \"{SOLANA_SIGNER}\""
        )])
        .unwrap_err();
        assert!(matches!(
            error,
            ConfigError::PeerChannelToonFieldRemoved {
                field: "channel_account",
                ..
            }
        ));
    }

    #[test]
    fn a_row_without_a_voucher_signer_is_refused_by_name() {
        let error = resolve(&["peer_id = \"store\""]).unwrap_err();
        assert!(matches!(
            error,
            ConfigError::PeerChannelVoucherSignerMissing { ref peer_id } if peer_id == "store"
        ));
    }

    #[test]
    fn a_signer_in_neither_chains_spelling_is_refused() {
        for bad in ["0x12", "not-base58!!!", CHANNEL] {
            let error = resolve(&[&row(bad)]).unwrap_err();
            assert!(
                matches!(error, ConfigError::PeerChannelInvalidVoucherSigner { .. }),
                "{bad}: {error:?}"
            );
        }
    }

    /// An inbound channel on the other chain from its signer names a
    /// channel that signer can never sign for.
    #[test]
    fn an_inbound_channel_on_another_chain_than_its_signer_is_refused() {
        let error = resolve(&[&format!(
            "{}\ninbound_channel = \"{SOLANA_CHANNEL}\"",
            row(EVM_SIGNER)
        )])
        .unwrap_err();
        assert!(matches!(
            error,
            ConfigError::PeerChannelInvalidInboundChannel { chain: "evm", .. }
        ));
    }

    /// A row on a chain this node takes no x402 voucher on names a channel
    /// no voucher could be admitted on.
    #[test]
    fn a_row_on_a_chain_without_x402_is_refused_per_chain() {
        let rows = || vec![parse(&row(EVM_SIGNER))];
        let error = resolve_peer_channels(rows(), SettlementTables::for_tests(false, true))
            .unwrap_err();
        assert!(matches!(
            error,
            ConfigError::PeerChannelWithoutX402 { chain: "evm", .. }
        ));
        assert!(
            resolve_peer_channels(rows(), SettlementTables::for_tests(true, false)).is_ok(),
            "the rule is per chain"
        );
    }

    #[test]
    fn one_signer_on_two_rows_is_refused() {
        let other = format!("peer_id = \"relay\"\nvoucher_signer = \"{EVM_SIGNER}\"");
        let error = resolve(&[&row(EVM_SIGNER), &other]).unwrap_err();
        assert!(matches!(error, ConfigError::PeerChannelDuplicate { .. }));
    }

    #[test]
    fn one_peering_may_bind_a_signer_on_each_chain() {
        let channels = resolve(&[&row(EVM_SIGNER), &row(SOLANA_SIGNER)]).expect("valid");
        assert_eq!(channels.len(), 2);
    }

    #[test]
    fn an_unknown_field_is_refused() {
        assert!(toml::from_str::<RawPeerChannel>(&format!(
            "{}\nvoucher_signr = \"{EVM_SIGNER}\"",
            row(EVM_SIGNER)
        ))
        .is_err());
    }
}
