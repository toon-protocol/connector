//! zk_toon's `test-vectors/zk-note.json`, at the pinned commit, replayed through the calls the
//! connector makes (ADR 0080 decision 12). The file is read from `zk_note::VECTORS_JSON`, so no
//! copy is committed here; its SHA-256 is pinned below so that a pin bump which changes the file
//! fails here first and is reviewed as the derivation change it may be.

use connector_zk_note::note::balance::{charge, commit, Commitment, Scalar};
use connector_zk_note::note::state::{sign_state, verify_state, verify_state_on, StateMessage};
use connector_zk_note::note::{field_to_string, next_anchor, policy, Field, VECTORS_JSON};
use connector_zk_note::request::{expected_tag, RequestPublic};
use connector_zk_note::session::{sign_fields, verify_fields};
use connector_zk_note::{CompressedPubKey, Keypair, NetworkId, SecKey, Signature};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::str::FromStr;

/// SHA-256 of `test-vectors/zk-note.json` at zk_toon `7b399412`.
const VECTORS_SHA256: &str = "69b4130ef59d00ad40330e4f54b071ccd93effdd5c76bcc4cb29c0d7e76fa47e";

fn vectors() -> Value {
    serde_json::from_str(VECTORS_JSON).expect("zk_toon's vectors are JSON")
}

fn s(v: &Value) -> &str {
    v.as_str().expect("a decimal string")
}

fn f(v: &Value) -> Field {
    Field::from_str(s(v)).expect("a field element")
}

fn u(v: &Value) -> u64 {
    s(v).parse().expect("a u64")
}

fn sc(v: &Value) -> Scalar {
    Scalar::from_str(s(v)).expect("a scalar")
}

fn pt(v: &Value) -> Commitment {
    Commitment::new_unchecked(f(&v["x"]), f(&v["y"]))
}

fn bit(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        other => s(other) == "1",
    }
}

fn keypair(secret: &Value) -> Keypair {
    Keypair::try_from(SecKey::new(sc(secret))).expect("a keypair")
}

fn signature(e: &Value) -> Signature {
    Signature::new(f(&e["signature"]["r"]), sc(&e["signature"]["s"]))
}

#[test]
fn the_vectors_are_the_ones_this_pin_was_reviewed_against() {
    let digest = Sha256::digest(VECTORS_JSON.as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(hex, VECTORS_SHA256);
}

#[test]
fn the_binding_tag_the_zk_edge_expects_is_zk_toons() {
    let v = vectors();
    let cases = v["authTags"].as_array().expect("authTags");
    assert!(!cases.is_empty());
    for e in cases {
        let ctx = &e["context"];
        let edge = CompressedPubKey {
            x: f(&ctx["edge"]),
            is_odd: false,
        };
        let session = CompressedPubKey {
            x: f(&ctx["sessionKeyX"]),
            is_odd: bit(&ctx["sessionKeyIsOdd"]),
        };
        let tag = expected_tag(
            f(&e["nullifier"]),
            &edge,
            &session,
            u(&ctx["tier"]),
            u(&ctx["expiry"]),
        );
        assert_eq!(field_to_string(tag), s(&e["tag"]));
    }
}

#[test]
fn a_request_proofs_public_inputs_decode_in_o1js_order_and_bind() {
    let v = vectors();
    let cases = v["requestProofs"].as_array().expect("requestProofs");
    assert!(!cases.is_empty());
    for e in cases {
        let p = &e["public"];
        // o1js flattens `RequestPublic` field by field: a `PublicKey` is `x, isOdd`, a `Group`
        // is `x, y`, a `Bool` is 0 or 1.
        let flat: Vec<Field> = vec![
            f(&p["vault"]["x"]),
            Field::from(u64::from(bit(&p["vault"]["isOdd"]))),
            f(&p["root"]),
            f(&p["requestTime"]),
            f(&p["tier"]),
            f(&p["nullifier"]),
            f(&p["tag"]),
            f(&p["commitment"]["x"]),
            f(&p["commitment"]["y"]),
            Field::from(u64::from(bit(&p["subsidised"]))),
            f(&p["period"]),
            f(&p["periodNullifier"]),
        ];
        let public = RequestPublic::from_fields(&flat).expect("decodes");
        assert_eq!(public.to_fields(), flat);
        assert_eq!(public.vault.x, f(&p["vault"]["x"]));
        assert_eq!(public.root, f(&p["root"]));
        assert_eq!(public.request_time, u(&p["requestTime"]));
        assert_eq!(public.tier, u(&p["tier"]));
        assert_eq!(public.nullifier, f(&p["nullifier"]));
        assert_eq!(public.commitment, pt(&p["commitment"]));
        assert!(!public.subsidised);

        // The tag the zk edge recomputes, with `edge` = the state key's x, is the proof's.
        let ctx = &e["private"]["context"];
        assert_eq!(f(&ctx["edge"]), public.vault.x);
        let session = CompressedPubKey {
            x: f(&ctx["sessionKeyX"]),
            is_odd: bit(&ctx["sessionKeyIsOdd"]),
        };
        let tag = expected_tag(
            public.nullifier,
            &public.vault,
            &session,
            public.tier,
            u(&ctx["expiry"]),
        );
        assert_eq!(tag, public.tag);
    }
}

#[test]
fn the_successor_anchor_follows_the_spent_nullifier() {
    let v = vectors();
    let chain = v["nullifiers"]["chain"].as_array().expect("chain");
    let next = v["nullifiers"]["nextAnchors"].as_array().expect("nextAnchors");
    assert_eq!(chain.len(), next.len());
    for (link, anchor) in chain.iter().zip(next) {
        assert_eq!(field_to_string(next_anchor(f(&link["nullifier"]))), s(anchor));
    }
}

#[test]
fn a_charge_subtracts_exactly_delta() {
    let v = vectors();
    let cases = v["charged"].as_array().expect("charged");
    assert!(!cases.is_empty());
    for e in cases {
        let c = commit(sc(&e["balance"]), sc(&e["blinding"]));
        let charged = charge(c, Scalar::from(u(&e["delta"])));
        assert_eq!(charged, pt(&e["charged"]));
    }
}

#[test]
fn the_state_is_signed_exactly_as_o1js_signs_it() {
    let v = vectors();
    assert_eq!(s(&v["stateSignatures"]["networkId"]), "devnet");
    let cases = v["stateSignatures"]["cases"].as_array().expect("cases");
    assert!(!cases.is_empty());
    for e in cases {
        let kp = keypair(&e["secret"]);
        let msg = StateMessage::new(&kp.public.into_compressed(), pt(&e["commitment"]), f(&e["anchor"]));
        let fields: Vec<String> = msg.to_fields().into_iter().map(field_to_string).collect();
        let pinned: Vec<&str> = e["fields"].as_array().expect("fields").iter().map(s).collect();
        assert_eq!(fields, pinned);
        assert_eq!(sign_state(&kp, &msg), signature(e));
        assert!(verify_state(&signature(e), &kp.public, &msg));

        // The connector's own field-list signature, which signs every allowance claim, is the
        // same Mina Schnorr under the same domain and nonce: it reproduces o1js's `(r, s)`.
        let flat = msg.to_fields();
        assert_eq!(sign_fields(&kp, &flat), signature(e));
        assert!(verify_fields(&signature(e), &kp.public, &flat));
    }
}

#[test]
fn a_mainnet_signed_state_is_refused() {
    let v = vectors();
    let e = &v["stateSignatures"]["mainnetState"];
    assert_eq!(s(&e["networkId"]), "mainnet");
    let kp = keypair(&e["secret"]);
    let msg = StateMessage::new(&kp.public.into_compressed(), pt(&e["commitment"]), f(&e["anchor"]));
    let sig = signature(e);
    assert!(verify_state_on(NetworkId::MAINNET, &sig, &kp.public, &msg));
    assert!(!verify_state(&sig, &kp.public, &msg));
    assert!(!verify_fields(&sig, &kp.public, &msg.to_fields()));
}

#[test]
fn the_request_time_window_is_zk_toons() {
    let v = vectors();
    let cases = v["requestTimes"].as_array().expect("requestTimes");
    assert!(!cases.is_empty());
    for e in cases {
        assert_eq!(
            policy::is_request_time_fresh(u(&e["requestTime"]), u(&e["now"])),
            e["fresh"].as_bool().expect("a bool"),
        );
    }
}
