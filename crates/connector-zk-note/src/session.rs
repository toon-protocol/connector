//! The session key's signature over an allowance claim (ADR 0080 decision 3, as amended).
//!
//! An allowance claim is signed by the allowance's session key with Mina's kimchi Schnorr, the
//! **chunked** nonce derivation and the **devnet** domain (`NetworkId::TESTNET`), exactly as the
//! state is signed, so a wallet signs one way with one library. `tests/zk_toon_parity.rs` pins
//! [`sign_fields`] against o1js's `(r, s)` for zk_toon's state vectors, which is the evidence that
//! the field-list signature here is the one o1js makes.
//!
//! The signed fields are the connector's own wire (decision 12 leaves the allowance claim and its
//! signed bytes here): [`allowance_claim_fields`].

use crate::{decompress, CompressedPubKey, Field};
use mina_hasher::{Hashable, ROInput};
use mina_signer::{Keypair, NetworkId, NonceMode, PubKey, Signature, Signer};

/// The signing domain: always the devnet one, on every network.
pub const SIGNING_NETWORK: NetworkId = NetworkId::TESTNET;

/// The domain tag that opens an allowance claim's signed fields, read as a field element by
/// zk_toon's `tag_to_field`. It is the connector's wire, not a zk-note derivation, so it is named
/// outside zk_toon's tag namespace: nothing a payer signs for zk_toon can be replayed as a claim.
pub const ALLOWANCE_CLAIM_TAG: &str = "toon-allowance-claim.v1";

/// An allowance id: 16 bytes the zk edge mints, opaque to the payer. On the wire it is 32
/// lowercase hex characters.
pub type AllowanceIdBytes = [u8; 16];

/// What the session key signs for one allowance claim, in order: the claim tag, the allowance
/// id (its 16 bytes read little-endian) and the cumulative amount in nanomina.
#[must_use]
pub fn allowance_claim_fields(allowance_id: &AllowanceIdBytes, cumulative_amount: u64) -> [Field; 3] {
    use ark_ff::PrimeField;
    [
        zk_note::tag_to_field(ALLOWANCE_CLAIM_TAG),
        Field::from_le_bytes_mod_order(allowance_id),
        Field::from(cumulative_amount),
    ]
}

/// Sign an allowance claim, as a payer's wallet does. The zk edge never calls this outside tests;
/// it is here so the tests sign with the same fields the gate verifies.
#[must_use]
pub fn sign_allowance_claim(
    session: &Keypair,
    allowance_id: &AllowanceIdBytes,
    cumulative_amount: u64,
) -> Signature {
    sign_fields(session, &allowance_claim_fields(allowance_id, cumulative_amount))
}

/// Whether `signature` is the session key's over this allowance claim.
#[must_use]
pub fn verify_allowance_claim(
    signature: &Signature,
    session_key: &CompressedPubKey,
    allowance_id: &AllowanceIdBytes,
    cumulative_amount: u64,
) -> bool {
    let Some(key) = decompress(session_key) else {
        return false;
    };
    verify_fields(
        signature,
        &key,
        &allowance_claim_fields(allowance_id, cumulative_amount),
    )
}

/// Sign `fields` the way o1js's `Signature.create` does.
#[must_use]
pub fn sign_fields(keypair: &Keypair, fields: &[Field]) -> Signature {
    mina_signer::create_kimchi::<Fields>(SIGNING_NETWORK).sign(
        keypair,
        &Fields(fields.to_vec()),
        NonceMode::Chunked,
    )
}

/// Verify a signature made by [`sign_fields`]: refuses one made under any other domain.
#[must_use]
pub fn verify_fields(signature: &Signature, key: &PubKey, fields: &[Field]) -> bool {
    mina_signer::create_kimchi::<Fields>(SIGNING_NETWORK).verify(
        signature,
        key,
        &Fields(fields.to_vec()),
    )
}

/// A signature as it rides the wire: `rx ‖ s`, each 32 bytes big-endian, as 128 lowercase hex
/// characters -- `mina_signer::Signature`'s own `Display`.
#[must_use]
pub fn signature_to_hex(signature: &Signature) -> String {
    signature.to_string()
}

/// The signature 64 wire bytes (`rx ‖ s`, big-endian) encode, or `None` when either half is not
/// a canonical element of its field.
#[must_use]
pub fn signature_from_be_bytes(bytes: &[u8; 64]) -> Option<Signature> {
    use ark_ff::{BigInteger, PrimeField};
    use mina_signer::ScalarField;
    let mut rx = [0u8; 32];
    rx.copy_from_slice(&bytes[..32]);
    rx.reverse();
    let mut s = [0u8; 32];
    s.copy_from_slice(&bytes[32..]);
    s.reverse();
    let rx_field = crate::field_from_le_bytes(&rx)?;
    let s_scalar = ScalarField::from_le_bytes_mod_order(&s);
    (s_scalar.into_bigint().to_bytes_le() == s).then(|| Signature::new(rx_field, s_scalar))
}

#[derive(Clone)]
struct Fields(Vec<Field>);

impl Hashable for Fields {
    type D = NetworkId;

    fn to_roinput(&self) -> ROInput {
        self.0
            .iter()
            .copied()
            .fold(ROInput::new(), ROInput::append_field)
    }

    fn domain_string(network: NetworkId) -> Option<String> {
        Some(network.into_domain_string())
    }
}
