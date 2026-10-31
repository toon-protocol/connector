//! Schema 10 (ADR 0080 decision 12, #1523): the zk edge's own wire -- the
//! allowance request and answer, the allowance claim and its signed bytes,
//! the close request and its signed bytes, and the signed successor with
//! its five fields and `(r, s)` -- computed through zk_toon's `zk-note`
//! crate at the commit [`ZK_TOON_COMMIT`] names.
//!
//! Only the connector's wire is pinned here. zk_toon's derived values (its
//! tags, generators, anchors, commitments) are zk_toon's
//! `test-vectors/zk-note.json`'s to pin, and are inputs here, not entries.
//! No asset or decimals key appears: ADR 0071 keeps this file unit-silent,
//! and the asset is stated in the greeting.

use connector_client_edge::{successor_json, SignedSuccessor, ZK_NOTE_ALLOWANCES_PATH};
use connector_domain::client_claim::{AllowanceId, SCHEME_ZK_NOTE};
use connector_domain::x402::{terms_body, GreetingTerms, X402ZkNoteTerms, ZkNoteSubsidy};
use connector_domain::{NodeFacts, Price};
use connector_signer::ZkStateSigner;
use connector_zk_note::note::balance::commit;
use connector_zk_note::note::field_to_string;
use connector_zk_note::note::policy::TIERS;
use connector_zk_note::request::{expected_tag, json_proof_for, point, RequestPublic};
use connector_zk_note::session::{
    allowance_claim_fields, allowance_close_fields, sign_allowance_claim, sign_allowance_close,
    signature_to_hex,
};
use connector_zk_note::successor::successor_state;
use connector_zk_note::{scalar_to_string, Field, Keypair, Scalar, SecKey, Signature};
use serde::Serialize;
use serde_json::{json, Value};

/// The zk_toon commit every value in this section was generated at: the
/// `rev` `crates/connector-zk-note/Cargo.toml` pins.
pub const ZK_TOON_COMMIT: &str = "0d6b10c332558caf7d8d6350e14b19d68f443f2c";

#[derive(Debug, Serialize)]
pub struct ZkNoteVectors {
    pub zk_toon_commit: &'static str,
    /// How every signature here is made: Mina kimchi Schnorr, the devnet
    /// network id (domain string `CodaSignature`) and the chunked nonce, on
    /// every network.
    pub signing: Value,
    /// The fixtures' keys: secrets as decimal scalars, public keys as `B62…`.
    pub keys: Value,
    pub allowance_request: Value,
    pub allowance_answer: Value,
    pub allowance_claim: Value,
    pub allowance_close: Value,
    pub successor: Value,
    /// A subsidised allowance's request (ADR 0080 decision 14, #1581).
    pub subsidised_allowance_request: Value,
    /// The successor of a subsidised allowance: the note is charged 0.
    pub subsidised_successor: Value,
    /// The greeting's zk-note `extra` as it reads while a subsidy is open,
    /// closed, and not offered at all.
    pub greeting_subsidy: Value,
}

fn keypair(secret: u64) -> Keypair {
    Keypair::try_from(SecKey::new(Scalar::from(secret))).expect("a fixture keypair")
}

fn signature_json(signature: &Signature) -> Value {
    json!({
        "r": field_to_string(signature.rx),
        "s": scalar_to_string(signature.s),
        "hex": signature_to_hex(signature),
    })
}

fn fields_json(fields: &[Field]) -> Vec<String> {
    fields.iter().copied().map(field_to_string).collect()
}

pub fn generate_zk_note_vectors() -> ZkNoteVectors {
    const SESSION_SECRET: u64 = 0x5e55_1011;
    const STATE_SECRET: u64 = 0x57a7_e001;
    const REQUEST_TIME_MS: u64 = 1_793_188_800_000;
    const EXPIRES_AT_MS: u64 = REQUEST_TIME_MS + 1_800_000;
    const BALANCE: u64 = 5_000_000_000;
    const BLINDING: u64 = 0x0123_4567_89ab_cdef;
    const CHARGE: u64 = 250_000;

    let session = keypair(SESSION_SECRET);
    let state = ZkStateSigner::from_base58(&SecKey::new(Scalar::from(STATE_SECRET)).to_base58())
        .expect("a fixture state key");
    let session_key = session.public.into_compressed();
    let state_key = state.public_key();
    let tier = TIERS[1];
    let nullifier = Field::from(0x6e75_6c6c_6966_6965u64);
    let commitment = commit(Scalar::from(BALANCE), Scalar::from(BLINDING));
    let allowance_id = AllowanceId::from_bytes([
        0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e,
        0x1f,
    ]);

    // The request: a JsonProof whose public input is zk_toon's request
    // proof's, bound by its tag to this edge, session key, tier and expiry.
    // Its `proof` is a placeholder: the o1js proof fixture is #1514's.
    let public = RequestPublic {
        vault_x: state_key.x,
        vault_is_odd: state_key.is_odd,
        root: Field::from(0x726f_6f74u64),
        request_time: REQUEST_TIME_MS,
        tier,
        nullifier,
        tag: expected_tag(nullifier, &state_key, &session_key, tier, EXPIRES_AT_MS),
        commitment: point(commitment),
        subsidised: false,
        period: 0,
        period_nullifier: Field::from(0u64),
    };
    let proof: Value = serde_json::from_slice(&json_proof_for(&public, "placeholder"))
        .expect("a JsonProof is JSON");
    let allowance_request = json!({
        "method": "POST",
        "path": ZK_NOTE_ALLOWANCES_PATH,
        "body": {
            "proof": proof,
            "sessionKey": session_key.into_address(),
            "expiresAt": EXPIRES_AT_MS.to_string(),
        },
        "public_input_order": [
            "vault.x", "vault.isOdd", "root", "requestTime", "tier", "nullifier", "tag",
            "commitment.x", "commitment.y", "subsidised", "period", "periodNullifier"
        ],
        "note": "the proof string is a placeholder; a request proof o1js made is #1514's fixture",
    });
    let allowance_answer = json!({
        "status": 200,
        "body": {
            "allowanceId": allowance_id.to_string(),
            "tier": tier.to_string(),
            "expiresAt": EXPIRES_AT_MS.to_string(),
        },
    });

    let claim_signature = sign_allowance_claim(&session, allowance_id.as_bytes(), CHARGE);
    let allowance_claim = json!({
        "claim": {
            "version": "1.0",
            "scheme": SCHEME_ZK_NOTE,
            "allowanceId": allowance_id.to_string(),
            "cumulativeAmount": CHARGE.to_string(),
            "signature": signature_to_hex(&claim_signature),
        },
        "signed_fields": fields_json(&allowance_claim_fields(allowance_id.as_bytes(), CHARGE)),
        "signature": signature_json(&claim_signature),
    });

    let close_signature = sign_allowance_close(&session, allowance_id.as_bytes());
    let allowance_close = json!({
        "method": "POST",
        "path": ZK_NOTE_ALLOWANCES_PATH,
        "body": {
            "allowanceId": allowance_id.to_string(),
            "signature": signature_to_hex(&close_signature),
        },
        "signed_fields": fields_json(&allowance_close_fields(allowance_id.as_bytes())),
        "signature": signature_json(&close_signature),
    });

    let next =
        successor_state(&state_key, commitment, nullifier, CHARGE).expect("a signable successor");
    let signed = SignedSuccessor {
        allowance_id,
        charge: CHARGE,
        state_key: state_key.clone(),
        commitment: next.commitment,
        anchor: next.anchor,
        signature: state.sign(&next.message),
    };
    let successor = json!({
        "path": format!("{ZK_NOTE_ALLOWANCES_PATH}/{allowance_id}/successor"),
        "nullifier": field_to_string(nullifier),
        "commitment_before": {
            "x": field_to_string(commitment.x),
            "y": field_to_string(commitment.y),
        },
        "charge": CHARGE.to_string(),
        "signed_fields": fields_json(&next.message.to_fields()),
        "signature": signature_json(&signed.signature),
        "answer": { "status": 200, "body": successor_json(&signed) },
        "pending": {
            "status": 202,
            "body": { "allowanceId": allowance_id.to_string(), "status": "pending" },
        },
    });

    // A subsidised allowance (decision 14): the smallest tier, the period the
    // edge is in, and a per-period nullifier. The proof states all three
    // publicly. Its successor charges the note nothing: the commitment
    // carries over unchanged and only the anchor moves.
    const SUBSIDY_PERIOD: u64 = 11;
    let subsidised_tier = TIERS[0];
    let subsidised_nullifier = Field::from(0x0073_7562_7369_6479_u64);
    let subsidised_public = RequestPublic {
        tier: subsidised_tier,
        nullifier: subsidised_nullifier,
        tag: expected_tag(
            subsidised_nullifier,
            &state_key,
            &session_key,
            subsidised_tier,
            EXPIRES_AT_MS,
        ),
        subsidised: true,
        period: SUBSIDY_PERIOD,
        period_nullifier: Field::from(0x7065_7269_6f64u64),
        ..public
    };
    let subsidised_proof: Value =
        serde_json::from_slice(&json_proof_for(&subsidised_public, "placeholder"))
            .expect("a JsonProof is JSON");
    let subsidised_allowance_request = json!({
        "method": "POST",
        "path": ZK_NOTE_ALLOWANCES_PATH,
        "body": {
            "proof": subsidised_proof,
            "sessionKey": session_key.into_address(),
            "expiresAt": EXPIRES_AT_MS.to_string(),
        },
        "subsidy_period": SUBSIDY_PERIOD.to_string(),
        "refusals": {
            "tier_above_the_smallest": "subsidy_tier_not_smallest",
            "period_not_the_current_one": "subsidy_period_not_current",
            "budget_closed": "subsidy_budget_closed",
            "per_period_nullifier_seen": "subsidy_nullifier_spent",
        },
        "note": "the proof string is a placeholder; the period is zk_toon's policy::subsidy_period",
    });
    let subsidised_next = successor_state(&state_key, commitment, subsidised_nullifier, 0)
        .expect("a signable successor");
    assert_eq!(
        subsidised_next.commitment, commitment,
        "a zero charge leaves the balance commitment as it was"
    );
    let subsidised_signed = SignedSuccessor {
        allowance_id,
        charge: 0,
        state_key: state_key.clone(),
        commitment: subsidised_next.commitment,
        anchor: subsidised_next.anchor,
        signature: state.sign(&subsidised_next.message),
    };
    let subsidised_successor = json!({
        "path": format!("{ZK_NOTE_ALLOWANCES_PATH}/{allowance_id}/successor"),
        "nullifier": field_to_string(subsidised_nullifier),
        "commitment_before": {
            "x": field_to_string(commitment.x),
            "y": field_to_string(commitment.y),
        },
        "charge": "0",
        "signed_fields": fields_json(&subsidised_next.message.to_fields()),
        "signature": signature_json(&subsidised_signed.signature),
        "answer": { "status": 200, "body": successor_json(&subsidised_signed) },
        "note": "the note is charged 0; the allowance's final watermark is the subsidy budget's",
    });
    let greeting_extra = |subsidy: Option<ZkNoteSubsidy>| -> Value {
        let terms = X402ZkNoteTerms {
            network: "mina:devnet".into(),
            asset: "slip44:12586".into(),
            pay_to: state_key.into_address(),
            verification_key_hash: "1".into(),
            state_key: state_key.into_address(),
            tiers: TIERS.iter().map(u64::to_string).collect(),
            allowance_lifetime_secs: 1_800,
            allowance_endpoint: ZK_NOTE_ALLOWANCES_PATH.into(),
            subsidy,
            subsidised_last_period: None,
        };
        let facts = NodeFacts {
            zk_note: Some(terms),
            ..NodeFacts::default()
        };
        let body = terms_body(&GreetingTerms {
            destination: "g.vector",
            price: Price::flat(1),
            node: Some(&facts),
            ..GreetingTerms::default()
        });
        let greeting: Value = serde_json::from_slice(&body).expect("a greeting is JSON");
        greeting["accepts"][0]["extra"].clone()
    };
    let greeting_subsidy = json!({
        "open": greeting_extra(Some(ZkNoteSubsidy::Open)),
        "closed": greeting_extra(Some(ZkNoteSubsidy::Closed)),
        "not_offered": greeting_extra(None),
        "note": "subsidy is open or closed, computed when the greeting is served, and absent \
                 while the node's subsidy budget is zero; it never says how much remains",
    });

    ZkNoteVectors {
        zk_toon_commit: ZK_TOON_COMMIT,
        signing: json!({
            "scheme": "mina-kimchi-schnorr",
            "network_id": "devnet",
            "domain_string": "CodaSignature",
            "nonce": "chunked",
        }),
        keys: json!({
            "session_secret": SESSION_SECRET.to_string(),
            "session_key": session_key.into_address(),
            "state_secret": STATE_SECRET.to_string(),
            "state_key": state_key.into_address(),
        }),
        allowance_request,
        allowance_answer,
        allowance_claim,
        allowance_close,
        successor,
        subsidised_allowance_request,
        subsidised_successor,
        greeting_subsidy,
    }
}
