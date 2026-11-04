//! The vault's actions as an archive node lists them, and the hash chain that
//! proves the list is the whole of it (ADR 0080 decision 15).
//!
//! The vault keeps only the root of its nullifier set, so "is this nullifier
//! spent on chain" is answered by its actions: a nullifier in any close or
//! escape action on the canonical chain is used, whether or not the reduce has
//! applied it yet. A lagging or lying archive would report a spent nullifier
//! as unused, so the list is checked against the account's own latest action
//! state: Mina folds every applied action into that state, and this module
//! recomputes the fold. A list that doesn't reach the account's state is not
//! evidence of anything.
//!
//! Pure: no I/O, no clock. The hashing is Mina's `Actions` / sequence-state
//! derivation, from `zkapp_account.ml` (the same one mina-rust ports):
//!
//! ```text
//! event_hash  = H["MinaZkappEvent"](fields)
//! actions_hash = foldr(empty = N("MinaZkappActionsEmpty"),
//!                      acc, e -> H["MinaZkappSeqEvents"](acc, event_hash(e)))
//! state'       = H["MinaZkappSeqEvents"](state, actions_hash)
//! first state  = N("MinaZkappActionStateEmptyElt")
//! ```
//!
//! where `H[p]` is Poseidon after absorbing the prefix `p` padded with `*` to
//! 20 bytes, and `N(s)` is the hash of `s` padded with zeros to 32 bytes.

use std::collections::HashSet;

use ark_ff::PrimeField;
use connector_zk_note::note::{field_to_string, field_to_u64};
use connector_zk_note::Field;
use mina_poseidon::constants::PlonkSpongeConstantsKimchi;
use mina_poseidon::pasta::{fp_kimchi, FULL_ROUNDS};
use mina_poseidon::poseidon::{ArithmeticSponge, Sponge};
use serde_json::Value;

/// The fields of a vault action (`VaultAction` in zk_toon's `vault.ts`):
/// kind, commitment, nullifier, next nullifier, amount, size, time,
/// valid-until, destination (x and parity), conceded.
pub const ACTION_FIELDS: usize = 11;
/// Where an action's nullifier sits.
const NULLIFIER_AT: usize = 2;
/// `ActionKind.mutualClose`.
const KIND_MUTUAL_CLOSE: u64 = 2;
/// `ActionKind.escape`.
const KIND_ESCAPE: u64 = 3;

fn sponge() -> ArithmeticSponge<Field, PlonkSpongeConstantsKimchi, FULL_ROUNDS> {
    ArithmeticSponge::new(fp_kimchi::static_params())
}

fn poseidon(inputs: &[Field]) -> Field {
    let mut sponge = sponge();
    sponge.absorb(inputs);
    sponge.squeeze()
}

/// A prefix as a field element: the bytes, padded with `*` to 20, then zeros.
fn prefix_field(prefix: &str) -> Field {
    let mut bytes = [0u8; 32];
    bytes[..20].fill(b'*');
    bytes[..prefix.len()].copy_from_slice(prefix.as_bytes());
    Field::from_le_bytes_mod_order(&bytes)
}

/// A salt with no inputs: the string padded with zeros to 32 bytes, hashed.
fn salt_field(salt: &str) -> Field {
    let mut bytes = [0u8; 32];
    bytes[..salt.len()].copy_from_slice(salt.as_bytes());
    poseidon(&[Field::from_le_bytes_mod_order(&bytes)])
}

/// Poseidon continued from the state after absorbing `prefix`: o1js'
/// `hashWithPrefix`.
fn hash_with_prefix(prefix: &str, inputs: &[Field]) -> Field {
    let mut sponge = sponge();
    sponge.absorb(&[prefix_field(prefix)]);
    sponge.squeeze();
    sponge.absorb(inputs);
    sponge.squeeze()
}

/// The action state of an account that has never had an action.
#[must_use]
pub fn empty_action_state() -> Field {
    salt_field("MinaZkappActionStateEmptyElt")
}

/// The hash of one account update's actions, in the order the archive lists
/// them.
#[must_use]
pub fn actions_hash(actions: &[Vec<Field>]) -> Field {
    actions
        .iter()
        .rev()
        .fold(salt_field("MinaZkappActionsEmpty"), |acc, action| {
            let event = hash_with_prefix("MinaZkappEvent", action);
            hash_with_prefix("MinaZkappSeqEvents", &[acc, event])
        })
}

/// The action state after one account update's actions are applied to `state`.
#[must_use]
pub fn action_state_after(state: Field, actions: &[Vec<Field>]) -> Field {
    hash_with_prefix("MinaZkappSeqEvents", &[state, actions_hash(actions)])
}

/// Where a block stands, as the archive says (`chainStatus`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChainStatus {
    /// Final, or on the best chain and not yet final. Both are the canonical
    /// chain as far as a zk edge can know it.
    Canonical,
    /// Off the best chain: its actions never happened.
    Orphaned,
}

/// One account update's actions, with the archive's account of where they
/// sit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    height: u64,
    chain: ChainStatus,
    applied: bool,
    /// The sequence state before and after this update, as the archive states
    /// them (`actionStateTwo`, `actionStateOne`).
    before: Field,
    after: Field,
    actions: Vec<Vec<Field>>,
}

fn field_of(value: &Value, what: &str) -> Result<Field, String> {
    let text = value
        .as_str()
        .ok_or_else(|| format!("{what} is not a decimal string"))?;
    let field: Field = text
        .parse()
        .map_err(|_| format!("{what} '{text}' is not a field element"))?;
    if field_to_string(field) != text {
        return Err(format!("{what} '{text}' is not canonical"));
    }
    Ok(field)
}

/// The `actions` of an archive's `VaultActions` answer.
///
/// # Errors
/// A description of the first thing in the answer this reading cannot
/// believe: the caller reports the read unavailable.
pub fn read_entries(data: &Value) -> Result<Vec<Entry>, String> {
    let list = data
        .get("actions")
        .and_then(Value::as_array)
        .ok_or("the archive's answer has no 'actions' list")?;
    let mut entries = Vec::with_capacity(list.len());
    for item in list {
        let height = item["blockInfo"]["height"]
            .as_u64()
            .ok_or("an action's block has no height")?;
        let chain = match item["blockInfo"]["chainStatus"].as_str() {
            Some("canonical" | "pending") => ChainStatus::Canonical,
            Some("orphaned") => ChainStatus::Orphaned,
            other => return Err(format!("an action's block has chainStatus {other:?}")),
        };
        let after = field_of(&item["actionState"]["actionStateOne"], "actionStateOne")?;
        let before = field_of(&item["actionState"]["actionStateTwo"], "actionStateTwo")?;
        let data = item["actionData"]
            .as_array()
            .ok_or("an action entry has no 'actionData' list")?;
        let mut statuses = HashSet::new();
        let mut actions = Vec::with_capacity(data.len());
        for action in data {
            statuses.insert(action["transactionInfo"]["status"].as_str() == Some("applied"));
            let fields = action["data"]
                .as_array()
                .ok_or("an action has no 'data' list")?
                .iter()
                .map(|f| field_of(f, "an action field"))
                .collect::<Result<Vec<_>, _>>()?;
            actions.push(fields);
        }
        // One account update lives in one transaction: an entry that is
        // partly applied is not a thing, and is not guessed at.
        if statuses.len() > 1 {
            return Err("an action entry is applied in part".to_string());
        }
        entries.push(Entry {
            height,
            chain,
            applied: statuses.into_iter().next().unwrap_or(true),
            before,
            after,
            actions,
        });
    }
    Ok(entries)
}

/// What the archive's list proves: the account states it passes through, and
/// the nullifiers its closes and escapes use.
#[derive(Debug)]
pub struct Replay {
    states: HashSet<Field>,
    spent: HashSet<Field>,
}

impl Replay {
    /// Whether `state` is a state the list passes through, the empty state
    /// included.
    #[must_use]
    pub fn reaches(&self, state: Field) -> bool {
        state == empty_action_state() || self.states.contains(&state)
    }

    /// Whether a close or an escape on the canonical chain uses `nullifier`.
    #[must_use]
    pub fn spent(&self, nullifier: &Field) -> bool {
        self.spent.contains(nullifier)
    }
}

/// Replay `entries` from the empty state, in block order, skipping what never
/// happened (an orphaned block, a failed transaction), and checking every
/// step against the states the archive states for it.
///
/// # Errors
/// A description of the first step that does not follow from the one before:
/// an archive that is incomplete or inconsistent is not read around.
pub fn replay(mut entries: Vec<Entry>) -> Result<Replay, String> {
    // Stable: entries of one block keep the order the archive lists them in.
    entries.sort_by_key(|entry| entry.height);
    let mut state = empty_action_state();
    let mut states = HashSet::new();
    let mut spent = HashSet::new();
    for entry in entries
        .iter()
        .filter(|e| e.chain == ChainStatus::Canonical && e.applied)
    {
        if entry.before != state {
            return Err(format!(
                "the archive's action at height {} follows state {}, not {}: the list is \
                 incomplete or out of order",
                entry.height,
                field_to_string(entry.before),
                field_to_string(state)
            ));
        }
        let next = action_state_after(state, &entry.actions);
        if next != entry.after {
            return Err(format!(
                "the actions at height {} do not hash to the state the archive states for them",
                entry.height
            ));
        }
        state = next;
        states.insert(state);
        for action in &entry.actions {
            if action.len() != ACTION_FIELDS {
                return Err(format!(
                    "an action at height {} has {} fields, not {ACTION_FIELDS}",
                    entry.height,
                    action.len()
                ));
            }
            if matches!(
                field_to_u64(action[0]),
                Some(KIND_MUTUAL_CLOSE | KIND_ESCAPE)
            ) {
                spent.insert(action[NULLIFIER_AT]);
            }
        }
    }
    Ok(Replay { states, spent })
}
