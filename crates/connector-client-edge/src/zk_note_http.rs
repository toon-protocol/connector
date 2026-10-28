//! The zk edge's two HTTP endpoints (ADR 0080 decision 8, #1523), on the
//! client edge's listener beside the Solana sponsor's open:
//!
//! - `POST /ilp/zk-note/allowances` opens an allowance from a request
//!   proof, or -- given an `allowanceId` and the session key's signature --
//!   closes one.
//! - `GET /ilp/zk-note/allowances/:id/successor` answers `202` "pending"
//!   until the allowance's charge is journaled, then its one signed
//!   successor, every time. It only reads.
//!
//! There is no BTP frame for either. Both are mounted on every node, and a
//! node that is no zk edge refuses them by name (`zk_note_not_offered`),
//! as the sponsor's endpoint refuses on a node without Solana.
//!
//! The wire (pinned in `vectors/wire-vectors.json`'s `zk_note` section):
//!
//! - open: `{"proof": <o1js JsonProof>, "sessionKey": "B62…",
//!   "expiresAt": "<ms>"}`, answered `{"allowanceId", "tier", "expiresAt"}`;
//! - close: `{"allowanceId", "signature": "<rx ‖ s, 128 hex>"}`, answered as
//!   the successor fetch would be at that moment;
//! - successor: `{"allowanceId", "status": "signed", "charge",
//!   "commitment": {"x", "y"}, "anchor", "signature": {"r", "s"}}`, field
//!   elements and scalars as decimal strings, zk_toon's encoding; or
//!   `{"allowanceId", "status": "pending"}`;
//! - a refusal: `{"error": <rule>, "detail": <words>}`.
//!
//! A log line names an allowance by its opaque id and a refusal by its
//! rule; neither ever carries the proof, a nullifier or a session key.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use connector_domain::client_claim::{decode_lower_hex, is_canonical_decimal, AllowanceId};
use connector_zk_note::note::field_to_string;
use connector_zk_note::CompressedPubKey;
use serde::Deserialize;
use serde_json::json;

use crate::claim_gate::{
    AllowanceClose, AllowanceCloseRefusal, AllowanceRefusal, AllowanceRequest, ClientClaimGate,
    NotAZkEdge, SuccessorStatus,
};

/// Where allowances are opened and closed; the successor is at
/// `<this>/:id/successor`. The greeting names this path from this constant.
pub const ZK_NOTE_ALLOWANCES_PATH: &str = "/ilp/zk-note/allowances";

/// The largest body read: an o1js `JsonProof` for one recursive proof is
/// tens of kilobytes of base64, so this leaves room without inviting
/// megabytes.
const BODY_LIMIT: usize = 256 * 1024;

/// An open request. `proof` is kept as its raw bytes, so the evidence file
/// records exactly what the payer sent.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpenBody<'a> {
    #[serde(borrow)]
    proof: &'a serde_json::value::RawValue,
    session_key: String,
    expires_at: String,
}

/// A close request.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CloseBody {
    allowance_id: String,
    signature: String,
}

/// The router serving both endpoints over `gate`.
pub fn zk_note_router(gate: Arc<ClientClaimGate>) -> Router {
    Router::new()
        .route(ZK_NOTE_ALLOWANCES_PATH, post(allowances))
        .route(
            &format!("{ZK_NOTE_ALLOWANCES_PATH}/:id/successor"),
            get(successor),
        )
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
        .with_state(gate)
}

async fn allowances(State(gate): State<Arc<ClientClaimGate>>, body: Bytes) -> Response {
    if !gate.is_zk_edge() {
        return not_a_zk_edge();
    }
    let value: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(error) => return malformed(format!("the body is not JSON: {error}")),
    };
    if value.get("allowanceId").is_some() {
        return close(&gate, &body);
    }
    let open: OpenBody<'_> = match serde_json::from_slice(&body) {
        Ok(open) => open,
        Err(error) => {
            return malformed(format!(
                "expected {{\"proof\", \"sessionKey\", \"expiresAt\"}} to open, or \
                 {{\"allowanceId\", \"signature\"}} to close: {error}"
            ))
        }
    };
    let Ok(session_key) = CompressedPubKey::from_address(&open.session_key) else {
        return malformed("'sessionKey' is not a Mina public key".to_string());
    };
    let Some(expires_at_ms) = is_canonical_decimal(&open.expires_at)
        .then(|| open.expires_at.parse::<u64>().ok())
        .flatten()
    else {
        return malformed("'expiresAt' must be a decimal string of milliseconds".to_string());
    };
    let request = AllowanceRequest {
        proof: open.proof.get().as_bytes().to_vec(),
        session_key,
        expires_at_ms,
    };
    match gate.admit_allowance(request).await {
        Ok(grant) => (
            StatusCode::OK,
            Json(json!({
                "allowanceId": grant.allowance_id.to_string(),
                "tier": grant.tier.to_string(),
                "expiresAt": grant.expires_at_ms.to_string(),
            })),
        )
            .into_response(),
        Err(refusal) => {
            tracing::info!(refusal = refusal.name(), "refused a zk-note allowance");
            let status = match refusal {
                AllowanceRefusal::NotAZkEdge => StatusCode::NOT_FOUND,
                AllowanceRefusal::Malformed(_) => StatusCode::BAD_REQUEST,
                AllowanceRefusal::VaultUnavailable(_) | AllowanceRefusal::NotDurable => {
                    StatusCode::SERVICE_UNAVAILABLE
                }
                _ => StatusCode::UNPROCESSABLE_ENTITY,
            };
            refusal_body(status, refusal.name(), refusal.to_string())
        }
    }
}

fn close(gate: &ClientClaimGate, body: &[u8]) -> Response {
    let request: CloseBody = match serde_json::from_slice(body) {
        Ok(request) => request,
        Err(error) => {
            return malformed(format!(
                "expected {{\"allowanceId\", \"signature\"}} to close: {error}"
            ))
        }
    };
    let Ok(allowance_id) = request.allowance_id.parse::<AllowanceId>() else {
        return malformed("'allowanceId' must be 32 lowercase hex characters".to_string());
    };
    let Some(signature) = decode_lower_hex::<64>(&request.signature) else {
        return malformed("'signature' must be 128 lowercase hex characters".to_string());
    };
    match gate.close_allowance(AllowanceClose {
        allowance_id,
        signature,
    }) {
        Ok(status) => successor_response(allowance_id, status),
        Err(refusal) => close_refused(refusal),
    }
}

async fn successor(State(gate): State<Arc<ClientClaimGate>>, Path(id): Path<String>) -> Response {
    if !gate.is_zk_edge() {
        return not_a_zk_edge();
    }
    let Ok(allowance_id) = id.parse::<AllowanceId>() else {
        return malformed("the allowance id must be 32 lowercase hex characters".to_string());
    };
    match gate.successor(allowance_id) {
        Ok(status) => successor_response(allowance_id, status),
        Err(refusal) => close_refused(refusal),
    }
}

/// A successor as the wire carries it: `200` and the signed state, or
/// `202` and "pending".
fn successor_response(allowance_id: AllowanceId, status: SuccessorStatus) -> Response {
    match status {
        SuccessorStatus::Pending => (
            StatusCode::ACCEPTED,
            Json(json!({
                "allowanceId": allowance_id.to_string(),
                "status": "pending",
            })),
        )
            .into_response(),
        SuccessorStatus::Signed(successor) => {
            (StatusCode::OK, Json(successor_json(&successor))).into_response()
        }
    }
}

/// The signed successor's JSON: what both the endpoint and the vectors
/// spell it as.
pub fn successor_json(successor: &crate::SignedSuccessor) -> serde_json::Value {
    json!({
        "allowanceId": successor.allowance_id.to_string(),
        "status": "signed",
        "charge": successor.charge.to_string(),
        "commitment": {
            "x": field_to_string(successor.commitment.x),
            "y": field_to_string(successor.commitment.y),
        },
        "anchor": field_to_string(successor.anchor),
        "signature": {
            "r": field_to_string(successor.signature.rx),
            "s": connector_zk_note::scalar_to_string(successor.signature.s),
        },
    })
}

/// Both endpoints' answer on a node that is no zk edge.
fn not_a_zk_edge() -> Response {
    refusal_body(
        StatusCode::NOT_FOUND,
        NotAZkEdge::NAME,
        NotAZkEdge.to_string(),
    )
}

fn close_refused(refusal: AllowanceCloseRefusal) -> Response {
    let status = match refusal {
        AllowanceCloseRefusal::NotAZkEdge | AllowanceCloseRefusal::UnknownAllowance => {
            StatusCode::NOT_FOUND
        }
        AllowanceCloseRefusal::SignatureInvalid => StatusCode::UNPROCESSABLE_ENTITY,
        AllowanceCloseRefusal::NotDurable => StatusCode::SERVICE_UNAVAILABLE,
    };
    refusal_body(status, refusal.name(), refusal.to_string())
}

fn malformed(detail: String) -> Response {
    refusal_body(StatusCode::BAD_REQUEST, "request_malformed", detail)
}

fn refusal_body(status: StatusCode, name: &str, detail: String) -> Response {
    (status, Json(json!({ "error": name, "detail": detail }))).into_response()
}
