//! [`MinaZkNoteVault`]: the port over a Mina daemon and an archive node.

use std::sync::Arc;

use connector_chain_rpc::graphql::{GraphqlError, GraphqlRpc};
use connector_client_edge::zk_note::{SlotAnchor, ZkNoteVault, ZkNoteVaultUnavailable};
use connector_zk_note::request::{public_input, RequestProof};
use connector_zk_note::{note::field_to_u64, Field};
use connector_zk_verifier::{Verifier, WrapProof};
use serde_json::{json, Value};
use tokio::sync::Semaphore;

use crate::actions;

/// `VaultState`: the daemon's account state, and the tip's slot.
const VAULT_STATE: &str = "query VaultState($vault: PublicKey!) { account(publicKey: $vault) { \
     zkappState actionState } bestChain(maxLength: 1) { protocolState { consensusState { \
     slotSinceGenesis } } } }";

/// `SlotAnchor`: one best-tip block's `slotSinceGenesis` and that same
/// block's timestamp, from a single read. Never `consensusTimeNow.globalSlot`,
/// which counts from the Mesa fork (ADR 0080 decision 16).
const SLOT_ANCHOR: &str = "query SlotAnchor { bestChain(maxLength: 1) { protocolState { \
     consensusState { slotSinceGenesis } blockchainState { utcDate } } } }";

/// `VaultActions`: every action the vault dispatched, with the block and
/// transaction status of each, from the archive node.
const VAULT_ACTIONS: &str = "query VaultActions($vault: String!) { actions(input: { address: \
     $vault }) { blockInfo { height chainStatus } actionState { actionStateOne actionStateTwo } \
     actionData { data transactionInfo { hash status } } } }";

/// The vault's `zkappState` is 32 fields (`declareState` order in zk_toon's
/// `vault.ts`); these are the ones read.
const STATE_FIELDS: usize = 32;
const NOTE_ROOT: usize = 2;
const ROOT_RING: std::ops::Range<usize> = 4..12;
const CREATION_TIME: usize = 13;
/// The sequence states an account keeps, newest first.
const ACTION_STATES: usize = 5;

/// A Mina vault, read through a daemon and an archive node.
pub struct MinaZkNoteVault {
    vault: String,
    daemon: GraphqlRpc,
    archive: GraphqlRpc,
    verifier: Arc<Verifier>,
    /// One verification is about 110 ms of one core; the blocking pool is
    /// large, and a flood of requests must not take the node's cores.
    verifications: Arc<Semaphore>,
}

impl std::fmt::Debug for MinaZkNoteVault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MinaZkNoteVault")
            .field("vault", &self.vault)
            .field("daemon", &self.daemon.transport().endpoint())
            .field("archive", &self.archive.transport().endpoint())
            .finish_non_exhaustive()
    }
}

fn unavailable(error: impl std::fmt::Display) -> ZkNoteVaultUnavailable {
    ZkNoteVaultUnavailable(error.to_string())
}

/// What a request proof's `JsonProof` says about its public output and proof.
fn read_json_proof(proof: &RequestProof) -> Option<(Vec<String>, String)> {
    let value: Value = serde_json::from_slice(proof.json()).ok()?;
    let output = value
        .get("publicOutput")?
        .as_array()?
        .iter()
        .map(|f| f.as_str().map(str::to_string))
        .collect::<Option<Vec<_>>>()?;
    Some((output, value.get("proof")?.as_str()?.to_string()))
}

impl MinaZkNoteVault {
    /// The adapter over `vault` (a `B62…` address), with the daemon's and the
    /// archive's clients and a verifier built under the request key.
    pub fn new(
        vault: String,
        daemon: GraphqlRpc,
        archive: GraphqlRpc,
        verifier: Arc<Verifier>,
    ) -> MinaZkNoteVault {
        let parallel = std::thread::available_parallelism().map_or(1, |n| n.get());
        MinaZkNoteVault {
            vault,
            daemon,
            archive,
            verifier,
            verifications: Arc::new(Semaphore::new(parallel.clamp(1, 4))),
        }
    }

    /// The vault's `zkappState` and latest action state, off the daemon.
    async fn account(&self) -> Result<Account, ZkNoteVaultUnavailable> {
        let data = self
            .daemon
            .query("VaultState", VAULT_STATE, json!({ "vault": self.vault }))
            .await
            .map_err(unavailable)?;
        Account::read(&data).map_err(unavailable)
    }

    async fn archive_actions(&self) -> Result<Value, GraphqlError> {
        self.archive
            .query(
                "VaultActions",
                VAULT_ACTIONS,
                json!({ "vault": self.vault }),
            )
            .await
    }
}

/// The part of the vault's account this adapter reads.
struct Account {
    app_state: Vec<Field>,
    /// The account's newest sequence state.
    action_state: Field,
}

fn decimal_field(value: &Value, what: &str) -> Result<Field, String> {
    let text = value
        .as_str()
        .ok_or_else(|| format!("{what} is not a decimal string"))?;
    let field: Field = text
        .parse()
        .map_err(|_| format!("{what} '{text}' is not a field element"))?;
    if connector_zk_note::note::field_to_string(field) != text {
        return Err(format!("{what} '{text}' is not canonical"));
    }
    Ok(field)
}

impl Account {
    fn read(data: &Value) -> Result<Account, String> {
        let account = data
            .get("account")
            .filter(|a| !a.is_null())
            .ok_or("the daemon has no account for the vault")?;
        let app_state = account["zkappState"]
            .as_array()
            .ok_or("the vault's account has no zkappState")?
            .iter()
            .map(|f| decimal_field(f, "a zkappState field"))
            .collect::<Result<Vec<_>, _>>()?;
        if app_state.len() != STATE_FIELDS {
            return Err(format!(
                "the vault's zkappState has {} fields, not {STATE_FIELDS}: it is not the vault \
                 this build reads",
                app_state.len()
            ));
        }
        let action_states = account["actionState"]
            .as_array()
            .filter(|states| states.len() == ACTION_STATES)
            .ok_or("the vault's account has no five-state actionState")?;
        Ok(Account {
            app_state,
            action_state: decimal_field(&action_states[0], "the account's actionState")?,
        })
    }

    fn root_known(&self, root: Field) -> bool {
        let zero = Field::from(0u64);
        root != zero
            && (self.app_state[NOTE_ROOT] == root || self.app_state[ROOT_RING].contains(&root))
    }
}

#[async_trait::async_trait]
impl ZkNoteVault for MinaZkNoteVault {
    async fn verify_request_proof(
        &self,
        proof: &RequestProof,
    ) -> Result<bool, ZkNoteVaultUnavailable> {
        // What a payer sent that is not a proof this key could ever accept is
        // `false`, not unavailable: the vault was not the problem.
        let Some((output, proof_text)) = read_json_proof(proof) else {
            return Ok(false);
        };
        if !output.is_empty() {
            return Ok(false);
        }
        let Ok(wrap) = WrapProof::from_o1js_base64(&proof_text) else {
            return Ok(false);
        };
        let public: Vec<Field> = public_input(proof.public());
        let _permit = self
            .verifications
            .acquire()
            .await
            .map_err(|_| unavailable("the verifier is shut down"))?;
        let verifier = Arc::clone(&self.verifier);
        tokio::task::spawn_blocking(move || verifier.verify(&public, &wrap))
            .await
            .map_err(|e| unavailable(format!("verification did not finish: {e}")))
    }

    async fn root_known(&self, root: Field) -> Result<bool, ZkNoteVaultUnavailable> {
        Ok(self.account().await?.root_known(root))
    }

    async fn nullifier_used(&self, nullifier: Field) -> Result<bool, ZkNoteVaultUnavailable> {
        // The account first: an archive that is behind it is a failed read,
        // and one that is ahead of it only knows more.
        let account = self.account().await?;
        let data = self.archive_actions().await.map_err(unavailable)?;
        let entries = actions::read_entries(&data).map_err(unavailable)?;
        let replay = actions::replay(entries).map_err(unavailable)?;
        if !replay.reaches(account.action_state) {
            return Err(unavailable(
                "the archive's actions do not reach the vault's latest action state: the \
                 archive is behind the daemon, or incomplete",
            ));
        }
        Ok(replay.spent(&nullifier))
    }

    async fn creation_slot(&self) -> Result<u64, ZkNoteVaultUnavailable> {
        let account = self.account().await?;
        field_to_u64(account.app_state[CREATION_TIME])
            .ok_or_else(|| unavailable("the vault's creationTime does not fit 64 bits"))
    }

    async fn slot_anchor(&self) -> Result<SlotAnchor, ZkNoteVaultUnavailable> {
        let data = self
            .daemon
            .query("SlotAnchor", SLOT_ANCHOR, json!({}))
            .await
            .map_err(unavailable)?;
        let state = &data["bestChain"][0]["protocolState"];
        let number = |value: &Value, what: &str| -> Result<u64, ZkNoteVaultUnavailable> {
            value
                .as_str()
                .and_then(|text| text.parse().ok())
                .ok_or_else(|| unavailable(format!("the tip's {what} is not a decimal number")))
        };
        Ok(SlotAnchor {
            slot: number(
                &state["consensusState"]["slotSinceGenesis"],
                "slotSinceGenesis",
            )?,
            time_ms: number(&state["blockchainState"]["utcDate"], "utcDate")?,
        })
    }
}
