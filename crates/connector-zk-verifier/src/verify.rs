//! The crate's public API over the vendored Pickles verifier.
//!
//! Derived from `toon-protocol/zk_toon` `zk-prover/src/verify.rs` (Apache-2.0), which builds on
//! `toon-protocol/mina-rust`; see `NOTICE`.

use std::{panic::AssertUnwindSafe, str::FromStr};

use ark_ff::PrimeField;
use base64::Engine as _;
use mina_curves::pasta::{Fp, Fq};

use crate::{
    ledger::{
        proofs::{
            unfinalized::dummy_ipa_wrap_challenges, verification::verify_sideloaded_with_index,
            verifiers::make_zkapp_verifier_index, VerifierIndex,
        },
        verifier::get_srs,
        VerificationKey,
    },
    p2p_messages::{
        binprot::BinProtRead,
        v2::{
            MinaBaseVerificationKeyWireStableV1, PicklesProofProofsVerified2ReprStableV2,
            PicklesProofProofsVerifiedMaxStableV2,
        },
    },
};

/// Why a key, a proof or a field element was refused.
#[derive(Debug, thiserror::Error)]
pub enum VerifierError {
    /// The text is not base64 of the expected alphabet.
    #[error("not valid base64: {0}")]
    Base64(#[from] base64::DecodeError),
    /// The verification key is not a binprot-encoded Pickles side-loaded key.
    #[error("not a verification key: {0}")]
    Key(String),
    /// The verification key decoded but bytes were left over.
    #[error("trailing bytes after the verification key")]
    TrailingBytes,
    /// The proof is not an o1js sexp-encoded Pickles proof.
    #[error("not a proof: {0}")]
    Proof(String),
    /// A decimal string is not a field element.
    #[error("not a field element: {0}")]
    Field(String),
}

/// A Pickles verification key, as o1js writes it (`verificationKey.data`).
#[derive(Clone, Debug)]
pub struct VerifierKey(VerificationKey);

impl VerifierKey {
    /// Decode o1js's base64 binprot verification key (`verificationKey.data`).
    ///
    /// # Errors
    ///
    /// [`VerifierError`] when the text is not base64, is not a verification key, or leaves bytes
    /// after one.
    pub fn from_o1js_base64(data: &str) -> Result<VerifierKey, VerifierError> {
        let bytes = base64::engine::general_purpose::STANDARD.decode(data)?;
        let mut slice = bytes.as_slice();
        let wire = MinaBaseVerificationKeyWireStableV1::binprot_read(&mut slice)
            .map_err(|e| VerifierError::Key(e.to_string()))?;
        if !slice.is_empty() {
            return Err(VerifierError::TrailingBytes);
        }
        VerificationKey::try_from(&wire)
            .map(VerifierKey)
            .map_err(|e| VerifierError::Key(format!("{e:?}")))
    }

    /// The Pickles verification-key hash: the decimal o1js prints as `verificationKey.hash`.
    pub fn hash(&self) -> Fp {
        self.0.hash()
    }
}

/// A verifier for the proofs of one program: its key and the verifier index built from it.
pub struct Verifier {
    key: VerifierKey,
    index: VerifierIndex<Fq>,
}

impl Verifier {
    /// Build the verifier index for `key`, and the structured reference string the accumulator
    /// check reads if nothing has yet (once per process). This takes a few seconds: do it once
    /// and share it.
    pub fn new(key: VerifierKey) -> Verifier {
        drop(get_srs());
        let index = make_zkapp_verifier_index(&key.0);
        Verifier { key, index }
    }

    /// The key this verifier checks proofs under.
    pub fn key(&self) -> &VerifierKey {
        &self.key
    }

    /// Whether `proof` verifies under the key for the public input and output fields `public`
    /// (the public input fields, then the public output fields). About 100 ms of CPU.
    ///
    /// A proof that is well-formed but hostile can reach an assertion in the vendored code; that
    /// is a refusal here, never a panic out of this call.
    pub fn verify(&self, public: &[Fp], proof: &WrapProof) -> bool {
        std::panic::catch_unwind(AssertUnwindSafe(|| {
            verify_sideloaded_with_index(&self.key.0, &self.index, public, &proof.0, &get_srs())
        }))
        .unwrap_or(false)
    }
}

/// A Pickles proof, as the side-loaded verifier reads it (at `maxProofsVerified` 2).
#[derive(Clone)]
pub struct WrapProof(PicklesProofProofsVerified2ReprStableV2);

impl WrapProof {
    /// Decode o1js's URL-safe base64 sexp proof (`proof` of a `JsonProof`), padded to the
    /// side-loaded maximum of two verified proofs.
    ///
    /// # Errors
    ///
    /// [`VerifierError`] when the text is not base64 or not a proof.
    pub fn from_o1js_base64(b64: &str) -> Result<WrapProof, VerifierError> {
        use rsexp::OfSexp;
        let bytes = base64::engine::general_purpose::URL_SAFE.decode(b64)?;
        let mut sexp =
            rsexp::from_slice(&bytes).map_err(|e| VerifierError::Proof(format!("{e:?}")))?;
        pad_proof(&mut sexp)?;
        let max = PicklesProofProofsVerifiedMaxStableV2::of_sexp(&sexp)
            .map_err(|e| VerifierError::Proof(format!("{e:?}")))?;
        Ok(WrapProof((&max).into()))
    }
}

/// `messages_for_next_wrap_proof.old_bulletproof_challenges` of a proof's sexp.
fn old_challenges(sexp: &mut rsexp::Sexp) -> Result<&mut Vec<rsexp::Sexp>, VerifierError> {
    use rsexp::Sexp;
    fn field<'a>(record: &'a mut Sexp, name: &str) -> Result<&'a mut Sexp, VerifierError> {
        let Sexp::List(fields) = record else {
            return Err(VerifierError::Proof("not a record".into()));
        };
        for f in fields.iter_mut() {
            if let Sexp::List(pair) = f {
                if matches!(pair.first(), Some(Sexp::Atom(a)) if a == name.as_bytes()) {
                    if let Some(value) = pair.get_mut(1) {
                        return Ok(value);
                    }
                }
            }
        }
        Err(VerifierError::Proof(format!("no field {name}")))
    }
    let chals = field(
        field(
            field(field(sexp, "statement")?, "proof_state")?,
            "messages_for_next_wrap_proof",
        )?,
        "old_bulletproof_challenges",
    )?;
    let Sexp::List(chals) = chals else {
        return Err(VerifierError::Proof("not a list".into()));
    };
    Ok(chals)
}

/// o1js writes a proof at its own `maxProofsVerified`; a side-loaded verifier reads it at the
/// maximum, 2. Pickles' `Wrap_hack.pad_proof` front-extends
/// `messages_for_next_wrap_proof.old_bulletproof_challenges` to length 2 with
/// `Dummy.Ipa.Wrap.challenges`; the step-side lists stay as they are.
fn pad_proof(sexp: &mut rsexp::Sexp) -> Result<(), VerifierError> {
    use rsexp::Sexp;
    let chals = old_challenges(sexp)?;
    // Written by hand: the vendored `SexpOf` for a 64-bit limb emits `0x...`, which its own
    // `OfSexp` refuses; OCaml writes 16 bare hex digits.
    let atom = |s: &str| Sexp::Atom(s.as_bytes().to_vec());
    let pair = |name: &str, v: Sexp| Sexp::List(vec![atom(name), v]);
    let dummy = Sexp::List(
        dummy_ipa_wrap_challenges()
            .iter()
            .map(|[a, b]| {
                let inner =
                    Sexp::List(vec![atom(&format!("{a:016x}")), atom(&format!("{b:016x}"))]);
                Sexp::List(vec![pair(
                    "prechallenge",
                    Sexp::List(vec![pair("inner", inner)]),
                )])
            })
            .collect(),
    );
    while chals.len() < 2 {
        chals.insert(0, dummy.clone());
    }
    Ok(())
}

/// Parse decimal field elements, as o1js writes `publicInputFields` / `publicOutputFields`.
///
/// A field element is plain decimal digits, canonical (less than the field's modulus, so that no
/// two strings are one element): no sign, no space, nothing o1js does not print.
///
/// # Errors
///
/// [`VerifierError::Field`] for the first string that is not a field element.
pub fn fields(decimals: &[&str]) -> Result<Vec<Fp>, VerifierError> {
    decimals
        .iter()
        .map(|s| {
            let digits = !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
            // `Fp::from_str` reduces modulo the field, so a canonical element is one whose
            // decimal reads back as the string it came from (less leading zeros).
            match digits.then(|| Fp::from_str(s)) {
                Some(Ok(field)) => {
                    let canonical = field.into_bigint().to_string();
                    let given = s.trim_start_matches('0');
                    if canonical == given || (given.is_empty() && canonical == "0") {
                        Ok(field)
                    } else {
                        Err(VerifierError::Field((*s).to_string()))
                    }
                }
                _ => Err(VerifierError::Field((*s).to_string())),
            }
        })
        .collect()
}

// The verifier is built once and shared across threads: a type in it that is not `Send + Sync`
// (an `Rc`, a `RefCell`) fails the build here.
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Verifier>();
    assert_send_sync::<VerifierKey>();
    assert_send_sync::<WrapProof>();
};
