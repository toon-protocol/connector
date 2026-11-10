//! The recorded fixtures and the request key are zk_toon's, byte for byte,
//! pinned by hash the way `connector-settlement-solana/fixtures/` is: a
//! changed file fails here, naming what to look at, instead of drifting.
//!
//! They come from `toon-protocol/zk_toon` `test-vectors/request-proofs/`
//! (zk_toon#65 and #67), byte-identical at `ad4ee440`, the rev
//! `connector-zk-note` pins.
//! The connector can't make them itself: its ADR 0080 forbids o1js.

use sha2::{Digest, Sha256};

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[test]
fn the_recorded_vault_is_the_one_zk_toon_committed() {
    assert_eq!(
        sha256(include_bytes!("../fixtures/recorded-vault.json")),
        "1fa9960e65df69527673563b3cea76c187f5da23d55ca26ef2bee5597ca8a853",
        "fixtures/recorded-vault.json changed: it is zk_toon's recording, not ours to edit"
    );
}

#[test]
fn the_request_proofs_are_the_ones_zk_toon_committed() {
    assert_eq!(
        sha256(include_bytes!("../fixtures/request-proofs.json")),
        "a27adcee0ad6faa1fd5053a94b385b86dce9af5a857f996b6d398fd526aeeae6",
        "fixtures/request-proofs.json changed: it is zk_toon's output, not ours to edit"
    );
}

#[test]
fn the_request_key_the_binary_carries_is_the_one_those_proofs_verify_under() {
    let key = include_bytes!("../key/request-verification-key.json");
    assert_eq!(
        sha256(key),
        "187002cb724fa74c6185c3407e2ffa8f07ce3e8688f84f17c2aec54f5d7b2c85",
        "key/request-verification-key.json changed: a different key is a different build"
    );
    let carried: serde_json::Value = serde_json::from_slice(key).expect("the key file");
    let proofs: serde_json::Value =
        serde_json::from_str(include_str!("../fixtures/request-proofs.json")).expect("proofs");
    assert_eq!(
        carried, proofs["verificationKey"],
        "the carried key is the fixture's verificationKey"
    );
}

#[test]
fn the_carried_key_hashes_to_the_hash_it_is_filed_under() {
    let hash = connector_zk_note_mina::carried_key_hash().expect("the carried key");
    let proofs: serde_json::Value =
        serde_json::from_str(include_str!("../fixtures/request-proofs.json")).expect("proofs");
    assert_eq!(hash, proofs["verificationKey"]["hash"].as_str().unwrap());
}

#[tokio::test]
async fn a_table_naming_another_key_hash_refuses_by_name_before_building_anything() {
    let error = connector_zk_note_mina::request_verifier("12345")
        .await
        .map(|_| ())
        .unwrap_err();
    assert!(
        matches!(
            error,
            connector_zk_note_mina::KeyRefusal::NotTheCarriedKey { ref configured, .. }
                if configured == "12345"
        ),
        "{error}"
    );
    assert!(
        error.to_string().contains("verification_key_hash"),
        "{error}"
    );
}
