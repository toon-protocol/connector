//! The verifier accepts o1js's real `RequestProgram` proofs, unsubsidised and subsidised, under the
//! request key, and refuses every changed public input, a proof of another program and garbage.
//!
//! The fixture is zk_toon's `test-vectors/request-proofs/request-proofs.json`, unchanged (its
//! sha256 is pinned below): real proofs, real keys, no mocks.

use std::sync::OnceLock;

use connector_zk_verifier::{fields, Fp, Verifier, VerifierKey, WrapProof};
use sha2::{Digest, Sha256};

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/request-proofs.json"
);

/// sha256 of `tests/fixtures/request-proofs.json`, as committed in zk_toon.
const FIXTURE_SHA256: &str = "a27adcee0ad6faa1fd5053a94b385b86dce9af5a857f996b6d398fd526aeeae6";

fn fixtures() -> &'static serde_json::Value {
    static FIXTURES: OnceLock<serde_json::Value> = OnceLock::new();
    FIXTURES.get_or_init(|| {
        serde_json::from_slice(&std::fs::read(FIXTURE).expect("fixture")).expect("fixture json")
    })
}

/// The verifier for the request key, built once for the whole test binary.
fn verifier() -> &'static Verifier {
    static VERIFIER: OnceLock<Verifier> = OnceLock::new();
    VERIFIER.get_or_init(|| {
        let data = fixtures()["verificationKey"]["data"].as_str().unwrap();
        Verifier::new(VerifierKey::from_o1js_base64(data).expect("request key"))
    })
}

fn strs(v: &serde_json::Value) -> Vec<&str> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap())
        .collect()
}

/// A JsonProof's decoded proof and its public input fields followed by its public output fields.
fn parse(json_proof: &serde_json::Value) -> (WrapProof, Vec<Fp>) {
    let proof = WrapProof::from_o1js_base64(json_proof["proof"].as_str().unwrap()).unwrap();
    let mut public = fields(&strs(&json_proof["publicInput"])).unwrap();
    public.extend(fields(&strs(&json_proof["publicOutput"])).unwrap());
    (proof, public)
}

#[test]
fn the_fixture_is_zk_toons_unchanged() {
    let digest = Sha256::digest(std::fs::read(FIXTURE).unwrap());
    assert_eq!(hex(&digest), FIXTURE_SHA256);
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn the_key_hash_is_the_one_o1js_prints() {
    let f = fixtures();
    let key =
        VerifierKey::from_o1js_base64(f["verificationKey"]["data"].as_str().unwrap()).unwrap();
    assert_eq!(
        fields(&[f["verificationKey"]["hash"].as_str().unwrap()]).unwrap(),
        vec![key.hash()]
    );
    // And it is the hash of *this* key: another program's key hashes differently.
    let other = VerifierKey::from_o1js_base64(
        f["otherProgram"]["verificationKey"]["data"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        fields(&[f["otherProgram"]["verificationKey"]["hash"]
            .as_str()
            .unwrap()])
        .unwrap(),
        vec![other.hash()]
    );
    assert_ne!(key.hash(), other.hash());
}

#[test]
fn both_request_proofs_verify() {
    for name in ["unsubsidised", "subsidised"] {
        let (proof, public) = parse(&fixtures()[name]["proof"]);
        assert_eq!(public.len(), 12, "{name} has 12 public input fields");
        assert!(
            verifier().verify(&public, &proof),
            "{name} proof is refused"
        );
    }
}

#[test]
fn every_changed_public_input_is_refused() {
    for name in ["unsubsidised", "subsidised"] {
        let (proof, public) = parse(&fixtures()[name]["proof"]);
        for i in 0..public.len() {
            let mut tampered = public.clone();
            // A boolean is flipped, any other field moves by one.
            tampered[i] = if public[i] == Fp::from(0u64) || public[i] == Fp::from(1u64) {
                Fp::from(1u64) - public[i]
            } else {
                public[i] + Fp::from(1u64)
            };
            assert_ne!(tampered[i], public[i]);
            assert!(
                !verifier().verify(&tampered, &proof),
                "{name} proof verifies with field {i} changed"
            );
        }
    }
}

#[test]
fn a_missing_or_extra_public_input_is_refused() {
    let (proof, public) = parse(&fixtures()["unsubsidised"]["proof"]);
    // Dropping the first field shifts every other one.
    assert!(!verifier().verify(&public[1..], &proof));
    // A field that is not zero, after the last.
    let mut extra = public.clone();
    extra.push(Fp::from(7u64));
    assert!(!verifier().verify(&extra, &proof));
    // The statement is padded with zeros to the circuit's width, so a trailing zero field is
    // implicit: this is the one length change the verifier cannot see, and the caller pins the
    // length of what it passes.
    assert_eq!(public.last(), Some(&Fp::from(0u64)));
    assert!(verifier().verify(&public[..11], &proof));
}

#[test]
fn another_programs_proof_is_refused_under_the_request_key() {
    let other = &fixtures()["otherProgram"]["proof"];
    let (proof, public) = parse(other);
    assert_eq!(public.len(), 13);
    assert!(!verifier().verify(&public, &proof));

    // It is a good proof under its own key, so the refusal is the key's.
    let data = fixtures()["otherProgram"]["verificationKey"]["data"]
        .as_str()
        .unwrap();
    let own = Verifier::new(VerifierKey::from_o1js_base64(data).unwrap());
    assert!(own.verify(&public, &proof));
}

#[test]
fn the_request_proof_is_refused_under_another_programs_key() {
    let data = fixtures()["otherProgram"]["verificationKey"]["data"]
        .as_str()
        .unwrap();
    let own = Verifier::new(VerifierKey::from_o1js_base64(data).unwrap());
    let (proof, public) = parse(&fixtures()["unsubsidised"]["proof"]);
    assert!(!own.verify(&public, &proof));
}

#[test]
fn garbage_is_an_error_and_never_a_panic() {
    let good = fixtures()["unsubsidised"]["proof"]["proof"]
        .as_str()
        .unwrap();
    let garbage = [
        "",
        "!!!not base64!!!",
        "AAAA",
        "KChzdGF0ZW1lbnQpKQ==",
        &good[..good.len() / 2],
        &good[..good.len() - 4],
        &format!("Z{}", &good[1..]),
        &"A".repeat(4096),
    ];
    for g in garbage {
        assert!(WrapProof::from_o1js_base64(g).is_err(), "accepted {g:.40}");
    }

    let key = fixtures()["verificationKey"]["data"].as_str().unwrap();
    let key_garbage = [
        "",
        "!!!",
        "AAAA",
        &key[..key.len() / 2],
        &format!("{key}AAAA"),
        &"A".repeat(2396),
    ];
    for g in key_garbage {
        assert!(
            VerifierKey::from_o1js_base64(g).is_err(),
            "accepted {g:.40}"
        );
    }

    assert!(fields(&["not a number"]).is_err());
    assert!(fields(&["-1"]).is_err());
    assert!(fields(&["+1"]).is_err());
    assert!(fields(&[" 1"]).is_err());
    // The modulus itself is not a field element; the one before it is.
    let p = "28948022309329048855892746252171976963363056481941560715954676764349967630337";
    assert!(fields(&[p]).is_err());
    assert!(fields(&[
        "28948022309329048855892746252171976963363056481941560715954676764349967630336"
    ])
    .is_ok());
    assert!(fields(&[""]).is_err());
    assert!(fields(&["1", "x"]).is_err());
}

#[test]
fn a_damaged_proof_is_refused_whatever_the_damage() {
    use base64::Engine as _;
    let good = fixtures()["unsubsidised"]["proof"]["proof"]
        .as_str()
        .unwrap();
    let public = parse(&fixtures()["unsubsidised"]["proof"]).1;
    let bytes = base64::engine::general_purpose::URL_SAFE
        .decode(good)
        .unwrap();
    let mut refused_at_decoding = 0;
    let mut refused_at_verifying = 0;
    for k in 1..=24 {
        // Change one hex digit, a quarter of the way through, a third, ...: the proof is a sexp
        // of big numbers, so that is a changed field element or point.
        let mut damaged = bytes.clone();
        let start = bytes.len() * k / 25;
        let at = (start..bytes.len())
            .find(|&i| damaged[i].is_ascii_hexdigit())
            .unwrap();
        damaged[at] = if damaged[at] == b'7' { b'8' } else { b'7' };
        let text = base64::engine::general_purpose::URL_SAFE.encode(&damaged);
        match WrapProof::from_o1js_base64(&text) {
            Err(_) => refused_at_decoding += 1,
            Ok(proof) => {
                assert!(
                    !verifier().verify(&public, &proof),
                    "damage {k} at {at} verifies"
                );
                refused_at_verifying += 1;
            }
        }
    }
    assert_eq!(refused_at_decoding + refused_at_verifying, 24);
}

#[test]
fn one_verifier_serves_several_threads() {
    let (proof, public) = parse(&fixtures()["subsidised"]["proof"]);
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..3)
            .map(|_| scope.spawn(|| verifier().verify(&public, &proof)))
            .collect();
        for handle in handles {
            assert!(handle.join().unwrap());
        }
    });
}
