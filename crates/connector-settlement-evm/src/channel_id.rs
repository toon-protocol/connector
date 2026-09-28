//! An EVM x402 channel id: the 32 bytes `x402BatchSettlement.getChannelId`
//! computes, and the [`ChannelId`] string every layer above this crate
//! carries them as.
//!
//! TOON's `TokenNetwork` channel-id derivation (ADR 0059) is deleted with
//! the channels it named (ADR 0075 decision 12, issue #1385). An x402 id is
//! the hash of the channel's whole config, recomputed by
//! `connector_signer::evm_batch_channel_id`, never derived from a pair.

use connector_settlement::ChannelId;

/// A channel id as `0x`-prefixed, zero-padded lowercase hex.
pub(crate) fn format_channel_id(id: [u8; 32]) -> ChannelId {
    let mut hex = String::with_capacity(2 + 64);
    hex.push_str("0x");
    for byte in id {
        hex.push_str(&format!("{byte:02x}"));
    }
    ChannelId(hex)
}

/// The inverse of [`format_channel_id`]: `None` for an id that is not 32
/// bytes of hex (`0x` optional).
#[cfg(any(test, feature = "test-util"))]
pub(crate) fn parse_channel_id(channel: &ChannelId) -> Option<[u8; 32]> {
    let hex_digits = channel.0.strip_prefix("0x").unwrap_or(channel.0.as_str());
    if hex_digits.len() != 64 || !hex_digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex_digits[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_channel_id_round_trips_through_its_hex_form() {
        let id = [0xab; 32];
        let formatted = format_channel_id(id);
        assert_eq!(formatted.0, format!("0x{}", "ab".repeat(32)));
        assert_eq!(parse_channel_id(&formatted), Some(id));
    }

    #[test]
    fn a_malformed_channel_id_does_not_parse() {
        assert_eq!(parse_channel_id(&ChannelId("0x1234".to_string())), None);
        assert_eq!(
            parse_channel_id(&ChannelId(format!("0x{}", "zz".repeat(32)))),
            None
        );
    }
}
