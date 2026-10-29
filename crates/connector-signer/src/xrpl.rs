//! XRPL channel claims (ADR 0087 decisions 2 and 3, issue #1611).
//!
//! An XRPL voucher is the ledger's own `PaymentChannelClaim` authorisation,
//! so the bytes are the ledger's, not TOON's:
//!
//! ```text
//! message = "CLM\0" (0x43 0x4C 0x4D 0x00) ‖ channelId[32] ‖ drops: u64 big-endian
//! ```
//!
//! The key is the channel's `PublicKey`, 33 bytes: **secp256k1 compressed**,
//! or **Ed25519 prefixed `0xED`** -- the prefix byte picks the algorithm.
//! secp256k1 signs `SHA512Half(message)` and the signature is DER; Ed25519
//! signs the raw message. Both are what `rippled`'s `PaymentChannelClaim`
//! verifies (and what `channel_authorize` produces).
//!
//! A channel's id is derived offline, `SHA512Half(0x0078 ‖ source ‖
//! destination ‖ sequence)`, so a payer can name the channel it is about to
//! create ([`xrpl_channel_id`]).
//!
//! The **claim-state challenge** (decision 3) proves control of a channel's
//! key without moving value. It has its own domain tag, so its bytes can never
//! parse as a claim (`'t'` against `0x43`) and a signature on one is never a
//! signature on the other. It binds the CAIP-2 network, which a claim does
//! not.
//!
//! A node's key file holds an XRPL **family seed** exactly as `wallet_propose`
//! emits it (`s…` for secp256k1, `sEd…` for Ed25519); the seed's encoding
//! names the algorithm ([`XrplSigner::from_family_seed`]).

use ed25519_dalek::Signer as _;
use ed25519_dalek::Verifier as _;
use libsecp256k1::{Message, PublicKey, SecretKey, Signature};
use ripemd::Ripemd160;
use sha2::{Digest, Sha256, Sha512};
use thiserror::Error;

/// A claim's first four bytes: `"CLM\0"`.
pub const XRPL_CLAIM_PREFIX: [u8; 4] = [0x43, 0x4C, 0x4D, 0x00];

/// The most a claim may name, in drops: XRP's total supply, 10¹⁷ (ADR 0087
/// decision 2). A figure inside `u64`, so the wire never truncates.
pub const XRPL_MAX_DROPS: u64 = 100_000_000_000_000_000;

/// The domain tag opening every XRPL claim-state challenge.
pub const XRPL_CLAIM_STATE_CHALLENGE_TAG: &[u8] = b"toon-xrpl-voucher-claim-state-challenge-v1";

/// The byte an Ed25519 `PublicKey` is prefixed with.
pub const XRPL_ED25519_PREFIX: u8 = 0xED;

/// Why an XRPL value was refused.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum XrplError {
    /// A claim above the ledger's total supply. Refused, never truncated.
    #[error("amount {amount} exceeds the XRPL ceiling of {max} drops")]
    AmountExceedsChain { amount: u128, max: u64 },
    /// A network string too long for the challenge's one-byte length.
    #[error("network is {0} bytes; a challenge carries at most 255")]
    NetworkTooLong(usize),
    /// Not an XRPL family seed.
    #[error("not an XRPL family seed")]
    InvalidSeed,
    /// Not a classic address.
    #[error("not an XRPL classic address")]
    InvalidAddress,
}

/// `SHA512Half`: the first 32 bytes of SHA-512.
pub fn sha512_half(data: &[u8]) -> [u8; 32] {
    let digest = Sha512::digest(data);
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest[..32]);
    out
}

/// A channel's id from its creating transaction: `SHA512Half(0x0078 ‖
/// source ‖ destination ‖ sequence)`, `sequence` big-endian `u32` (the
/// source's `Sequence` -- or ticket sequence -- on `PaymentChannelCreate`).
pub fn xrpl_channel_id(source: &[u8; 20], destination: &[u8; 20], sequence: u32) -> [u8; 32] {
    let mut buf = Vec::with_capacity(2 + 20 + 20 + 4);
    buf.extend_from_slice(&[0x00, 0x78]);
    buf.extend_from_slice(source);
    buf.extend_from_slice(destination);
    buf.extend_from_slice(&sequence.to_be_bytes());
    sha512_half(&buf)
}

/// The signed claim message for `drops` on `channel_id`. A figure above
/// [`XRPL_MAX_DROPS`] is refused, never truncated.
pub fn xrpl_claim_message(channel_id: &[u8; 32], drops: u128) -> Result<[u8; 44], XrplError> {
    let drops = checked_drops(drops)?;
    let mut message = [0u8; 44];
    message[..4].copy_from_slice(&XRPL_CLAIM_PREFIX);
    message[4..36].copy_from_slice(channel_id);
    message[36..].copy_from_slice(&drops.to_be_bytes());
    Ok(message)
}

/// `drops` as the `u64` the wire carries, or [`XrplError::AmountExceedsChain`].
pub fn checked_drops(drops: u128) -> Result<u64, XrplError> {
    match u64::try_from(drops) {
        Ok(drops) if drops <= XRPL_MAX_DROPS => Ok(drops),
        _ => Err(XrplError::AmountExceedsChain {
            amount: drops,
            max: XRPL_MAX_DROPS,
        }),
    }
}

/// The message a channel's key signs to prove control of `channel_id` on
/// `network` (CAIP-2, e.g. `xrpl:1`) until `expires`:
/// tag ‖ `len(network)`: u8 ‖ network ‖ channelId ‖ expires: u64 big-endian.
pub fn xrpl_voucher_claim_state_challenge_message(
    network: &str,
    channel_id: &[u8; 32],
    expires: u64,
) -> Result<Vec<u8>, XrplError> {
    let length =
        u8::try_from(network.len()).map_err(|_| XrplError::NetworkTooLong(network.len()))?;
    let mut message =
        Vec::with_capacity(XRPL_CLAIM_STATE_CHALLENGE_TAG.len() + 1 + network.len() + 32 + 8);
    message.extend_from_slice(XRPL_CLAIM_STATE_CHALLENGE_TAG);
    message.push(length);
    message.extend_from_slice(network.as_bytes());
    message.extend_from_slice(channel_id);
    message.extend_from_slice(&expires.to_be_bytes());
    Ok(message)
}

/// Whether `signature` is the `public_key` holder's signature over `message`:
/// SHA512Half and DER (low-`s`, as the ledger requires of a claim) for a
/// secp256k1 key, the raw message for an `0xED`-prefixed Ed25519 key. Never
/// panics on attacker-controlled bytes.
pub fn verify_xrpl_signature(public_key: &[u8; 33], message: &[u8], signature: &[u8]) -> bool {
    if public_key[0] == XRPL_ED25519_PREFIX {
        let Ok(key) = ed25519_dalek::PublicKey::from_bytes(&public_key[1..]) else {
            return false;
        };
        let Ok(signature) = ed25519_dalek::Signature::from_bytes(signature) else {
            return false;
        };
        return key.verify(message, &signature).is_ok();
    }
    let Ok(key) = PublicKey::parse_compressed(public_key) else {
        return false;
    };
    let Ok(parsed) = Signature::parse_der(signature) else {
        return false;
    };
    let mut normalized = parsed;
    normalized.normalize_s();
    if normalized != parsed {
        return false;
    }
    libsecp256k1::verify(&Message::parse(&sha512_half(message)), &parsed, &key)
}

/// Whether `signature` is `public_key`'s claim for `drops` on `channel_id`.
/// An amount above the ceiling never verifies.
pub fn verify_xrpl_claim(
    public_key: &[u8; 33],
    channel_id: &[u8; 32],
    drops: u128,
    signature: &[u8],
) -> bool {
    match xrpl_claim_message(channel_id, drops) {
        Ok(message) => verify_xrpl_signature(public_key, &message, signature),
        Err(_) => false,
    }
}

/// Whether `signature` is `public_key`'s claim-state challenge for
/// `channel_id` on `network`.
pub fn verify_xrpl_voucher_claim_state_challenge(
    public_key: &[u8; 33],
    network: &str,
    channel_id: &[u8; 32],
    expires: u64,
    signature: &[u8],
) -> bool {
    match xrpl_voucher_claim_state_challenge_message(network, channel_id, expires) {
        Ok(message) => verify_xrpl_signature(public_key, &message, signature),
        Err(_) => false,
    }
}

/// The 20-byte account id behind a channel `PublicKey`:
/// `RIPEMD160(SHA256(public_key))`.
pub fn xrpl_account_id(public_key: &[u8; 33]) -> [u8; 20] {
    let mut out = [0u8; 20];
    out.copy_from_slice(&Ripemd160::digest(Sha256::digest(public_key)));
    out
}

const BASE58_CHECK_LEN: usize = 4;

fn base58_check_encode(payload: &[u8]) -> String {
    let mut bytes = payload.to_vec();
    bytes.extend_from_slice(&checksum(payload));
    bs58::encode(bytes)
        .with_alphabet(bs58::Alphabet::RIPPLE)
        .into_string()
}

fn base58_check_decode(text: &str) -> Option<Vec<u8>> {
    let bytes = bs58::decode(text)
        .with_alphabet(bs58::Alphabet::RIPPLE)
        .into_vec()
        .ok()?;
    let split = bytes.len().checked_sub(BASE58_CHECK_LEN)?;
    let (payload, sum) = bytes.split_at(split);
    (checksum(payload) == sum).then(|| payload.to_vec())
}

fn checksum(payload: &[u8]) -> [u8; BASE58_CHECK_LEN] {
    let digest = Sha256::digest(Sha256::digest(payload));
    let mut out = [0u8; BASE58_CHECK_LEN];
    out.copy_from_slice(&digest[..BASE58_CHECK_LEN]);
    out
}

/// A classic `r…` address for a 20-byte account id.
pub fn xrpl_classic_address(account_id: &[u8; 20]) -> String {
    let mut payload = vec![0x00];
    payload.extend_from_slice(account_id);
    base58_check_encode(&payload)
}

/// The account id behind a classic `r…` address.
pub fn decode_xrpl_classic_address(address: &str) -> Result<[u8; 20], XrplError> {
    let payload = base58_check_decode(address).ok_or(XrplError::InvalidAddress)?;
    match payload.split_first() {
        Some((0x00, id)) => id.try_into().map_err(|_| XrplError::InvalidAddress),
        _ => Err(XrplError::InvalidAddress),
    }
}

const SECP256K1_SEED_PREFIX: &[u8] = &[0x21];
const ED25519_SEED_PREFIX: &[u8] = &[0x01, 0xE1, 0x4B];

/// The two algorithms an XRPL account key may use. A family seed's encoding
/// names one, and so does a channel `PublicKey`'s first byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XrplAlgorithm {
    Secp256k1,
    Ed25519,
}

impl XrplAlgorithm {
    /// The algorithm a 33-byte channel `PublicKey` names by its prefix byte:
    /// `0xED` is Ed25519, `0x02`/`0x03` a compressed secp256k1 point, and
    /// anything else is no channel key (ADR 0087 decision 2).
    pub fn of_public_key(public_key: &[u8; 33]) -> Option<XrplAlgorithm> {
        match public_key[0] {
            XRPL_ED25519_PREFIX => Some(XrplAlgorithm::Ed25519),
            0x02 | 0x03 => Some(XrplAlgorithm::Secp256k1),
            _ => None,
        }
    }
}

/// The family seed `wallet_propose` would emit for 16 bytes of `entropy`:
/// `s…` for secp256k1, `sEd…` for Ed25519.
pub fn xrpl_family_seed(algorithm: XrplAlgorithm, entropy: &[u8; 16]) -> String {
    let prefix = match algorithm {
        XrplAlgorithm::Secp256k1 => SECP256K1_SEED_PREFIX,
        XrplAlgorithm::Ed25519 => ED25519_SEED_PREFIX,
    };
    let mut payload = prefix.to_vec();
    payload.extend_from_slice(entropy);
    base58_check_encode(&payload)
}

/// Why an XRPL key file could not be read as a family seed.
#[derive(Debug, Error)]
pub enum XrplKeyFileError {
    #[error("the XRPL key file {} could not be read: {source}", path.display())]
    Unreadable {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "the XRPL key file {} is not a family seed: write what wallet_propose emits (s… for \
         secp256k1, sEd… for Ed25519)",
        path.display()
    )]
    NotAFamilySeed { path: std::path::PathBuf },
}

/// A key that signs XRPL claims, loaded from a family seed.
pub enum XrplSigner {
    Secp256k1(SecretKey),
    Ed25519(ed25519_dalek::Keypair),
}

impl std::fmt::Debug for XrplSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("XrplSigner").finish_non_exhaustive()
    }
}

impl XrplSigner {
    /// Read the key from `path`: an XRPL family seed exactly as
    /// `wallet_propose` emits it, surrounding whitespace ignored. A key is
    /// always a file's content, never a config value (ADR 0009).
    ///
    /// # Errors
    /// [`XrplKeyFileError`] for a file that cannot be read or holds no seed.
    pub fn from_key_file(path: &std::path::Path) -> Result<XrplSigner, XrplKeyFileError> {
        let text =
            std::fs::read_to_string(path).map_err(|source| XrplKeyFileError::Unreadable {
                path: path.to_path_buf(),
                source,
            })?;
        XrplSigner::from_family_seed(&text).map_err(|_| XrplKeyFileError::NotAFamilySeed {
            path: path.to_path_buf(),
        })
    }

    /// Load the key a family seed (`s…`, or `sEd…` for Ed25519) names, as the
    /// account's key (`wallet_propose`'s `master_key`/`public_key`).
    pub fn from_family_seed(seed: &str) -> Result<XrplSigner, XrplError> {
        let payload = base58_check_decode(seed.trim()).ok_or(XrplError::InvalidSeed)?;
        if let Some(entropy) = payload.strip_prefix(ED25519_SEED_PREFIX) {
            if entropy.len() != 16 {
                return Err(XrplError::InvalidSeed);
            }
            let secret = ed25519_dalek::SecretKey::from_bytes(&sha512_half(entropy))
                .map_err(|_| XrplError::InvalidSeed)?;
            let public = ed25519_dalek::PublicKey::from(&secret);
            return Ok(XrplSigner::Ed25519(ed25519_dalek::Keypair {
                secret,
                public,
            }));
        }
        if let Some(entropy) = payload.strip_prefix(SECP256K1_SEED_PREFIX) {
            if entropy.len() != 16 {
                return Err(XrplError::InvalidSeed);
            }
            return Ok(XrplSigner::Secp256k1(secp256k1_account_key(entropy)));
        }
        Err(XrplError::InvalidSeed)
    }

    /// Which algorithm this key signs with.
    pub fn algorithm(&self) -> XrplAlgorithm {
        match self {
            XrplSigner::Secp256k1(_) => XrplAlgorithm::Secp256k1,
            XrplSigner::Ed25519(_) => XrplAlgorithm::Ed25519,
        }
    }

    /// The 33-byte channel `PublicKey` form: compressed secp256k1, or `0xED`
    /// ‖ the Ed25519 key.
    pub fn public_key(&self) -> [u8; 33] {
        match self {
            XrplSigner::Secp256k1(secret) => {
                PublicKey::from_secret_key(secret).serialize_compressed()
            }
            XrplSigner::Ed25519(keypair) => {
                let mut out = [0u8; 33];
                out[0] = XRPL_ED25519_PREFIX;
                out[1..].copy_from_slice(keypair.public.as_bytes());
                out
            }
        }
    }

    /// The account's id, and so its classic address.
    pub fn account_id(&self) -> [u8; 20] {
        xrpl_account_id(&self.public_key())
    }

    /// The account's classic `r…` address (a channel's `payTo`).
    pub fn classic_address(&self) -> String {
        xrpl_classic_address(&self.account_id())
    }

    /// Sign `message`: SHA512Half and DER for secp256k1, the raw message for
    /// Ed25519.
    pub fn sign(&self, message: &[u8]) -> Vec<u8> {
        match self {
            XrplSigner::Secp256k1(secret) => {
                let (signature, _) =
                    libsecp256k1::sign(&Message::parse(&sha512_half(message)), secret);
                signature.serialize_der().as_ref().to_vec()
            }
            XrplSigner::Ed25519(keypair) => keypair.sign(message).to_bytes().to_vec(),
        }
    }

    /// Sign a claim for `drops` on `channel_id`.
    pub fn sign_claim(&self, channel_id: &[u8; 32], drops: u128) -> Result<Vec<u8>, XrplError> {
        Ok(self.sign(&xrpl_claim_message(channel_id, drops)?))
    }

    /// Sign a claim-state challenge.
    pub fn sign_claim_state_challenge(
        &self,
        network: &str,
        channel_id: &[u8; 32],
        expires: u64,
    ) -> Result<Vec<u8>, XrplError> {
        Ok(self.sign(&xrpl_voucher_claim_state_challenge_message(
            network, channel_id, expires,
        )?))
    }
}

/// The first `SHA512Half(prefix ‖ sequence)` that is a valid secp256k1 scalar.
fn first_valid_scalar(prefix: &[u8], middle: Option<u32>) -> SecretKey {
    for sequence in 0u32.. {
        let mut buf = prefix.to_vec();
        if let Some(middle) = middle {
            buf.extend_from_slice(&middle.to_be_bytes());
        }
        buf.extend_from_slice(&sequence.to_be_bytes());
        if let Ok(key) = SecretKey::parse(&sha512_half(&buf)) {
            return key;
        }
    }
    unreachable!("a valid scalar turns up within a few tries")
}

/// ripple-keypairs' account key: the family generator's root key, plus the
/// intermediate key of account index 0.
fn secp256k1_account_key(entropy: &[u8]) -> SecretKey {
    let root = first_valid_scalar(entropy, None);
    let root_public = PublicKey::from_secret_key(&root).serialize_compressed();
    let intermediate = first_valid_scalar(&root_public, Some(0));
    let mut account = root;
    account
        .tweak_add_assign(&intermediate)
        .expect("a sum of valid scalars is zero with probability 2^-256");
    account
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// rippled's well-known genesis account: `masterpassphrase`.
    const MASTER_SEED: &str = "snoPBrXtMeMyMHUVTgbuqAfg1SUTb";
    const MASTER_ADDRESS: &str = "rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh";
    const MASTER_PUBLIC_KEY: &str =
        "0330E7FC9D56BB25D6893BA3F317AE5BCF33B3291BD63DB32654A313222F7FD020";

    fn hex_of(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02X}")).collect()
    }

    fn secp() -> XrplSigner {
        XrplSigner::from_family_seed(MASTER_SEED).unwrap()
    }

    fn ed() -> XrplSigner {
        XrplSigner::Ed25519(ed25519_dalek::Keypair::generate(&mut rand::rngs::OsRng))
    }

    #[test]
    fn the_masterpassphrase_seed_is_the_genesis_account() {
        let signer = secp();
        assert_eq!(hex_of(&signer.public_key()), MASTER_PUBLIC_KEY);
        assert_eq!(signer.classic_address(), MASTER_ADDRESS);
        assert_eq!(
            decode_xrpl_classic_address(MASTER_ADDRESS).unwrap(),
            signer.account_id()
        );
    }

    // The references below were produced by xrpl.js 4 (`Wallet.fromSeed`,
    // `signPaymentChannelClaim`, `hashPaymentChannel`), not by this module.
    // Its claim amount is in XRP: 1000000 XRP is 10^12 drops.

    const REFERENCE_CHANNEL: &str =
        "5DB01B7FFED6B67E6B0414DED11E051D2EE2B7619CE0EAA6286D67A3A4D5BDB3";

    #[test]
    fn a_secp256k1_claim_signed_by_xrpl_js_verifies() {
        let channel_id: [u8; 32] = decode_hex(REFERENCE_CHANNEL);
        let signature = decode_vec("304402202BD21754C419DF925C1BA6A33AA5829FD0E5B02CDDB6E99BF1ED5AD9322E9C5202206BC7F871E86F987BB876EAAA075EC832C44E0A9ADAEF6E47C1A387BC8D2ADB60");
        let key = secp().public_key();
        assert!(verify_xrpl_claim(
            &key,
            &channel_id,
            1_000_000_000_000,
            &signature
        ));
        assert!(!verify_xrpl_claim(
            &key,
            &channel_id,
            1_000_000_000_001,
            &signature
        ));
    }

    #[test]
    fn an_ed25519_claim_is_byte_identical_to_xrpl_js() {
        const SEED: &str = "sEdSPx8CLbFUna3DJdvJ74aQPLdemqu";
        let signer = XrplSigner::from_family_seed(SEED).unwrap();
        assert_eq!(
            signer.classic_address(),
            "rEhh6f9rj5UUBhFzGGaxS5zYU2CCqKFXBC"
        );
        assert_eq!(
            hex_of(&signer.public_key()),
            "EDF9096653463E3A0B79A67E69D9E66AB83D93D4E867335ADE5DAFE8467F4F1734"
        );
        let channel_id: [u8; 32] = decode_hex(REFERENCE_CHANNEL);
        assert_eq!(
            hex_of(&signer.sign_claim(&channel_id, 1_000_000_000_000).unwrap()),
            "3B03A8F9AF48039FF3ADABBAC4883EEB617FF9CF428C686A3422B4852FCA3602A80C94C3DF4739F7577BC5591BD7602E10CF220176E618BA73DA8007D0B5280E"
        );
    }

    #[test]
    fn a_channel_id_is_the_one_xrpl_js_derives() {
        let source = decode_xrpl_classic_address("rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh").unwrap();
        let destination =
            decode_xrpl_classic_address("rLNaPoKeeBjZe2qs6x52yVPZpZ8td4dc6w").unwrap();
        assert_eq!(
            hex_of(&xrpl_channel_id(&source, &destination, 5)),
            "D53C24F11BEA8EC2256FCB3F85371265DACB031B39D1DA676A22393497C0ED9F"
        );
    }

    fn decode_vec(text: &str) -> Vec<u8> {
        (0..text.len() / 2)
            .map(|i| u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).unwrap())
            .collect()
    }

    fn decode_hex<const N: usize>(text: &str) -> [u8; N] {
        let mut out = [0u8; N];
        for (i, byte) in out.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).unwrap();
        }
        out
    }

    #[test]
    fn a_seed_that_is_not_a_family_seed_is_refused() {
        for bad in [
            "",
            "snoPBrXtMeMyMHUVTgbuqAfg1SUTc",
            MASTER_ADDRESS,
            "0000000000000000000000000000000000000000000000000000000000000001",
        ] {
            assert!(XrplSigner::from_family_seed(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn family_seeds_of_both_algorithms_load_from_a_key_file() {
        let dir = tempfile::tempdir().unwrap();
        for (algorithm, expected_prefix) in [
            (XrplAlgorithm::Secp256k1, 's'),
            (XrplAlgorithm::Ed25519, 'E'),
        ] {
            let seed = xrpl_family_seed(algorithm, &[9u8; 16]);
            assert!(seed.starts_with('s'));
            let path = dir.path().join(format!("{expected_prefix}.key"));
            // A trailing newline, as `echo > file` leaves.
            std::fs::write(&path, format!("{seed}\n")).unwrap();
            let signer = XrplSigner::from_key_file(&path).unwrap();
            assert_eq!(signer.algorithm(), algorithm);
            assert_eq!(
                XrplAlgorithm::of_public_key(&signer.public_key()),
                Some(algorithm)
            );
            assert_eq!(
                signer.public_key(),
                XrplSigner::from_family_seed(&seed).unwrap().public_key()
            );
        }
    }

    #[test]
    fn a_key_file_that_is_not_a_family_seed_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settlement.key");
        // A 32-byte hex key is what the other chains' key files hold.
        std::fs::write(&path, "ab".repeat(32)).unwrap();
        assert!(matches!(
            XrplSigner::from_key_file(&path),
            Err(XrplKeyFileError::NotAFamilySeed { .. })
        ));
        assert!(matches!(
            XrplSigner::from_key_file(&dir.path().join("missing.key")),
            Err(XrplKeyFileError::Unreadable { .. })
        ));
    }

    #[test]
    fn an_ed25519_seed_is_named_by_its_encoding() {
        let seed = xrpl_family_seed(XrplAlgorithm::Ed25519, &[7u8; 16]);
        assert!(seed.starts_with("sEd"), "{seed}");
        let signer = XrplSigner::from_family_seed(&seed).unwrap();
        assert!(matches!(signer, XrplSigner::Ed25519(_)));
        assert_eq!(signer.public_key()[0], 0xED);
        assert!(signer.classic_address().starts_with('r'));
    }

    #[test]
    fn a_challenge_is_never_a_claim() {
        let message = xrpl_voucher_claim_state_challenge_message("xrpl:1", &[9; 32], 5).unwrap();
        assert_ne!(message[..4], XRPL_CLAIM_PREFIX);
        assert_ne!(message.len(), 44);
        for signer in [secp(), ed()] {
            let signature = signer.sign(&message);
            assert!(verify_xrpl_voucher_claim_state_challenge(
                &signer.public_key(),
                "xrpl:1",
                &[9; 32],
                5,
                &signature
            ));
            assert!(!verify_xrpl_claim(
                &signer.public_key(),
                &[9; 32],
                5,
                &signature
            ));
        }
    }

    #[test]
    fn an_amount_above_the_ceiling_is_refused_not_truncated() {
        assert!(xrpl_claim_message(&[1; 32], XRPL_MAX_DROPS as u128).is_ok());
        for over in [
            XRPL_MAX_DROPS as u128 + 1,
            u64::MAX as u128,
            u64::MAX as u128 + 1,
        ] {
            assert!(matches!(
                xrpl_claim_message(&[1; 32], over),
                Err(XrplError::AmountExceedsChain { .. })
            ));
        }
    }

    proptest! {
        #[test]
        fn claims_round_trip_in_both_algorithms(
            channel in any::<[u8; 32]>(),
            drops in 0..=XRPL_MAX_DROPS,
            use_ed in any::<bool>(),
        ) {
            let signer = if use_ed { ed() } else { secp() };
            let signature = signer.sign_claim(&channel, drops as u128).unwrap();
            prop_assert!(verify_xrpl_claim(&signer.public_key(), &channel, drops as u128, &signature));
        }

        #[test]
        fn any_tampered_claim_byte_fails(
            channel in any::<[u8; 32]>(),
            drops in 0..XRPL_MAX_DROPS,
            use_ed in any::<bool>(),
            flip in any::<usize>(),
            bit in 0u8..8,
        ) {
            let signer = if use_ed { ed() } else { secp() };
            let signature = signer.sign_claim(&channel, drops as u128).unwrap();
            let key = signer.public_key();
            // the amount
            prop_assert!(!verify_xrpl_claim(&key, &channel, drops as u128 + 1, &signature));
            // the channel id
            let mut other = channel;
            other[flip % 32] ^= 1 << bit;
            prop_assert!(!verify_xrpl_claim(&key, &other, drops as u128, &signature));
            // the signature
            let mut bad = signature.clone();
            let at = flip % bad.len();
            bad[at] ^= 1 << bit;
            prop_assert!(!verify_xrpl_claim(&key, &channel, drops as u128, &bad));
        }

        #[test]
        fn challenges_round_trip_and_bind_network_channel_and_expiry(
            channel in any::<[u8; 32]>(),
            expires in 0..u64::MAX,
            use_ed in any::<bool>(),
            flip in 0usize..32,
        ) {
            let signer = if use_ed { ed() } else { secp() };
            let key = signer.public_key();
            let signature = signer.sign_claim_state_challenge("xrpl:1", &channel, expires).unwrap();
            prop_assert!(verify_xrpl_voucher_claim_state_challenge(&key, "xrpl:1", &channel, expires, &signature));
            prop_assert!(!verify_xrpl_voucher_claim_state_challenge(&key, "xrpl:0", &channel, expires, &signature));
            prop_assert!(!verify_xrpl_voucher_claim_state_challenge(&key, "xrpl:1", &channel, expires + 1, &signature));
            let mut other = channel;
            other[flip] ^= 1;
            prop_assert!(!verify_xrpl_voucher_claim_state_challenge(&key, "xrpl:1", &other, expires, &signature));
        }
    }

    #[test]
    fn channel_ids_differ_with_every_input() {
        let base = xrpl_channel_id(&[1; 20], &[2; 20], 3);
        assert_ne!(base, xrpl_channel_id(&[9; 20], &[2; 20], 3));
        assert_ne!(base, xrpl_channel_id(&[1; 20], &[9; 20], 3));
        assert_ne!(base, xrpl_channel_id(&[1; 20], &[2; 20], 4));
    }
}
