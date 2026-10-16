//! Byte-for-byte parity with o1js 3.1.0, against `fixtures/parity.json` (spike #1512,
//! `toon-protocol/zk-note-vectors@658c3d43`). One test per primitive, plus the fixture's hash.

use ark_ff::{BigInteger, PrimeField};
use connector_zk_note::group_map::hash_to_group;
use connector_zk_note::poseidon::{hash, hash_with_prefix};
use connector_zk_note::state::{sign_fields, sign_with, verify_fields};
use connector_zk_note::{Field, Scalar};
use mina_hasher::{Hashable, ROInput};
use mina_signer::{Keypair, NetworkId, NonceMode, SecKey};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::str::FromStr;

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/parity.json");

/// SHA-256 of `fixtures/parity.json`, copied from `o1js-3.1.0/fixtures/parity.json` at
/// `toon-protocol/zk-note-vectors@658c3d43`. A Mina hard fork means regenerating the fixture
/// with the o1js release for the new era and repinning proof-systems; this hash is what stops
/// the fixture changing underneath a passing test.
const FIXTURE_SHA256: &str = "41a2d4f01cca38057b54ca5fc59195083bf42c1fadc6fba8d1b3223e13934679";

fn fixture() -> Value {
    serde_json::from_slice(&std::fs::read(FIXTURE).unwrap()).unwrap()
}

fn f(v: &Value) -> Field {
    Field::from_str(v.as_str().unwrap()).unwrap()
}

fn fs(v: &Value) -> Vec<Field> {
    v.as_array().unwrap().iter().map(f).collect()
}

fn prefix(v: &Value) -> Vec<u8> {
    v.as_str().unwrap().as_bytes().to_vec()
}

fn keypair(private_key: &Value) -> Keypair {
    let k = Scalar::from_str(private_key.as_str().unwrap()).unwrap();
    Keypair::try_from(SecKey::new(k)).unwrap()
}

#[test]
fn the_fixture_is_the_one_o1js_produced() {
    let bytes = std::fs::read(FIXTURE).unwrap();
    let hex: String = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(hex, FIXTURE_SHA256);
}

#[test]
fn poseidon_hash_matches_o1js() {
    let cases = fixture()["poseidon"].as_array().unwrap().clone();
    assert!(!cases.is_empty());
    for c in cases {
        assert_eq!(hash(&fs(&c["input"])), f(&c["output"]), "{c}");
    }
}

#[test]
fn hash_with_prefix_matches_o1js_for_every_20_byte_prefix() {
    let cases = fixture()["hashWithPrefix"].as_array().unwrap().clone();
    let mut n = 0;
    for c in cases {
        let p = prefix(&c["prefix"]);
        let Ok(p) = <[u8; 20]>::try_from(p.as_slice()) else {
            continue;
        };
        assert_eq!(
            hash_with_prefix(&p, &fs(&c["input"])),
            f(&c["output"]),
            "{c}"
        );
        n += 1;
    }
    assert_eq!(n, 12);
}

/// A string `mina-hasher` pads with `*` to 20 bytes, as Rust does for any short domain.
#[derive(Clone)]
struct Short(Vec<Field>);

impl Hashable for Short {
    type D = ();
    fn to_roinput(&self) -> ROInput {
        self.0
            .iter()
            .copied()
            .fold(ROInput::new(), ROInput::append_field)
    }
    fn domain_string(_: ()) -> Option<String> {
        Some("toon-zk-note".to_string())
    }
}

#[test]
fn a_short_prefix_does_not_match() {
    // o1js zero-pads `toon-zk-note`; Rust's mina-hasher pads it with `*`. The fixture's four
    // short-prefix cases are o1js's, and the `*`-padded hash differs from every one.
    let cases: Vec<Value> = fixture()["hashWithPrefix"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["prefix"] == "toon-zk-note")
        .cloned()
        .collect();
    assert_eq!(cases.len(), 4);
    let mut hasher = mina_hasher::create_kimchi::<Short>(());
    for c in cases {
        let input = fs(&c["input"]);
        let mut padded = [b'*'; 20];
        padded[..12].copy_from_slice(b"toon-zk-note");
        assert_ne!(hash_with_prefix(&padded, &input), f(&c["output"]), "{c}");
        assert_ne!(
            mina_hasher::Hasher::hash(&mut hasher, &Short(input.clone())),
            f(&c["output"])
        );
        // and the o1js value is the zero-padded prefix, which the 20-byte API can express.
        let mut zero = [0u8; 20];
        zero[..12].copy_from_slice(b"toon-zk-note");
        assert_eq!(hash_with_prefix(&zero, &input), f(&c["output"]), "{c}");
    }
}

#[test]
fn hash_to_group_matches_o1js() {
    let cases = fixture()["hashToGroup"].as_array().unwrap().clone();
    assert_eq!(cases.len(), 6);
    for c in cases {
        let p = hash_to_group(&fs(&c["input"]));
        assert_eq!((p.x, p.y), (f(&c["x"]), f(&c["y"])), "{c}");
        assert!(!p.y.into_bigint().is_odd());
    }
}

#[test]
fn schnorr_signatures_match_o1js_byte_for_byte() {
    let cases = fixture()["schnorr"]["provable"].as_array().unwrap().clone();
    assert_eq!(cases.len(), 20);
    for c in cases {
        let kp = keypair(&c["privateKey"]);
        assert_eq!(
            kp.clone().get_address(),
            c["publicKeyBase58"].as_str().unwrap()
        );
        let msg = fs(&c["message"]);
        let sig = sign_fields(&kp, &msg);
        let (r, s) = (
            f(&c["r"]),
            Scalar::from_str(c["s"].as_str().unwrap()).unwrap(),
        );
        assert_eq!((sig.rx, sig.s), (r, s), "{c}");
        assert!(verify_fields(&sig, &kp.public, &msg));
    }
}

#[test]
fn schnorr_matches_mina_signer_sign_fields_too() {
    for c in fixture()["schnorr"]["minaSigner"].as_array().unwrap() {
        let kp = keypair(&c["privateKey"]);
        let sig = sign_fields(&kp, &fs(&c["message"]));
        assert_eq!(sig.rx, f(&c["r"]), "{c}");
        assert_eq!(
            sig.s,
            Scalar::from_str(c["s"].as_str().unwrap()).unwrap(),
            "{c}"
        );
    }
}

#[test]
fn the_legacy_nonce_signs_validly_but_differs_from_the_pinned_signature() {
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
        fn domain_string(n: NetworkId) -> Option<String> {
            Some(n.into_domain_string())
        }
    }
    let mut differing = 0;
    for c in fixture()["schnorr"]["provable"].as_array().unwrap() {
        let kp = keypair(&c["privateKey"]);
        let msg = Fields(fs(&c["message"]));
        let legacy = sign_with(&kp, &msg, NonceMode::Legacy);
        // Still a valid signature: a test that only verified would not notice.
        assert!(verify_fields(&legacy, &kp.public, &msg.0));
        let pinned = (
            f(&c["r"]),
            Scalar::from_str(c["s"].as_str().unwrap()).unwrap(),
        );
        if (legacy.rx, legacy.s) != pinned {
            differing += 1;
        }
    }
    assert_eq!(differing, 20);
}
