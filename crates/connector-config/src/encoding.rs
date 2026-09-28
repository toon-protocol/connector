//! The byte spellings config fields are written in -- hex addresses and
//! ids, base58 Solana accounts -- parsed and rendered one way for every
//! table that names one.

/// Parse a 20-byte EVM address written as 40 hex characters, an optional
/// `0x`/`0X` prefix accepted -- the one rule for every address a table
/// names, since operators write them all the same way.
pub(crate) fn parse_evm_address(value: &str) -> Option<[u8; 20]> {
    parse_hex_bytes::<20>(value)
}

/// Parse exactly `N` bytes written as `2N` hex characters, an optional
/// `0x`/`0X` prefix accepted.
pub(crate) fn parse_hex_bytes<const N: usize>(value: &str) -> Option<[u8; N]> {
    let hex = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);
    if hex.len() != N * 2 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = [0u8; N];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// `0x` and lowercase hex: the canonical spelling a parsed value is stored
/// and compared in.
pub(crate) fn to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(2 + bytes.len() * 2);
    out.push_str("0x");
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Whether `value` is base58 encoding exactly 32 bytes -- a Solana account
/// or Ed25519 public key's own wire shape. Only checked: the string the
/// operator wrote is what is stored.
pub(crate) fn is_base58_32_bytes(value: &str) -> bool {
    matches!(bs58::decode(value).into_vec(), Ok(bytes) if bytes.len() == 32)
}
