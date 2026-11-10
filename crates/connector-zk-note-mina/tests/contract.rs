//! The adapter upholds `ZkNoteVault`'s contract suite, over zk_toon's
//! recorded vault served by real HTTP, and verifies the request proofs o1js
//! made under the key the binary carries (zk_toon#65, #67).

mod support;

use std::sync::Arc;

use connector_client_edge::zk_note::contract::{assert_upholds_the_contract, ZkNoteVaultFixture};
use connector_client_edge::zk_note::{SlotAnchor, ZkNoteVault};
use connector_zk_note::request::RequestProof;
use connector_zk_note::Field;
use serde_json::{json, Value};
use support::{proofs, recorded, vault_over, StandIn, ANCHOR_SLOT, ANCHOR_TIME_MS, SHORT};

fn field(text: &str) -> Field {
    text.parse().expect("a field element")
}

fn proof_of(value: &Value) -> RequestProof {
    RequestProof::parse(&serde_json::to_vec(value).unwrap()).expect("a request proof")
}

/// `proof` with its `i`th public input altered: a boolean flipped, any other
/// field raised by one.
fn altered(proof: &Value, i: usize) -> Value {
    let mut out = proof.clone();
    let old = out["publicInput"][i].as_str().unwrap().to_string();
    let new = if matches!(i, 1 | 9) {
        if old == "0" { "1" } else { "0" }.to_string()
    } else {
        connector_zk_note::note::field_to_string(field(&old) + Field::from(1u64))
    };
    out["publicInput"][i] = json!(new);
    out
}

#[tokio::test(flavor = "multi_thread")]
async fn the_adapter_upholds_the_contract() {
    let (daemon, archive) = (StandIn::daemon().await, StandIn::archive().await);
    let vault = vault_over(&daemon, &archive, SHORT).await;
    let (rec, set) = (recorded(), proofs());

    let valid = &set["unsubsidised"]["proof"];
    let subsidised = &set["subsidised"]["proof"];
    // Altered proofs the vault is asked about: the subsidy fields (the suite's
    // own list), and a proof the request key never made.
    let mut invalid = valid.clone();
    invalid["proof"] = set["otherProgram"]["proof"]["proof"].clone();

    let note_root = rec["vaultState"]["data"]["account"]["zkappState"][2]
        .as_str()
        .unwrap();
    assert_upholds_the_contract(ZkNoteVaultFixture {
        vault: Arc::new(vault),
        known_root: field(note_root),
        unknown_root: field(rec["named"]["absentRoot"].as_str().unwrap()),
        used_nullifier: field(rec["named"]["consumedNullifier"].as_str().unwrap()),
        unused_nullifier: field(rec["named"]["absentNullifier"].as_str().unwrap()),
        valid_proof: proof_of(valid),
        invalid_proof: proof_of(&invalid),
        valid_subsidised_proof: proof_of(subsidised),
        subsidy_altered_proofs: [9, 10, 11]
            .into_iter()
            .map(|i| proof_of(&altered(subsidised, i)))
            .collect(),
        creation_slot: connector_zk_note::Slot::new(
            set["vaultCreation"].as_str().unwrap().parse().unwrap(),
        ),
        anchor: SlotAnchor {
            slot: connector_zk_note::Slot::new(ANCHOR_SLOT),
            time_ms: ANCHOR_TIME_MS,
        },
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_proof_with_any_one_public_input_altered_is_refused_for_every_input() {
    let (daemon, archive) = (StandIn::daemon().await, StandIn::archive().await);
    let vault = vault_over(&daemon, &archive, SHORT).await;
    let set = proofs();
    for which in ["unsubsidised", "subsidised"] {
        let original = &set[which]["proof"];
        assert_eq!(
            vault.verify_request_proof(&proof_of(original)).await,
            Ok(true),
            "{which} verifies as made"
        );
        let mut reached_the_verifier = 0;
        for i in 0..12 {
            // An input that no longer reads as a request's (a commitment off
            // the curve) never gets as far as the vault: refused as malformed.
            let Ok(proof) =
                RequestProof::parse(&serde_json::to_vec(&altered(original, i)).unwrap())
            else {
                continue;
            };
            reached_the_verifier += 1;
            assert_eq!(
                vault.verify_request_proof(&proof).await,
                Ok(false),
                "{which} with public input {i} altered must not verify"
            );
        }
        assert!(
            reached_the_verifier >= 9,
            "most inputs are checked by the verifier itself, not only by parsing"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_proof_under_a_different_key_is_refused() {
    let (daemon, archive) = (StandIn::daemon().await, StandIn::archive().await);
    let vault = vault_over(&daemon, &archive, SHORT).await;
    let set = proofs();
    // The withdrawal program's own proof, dressed in a request's public input.
    let mut dressed = set["unsubsidised"]["proof"].clone();
    dressed["proof"] = set["otherProgram"]["proof"]["proof"].clone();
    assert_eq!(
        vault.verify_request_proof(&proof_of(&dressed)).await,
        Ok(false)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn bytes_that_are_not_a_proof_are_refused_not_unavailable() {
    let (daemon, archive) = (StandIn::daemon().await, StandIn::archive().await);
    let vault = vault_over(&daemon, &archive, SHORT).await;
    let mut garbage = proofs()["unsubsidised"]["proof"].clone();
    garbage["proof"] = json!("not base64 at all !!");
    assert_eq!(
        vault.verify_request_proof(&proof_of(&garbage)).await,
        Ok(false)
    );
    let mut with_output = proofs()["unsubsidised"]["proof"].clone();
    with_output["publicOutput"] = json!(["1"]);
    assert_eq!(
        vault.verify_request_proof(&proof_of(&with_output)).await,
        Ok(false)
    );
}
