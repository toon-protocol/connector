//! The request proof's public inputs, and the proof as the payer sends it.
//!
//! **Stand-in.** zk_toon has no Rust `RequestPublic` at the pinned commit; it exists only in
//! o1js (`o1js/src/request.ts`). zk_toon#29 will add `zk_note::RequestPublic`, decodable from a
//! `JsonProof`'s `publicInput` in o1js's field order. When it lands, this module's
//! [`RequestPublic`] is replaced by a re-export of that one, and its decoding is dropped.
//!
//! The order is o1js's `RequestPublic` struct, flattened as o1js flattens a `Struct`: a
//! `PublicKey` is `x, isOdd`, a `Group` is `x, y`, a `Bool` is 0 or 1. Twelve field elements.

use crate::{CompressedPubKey, Commitment, Field};
use ark_ec::AffineRepr;
use ark_ff::{BigInteger, PrimeField};
use std::str::FromStr;
use zk_note::RequestContext;

/// How many field elements a request proof's public input is.
pub const PUBLIC_INPUT_LEN: usize = 12;

/// The request proof's public inputs, in o1js order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestPublic {
    /// The state key whose signature the proof checks: the zk edge's own, or the proof is not
    /// about a state this zk edge signed.
    pub vault: CompressedPubKey,
    /// The vault root the note's leaf is proved under.
    pub root: Field,
    /// When the payer says it made the request, in ms. The proof checks the note's expiry only
    /// against this, so the zk edge bounds it against its own clock.
    pub request_time: u64,
    /// The tier: the budget `b` the note covers, in nanomina.
    pub tier: u64,
    /// The spent state's nullifier.
    pub nullifier: Field,
    /// The authorization tag binding the nullifier to this zk edge, the session key, the tier
    /// and the allowance expiry.
    pub tag: Field,
    /// The re-randomised balance commitment.
    pub commitment: Commitment,
    /// A subsidised request. Refused until connector#1581.
    pub subsidised: bool,
    /// The subsidy period. Zero unless subsidised.
    pub period: Field,
    /// The per-period nullifier. Zero unless subsidised.
    pub period_nullifier: Field,
}

/// Why a request proof's public input is refused as malformed.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RequestProofError {
    /// The proof is not a JSON object with a `publicInput` array of decimal strings.
    #[error("the request proof is not a JsonProof: {0}")]
    NotJsonProof(String),
    /// Not twelve field elements.
    #[error("the request proof's public input is {0} field elements, not {PUBLIC_INPUT_LEN}")]
    Length(usize),
    /// A public input that is not a canonical decimal field element.
    #[error("the request proof's public input {0} is not a field element")]
    NotAField(usize),
    /// A flag (`isOdd`, `subsidised`) that is neither 0 nor 1.
    #[error("the request proof's {0} is not a boolean")]
    NotABool(&'static str),
    /// A time or amount that does not fit 64 bits.
    #[error("the request proof's {0} does not fit 64 bits")]
    NotAU64(&'static str),
    /// A commitment that is not a point on Pallas.
    #[error("the request proof's commitment is not a point on Pallas")]
    NotAPoint,
}

impl RequestPublic {
    /// Decode twelve field elements in o1js order.
    ///
    /// # Errors
    /// [`RequestProofError`] naming the first input that is not what its position requires.
    pub fn from_fields(fields: &[Field]) -> Result<RequestPublic, RequestProofError> {
        if fields.len() != PUBLIC_INPUT_LEN {
            return Err(RequestProofError::Length(fields.len()));
        }
        let commitment = Commitment::new_unchecked(fields[7], fields[8]);
        if !commitment.is_on_curve() || commitment.is_zero() {
            return Err(RequestProofError::NotAPoint);
        }
        Ok(RequestPublic {
            vault: CompressedPubKey {
                x: fields[0],
                is_odd: as_bool(fields[1], "vault isOdd")?,
            },
            root: fields[2],
            request_time: as_u64(fields[3], "requestTime")?,
            tier: as_u64(fields[4], "tier")?,
            nullifier: fields[5],
            tag: fields[6],
            commitment,
            subsidised: as_bool(fields[9], "subsidised")?,
            period: fields[10],
            period_nullifier: fields[11],
        })
    }

    /// The twelve field elements, in o1js order.
    #[must_use]
    pub fn to_fields(&self) -> Vec<Field> {
        vec![
            self.vault.x,
            Field::from(u64::from(self.vault.is_odd)),
            self.root,
            Field::from(self.request_time),
            Field::from(self.tier),
            self.nullifier,
            self.tag,
            self.commitment.x,
            self.commitment.y,
            Field::from(u64::from(self.subsidised)),
            self.period,
            self.period_nullifier,
        ]
    }
}

/// The binding tag a zk edge whose state key is `edge` expects for this nullifier, session key,
/// tier and allowance expiry: zk_toon's `auth_tag`, with `edge` entered as the x-coordinate of
/// the state public key (ADR 0080 decision 3).
#[must_use]
pub fn expected_tag(
    nullifier: Field,
    edge: &CompressedPubKey,
    session_key: &CompressedPubKey,
    tier: u64,
    expiry: u64,
) -> Field {
    zk_note::auth_tag(
        nullifier,
        &RequestContext {
            edge: edge.x,
            session_key_x: session_key.x,
            session_key_is_odd: session_key.is_odd,
            tier,
            expiry,
        },
    )
}

/// A request proof as the payer sent it: o1js `JsonProof` bytes, kept verbatim for the evidence
/// file and the verifier, and its public input decoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestProof {
    json: Vec<u8>,
    public: RequestPublic,
}

impl RequestProof {
    /// Read a `JsonProof`: an object whose `publicInput` is an array of decimal strings. The
    /// rest of the object is the verifier's to read.
    ///
    /// # Errors
    /// [`RequestProofError`] when the bytes are not a `JsonProof` or its public input is not a
    /// request proof's.
    pub fn parse(json: &[u8]) -> Result<RequestProof, RequestProofError> {
        let value: serde_json::Value = serde_json::from_slice(json)
            .map_err(|error| RequestProofError::NotJsonProof(error.to_string()))?;
        let inputs = value
            .get("publicInput")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| RequestProofError::NotJsonProof("no 'publicInput' array".into()))?;
        let fields = inputs
            .iter()
            .enumerate()
            .map(|(i, input)| {
                input
                    .as_str()
                    .and_then(decimal_field)
                    .ok_or(RequestProofError::NotAField(i))
            })
            .collect::<Result<Vec<Field>, _>>()?;
        Ok(RequestProof {
            json: json.to_vec(),
            public: RequestPublic::from_fields(&fields)?,
        })
    }

    /// The `JsonProof` bytes, verbatim.
    #[must_use]
    pub fn json(&self) -> &[u8] {
        &self.json
    }

    /// The decoded public input.
    #[must_use]
    pub fn public(&self) -> &RequestPublic {
        &self.public
    }
}

/// A `JsonProof` carrying `public` and a placeholder proof: what a test sends where a payer
/// would send o1js's output. The format does not depend on the proof being valid.
#[must_use]
pub fn json_proof_for(public: &RequestPublic, proof: &str) -> Vec<u8> {
    let inputs: Vec<String> = public
        .to_fields()
        .into_iter()
        .map(zk_note::field_to_string)
        .collect();
    serde_json::json!({
        "publicInput": inputs,
        "publicOutput": [],
        "maxProofsVerified": 0,
        "proof": proof,
    })
    .to_string()
    .into_bytes()
}

/// A canonical decimal field element: digits only, no leading zero, below the modulus.
fn decimal_field(text: &str) -> Option<Field> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if text.len() > 1 && text.starts_with('0') {
        return None;
    }
    let field = Field::from_str(text).ok()?;
    (zk_note::field_to_string(field) == text).then_some(field)
}

fn as_bool(field: Field, name: &'static str) -> Result<bool, RequestProofError> {
    if field == Field::from(0u64) {
        Ok(false)
    } else if field == Field::from(1u64) {
        Ok(true)
    } else {
        Err(RequestProofError::NotABool(name))
    }
}

fn as_u64(field: Field, name: &'static str) -> Result<u64, RequestProofError> {
    let bytes = field.into_bigint().to_bytes_le();
    if bytes[8..].iter().any(|b| *b != 0) {
        return Err(RequestProofError::NotAU64(name));
    }
    let mut low = [0u8; 8];
    low.copy_from_slice(&bytes[..8]);
    Ok(u64::from_le_bytes(low))
}
