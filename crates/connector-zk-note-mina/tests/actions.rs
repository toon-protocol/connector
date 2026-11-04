//! The action-state hash chain, over what a real archive node answered for a
//! real vault (zk_toon#67's recording): the Rust fold reproduces the states
//! Mina computed, and so a list that doesn't reach the account's state can be
//! told from one that does.

use connector_zk_note::Field;
use connector_zk_note_mina::actions::{empty_action_state, read_entries, replay};
use serde_json::{json, Value};

fn recording() -> Value {
    serde_json::from_str(include_str!("../fixtures/recorded-vault.json")).expect("the recording")
}

fn field(text: &str) -> Field {
    text.parse().expect("a field element")
}

fn archive() -> Value {
    recording()["actions"]["data"].clone()
}

fn account_state() -> Field {
    field(
        recording()["vaultState"]["data"]["account"]["actionState"][0]
            .as_str()
            .expect("the newest state"),
    )
}

#[test]
fn the_empty_state_is_the_one_a_fresh_vault_holds() {
    // A fresh vault's `actionState` in zk_toon's vault-check vectors.
    assert_eq!(
        empty_action_state(),
        field("25079927036070901246064867767436987657692091363973573142121686150614948079097")
    );
}

#[test]
fn the_fold_reproduces_every_state_the_chain_computed_and_reaches_the_accounts() {
    let replay = replay(read_entries(&archive()).expect("entries")).expect("a consistent list");
    assert!(replay.reaches(account_state()));
    assert!(replay.reaches(empty_action_state()));
    assert!(!replay.reaches(field("12345")));
}

#[test]
fn a_nullifier_in_an_escape_is_used_and_one_in_no_action_is_not() {
    let rec = recording();
    let replay = replay(read_entries(&archive()).expect("entries")).expect("replays");
    assert!(replay.spent(&field(rec["named"]["consumedNullifier"].as_str().unwrap())));
    assert!(!replay.spent(&field(rec["named"]["absentNullifier"].as_str().unwrap())));
    // A deposit's nullifier slot is zero, and a deposit spends nothing.
    assert!(!replay.spent(&Field::from(0u64)));
}

#[test]
fn an_archive_that_is_behind_the_account_does_not_reach_its_state() {
    let mut data = archive();
    data["actions"].as_array_mut().unwrap().pop();
    let replay = replay(read_entries(&data).unwrap()).expect("a prefix is a consistent list");
    assert!(
        !replay.reaches(account_state()),
        "the dropped escape is exactly what the stale list would miss"
    );
}

#[test]
fn a_list_with_a_hole_is_refused() {
    let mut data = archive();
    data["actions"].as_array_mut().unwrap().remove(1);
    let error = replay(read_entries(&data).unwrap()).unwrap_err();
    assert!(error.contains("incomplete or out of order"), "{error}");
}

#[test]
fn actions_that_do_not_hash_to_their_stated_state_are_refused() {
    let mut data = archive();
    data["actions"][2]["actionData"][0]["data"][2] = json!("1");
    let error = replay(read_entries(&data).unwrap()).unwrap_err();
    assert!(error.contains("do not hash"), "{error}");
}

#[test]
fn orphaned_blocks_and_failed_transactions_never_happened() {
    let rec = recording();
    let consumed = field(rec["named"]["consumedNullifier"].as_str().unwrap());
    for edit in [
        |data: &mut Value| data["actions"][2]["blockInfo"]["chainStatus"] = json!("orphaned"),
        |data: &mut Value| {
            data["actions"][2]["actionData"][0]["transactionInfo"]["status"] = json!("failed");
        },
    ] {
        let mut data = archive();
        edit(&mut data);
        let replay = replay(read_entries(&data).unwrap()).expect("the rest still replays");
        assert!(!replay.spent(&consumed));
        // The account has the escape; a list without it falls short.
        assert!(!replay.reaches(account_state()));
    }
}

#[test]
fn a_canonical_block_counts_as_much_as_a_pending_one() {
    let mut data = archive();
    for action in data["actions"].as_array_mut().unwrap() {
        action["blockInfo"]["chainStatus"] = json!("canonical");
    }
    let replay = replay(read_entries(&data).unwrap()).unwrap();
    assert!(replay.reaches(account_state()));
}

#[test]
fn an_answer_that_is_not_a_list_of_actions_is_refused() {
    for data in [
        json!({}),
        json!({"actions": "no"}),
        json!({"actions": [{}]}),
    ] {
        assert!(read_entries(&data).is_err(), "{data}");
    }
}
