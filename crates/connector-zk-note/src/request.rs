//! The request proof as the payer sends it, and the binding tag the zk edge expects of it.
//!
//! The public inputs are zk_toon's [`RequestPublic`], decoded by zk_toon in o1js's field order
//! (zk_toon#29). What the connector adds is the stricter reading of the `JsonProof` around them:
//! every input a canonical decimal field element, and the commitment a point on Pallas, which
//! zk_toon leaves unchecked.

use crate::{Commitment, CompressedPubKey, Field};
use ark_ec::AffineRepr;
use std::str::FromStr;
use zk_note::RequestContext;

pub use zk_note::request::{
    Point, RequestPublic, RequestPublicError, SubsidyPeriodError, REQUEST_PUBLIC_FIELDS,
};

/// Why a request proof's public input is refused as malformed.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RequestProofError {
    /// The proof is not a JSON object with a `publicInput` array of decimal strings.
    #[error("the request proof is not a JsonProof: {0}")]
    NotJsonProof(String),
    /// Not twelve field elements.
    #[error("the request proof's public input is {0} field elements, not {REQUEST_PUBLIC_FIELDS}")]
    Length(usize),
    /// A public input that is not a canonical decimal field element.
    #[error("the request proof's public input {0} is not a field element")]
    NotAField(usize),
    /// A flag (`isOdd`, `subsidised`) that is neither 0 nor 1.
    #[error("the request proof's {0} is not a boolean")]
    NotABool(&'static str),
    /// A time, amount or period that does not fit 64 bits.
    #[error("the request proof's {0} does not fit 64 bits")]
    NotAU64(&'static str),
    /// A commitment that is not a point on Pallas.
    #[error("the request proof's commitment is not a point on Pallas")]
    NotAPoint,
}

impl From<RequestPublicError> for RequestProofError {
    fn from(error: RequestPublicError) -> Self {
        match error {
            RequestPublicError::Length(len) => RequestProofError::Length(len),
            RequestPublicError::NotBool(name) => RequestProofError::NotABool(name),
            RequestPublicError::NotU64(name) => RequestProofError::NotAU64(name),
        }
    }
}

/// A commitment as zk_toon's [`RequestPublic`] holds it: its affine coordinates.
#[must_use]
pub fn point(commitment: Commitment) -> Point {
    Point {
        x: commitment.x,
        y: commitment.y,
    }
}

/// The twelve field elements of `public`, in o1js order: the inverse of
/// [`RequestPublic::from_fields`].
#[must_use]
pub fn public_input(public: &RequestPublic) -> Vec<Field> {
    vec![
        public.vault_x,
        Field::from(u64::from(public.vault_is_odd)),
        public.root,
        Field::from(public.request_time),
        Field::from(public.tier),
        public.nullifier,
        public.tag,
        public.commitment.x,
        public.commitment.y,
        Field::from(u64::from(public.subsidised)),
        Field::from(public.period),
        public.period_nullifier,
    ]
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
    commitment: Commitment,
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
        let public = RequestPublic::from_fields(&fields)?;
        let commitment = Commitment::new_unchecked(public.commitment.x, public.commitment.y);
        if !commitment.is_on_curve() || commitment.is_zero() {
            return Err(RequestProofError::NotAPoint);
        }
        Ok(RequestProof {
            json: json.to_vec(),
            public,
            commitment,
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

    /// The state key whose signature the proof checks: the zk edge's own, or the proof is not
    /// about a state this zk edge signed.
    #[must_use]
    pub fn vault(&self) -> CompressedPubKey {
        CompressedPubKey {
            x: self.public.vault_x,
            is_odd: self.public.vault_is_odd,
        }
    }

    /// The re-randomised balance commitment, checked to be a point on Pallas.
    pub fn commitment(&self) -> Commitment {
        self.commitment
    }
}

/// A `JsonProof` carrying `public` and a placeholder proof: what a test sends where a payer
/// would send o1js's output. The format does not depend on the proof being valid.
#[must_use]
pub fn json_proof_for(public: &RequestPublic, proof: &str) -> Vec<u8> {
    let inputs: Vec<String> = public_input(public)
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
