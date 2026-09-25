//! `POST /ilp/batch-settlement/solana/open`: the public Solana sponsor
//! endpoint (ADR 0074 decision 9, issue #1346; `client-edge-spec.md` §1.11).
//!
//! A client that holds USDC and no SOL posts the payer-signed `open` it
//! built from this node's greeting, and this node co-signs it as fee payer
//! and `rent_payer`, submits it, and answers with the channel it made.
//!
//! **Public, and not an operator write.** The channel does not exist yet, so
//! nothing can pay for this call, and a buyer this node has never heard of
//! must be able to make it (ADR 0052). It is therefore mounted on the client
//! edge's listener beside `/ilp/claim-state`, with no RFC 9421 signature
//! (ADR 0008 governs `[operator] write_keys`, which a buyer does not hold).
//! What bounds it is what it will sign -- every rule is
//! [`connector_settlement_solana::batch::sponsor`]'s, and this module decides
//! none of them -- plus two limits of its own: a body no larger than a
//! transaction needs, and at most [`CONCURRENT_SPONSORSHIPS`] in flight, so
//! a flood of well-formed opens cannot queue unbounded simulations against
//! the settlement RPC endpoint.
//!
//! **Off unless configured.** Always mounted, so that a node without
//! `[settlement.solana.batch_settlement]` refuses by name
//! (`batch_settlement_not_offered`) rather than with a bare 404 a client
//! cannot tell from a wrong URL.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use connector_settlement_solana::batch::sponsor::{
    RefusalClass, SponsorRefusal, MAX_TRANSACTION_BYTES,
};
use connector_settlement_solana::batch::SolanaBatchSettlement;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::Semaphore;

/// Where the endpoint is served.
pub const SPONSOR_PATH: &str = "/ilp/batch-settlement/solana/open";

/// How many sponsorships may be in flight at once. Each holds a slot from
/// its first chain read to its confirmation -- up to a blockhash's lifetime
/// -- so this bounds the RPC work the public surface can cause, not the
/// rate at which channels open.
pub const CONCURRENT_SPONSORSHIPS: usize = 8;

/// The largest body read: a transaction's base64 (four bytes per three)
/// plus room for the JSON around it.
const BODY_LIMIT: usize = MAX_TRANSACTION_BYTES.div_ceil(3) * 4 + 1024;

/// The request: the payer-signed transaction, base64 of its wire bytes --
/// exactly what a stock x402 client's `buildOpenPaymentChannelTransaction`
/// returns as `transaction` (x402's `deposit.transaction`). Unknown fields
/// are ignored, so a client may post the rest of its x402 payload beside it.
#[derive(Deserialize)]
struct SponsorRequest {
    transaction: String,
}

/// The sponsor this node runs, if any.
struct Sponsor {
    backend: Arc<SolanaBatchSettlement>,
    min_sponsored_deposit: u64,
    in_flight: Semaphore,
}

/// The endpoint's router. `sponsor` is this node's Solana batch-settlement
/// backend and its `min_sponsored_deposit`, or `None` for a node that has
/// not opted in on Solana.
pub fn router(sponsor: Option<(Arc<SolanaBatchSettlement>, u64)>) -> Router {
    let state = Arc::new(sponsor.map(|(backend, min_sponsored_deposit)| Sponsor {
        backend,
        min_sponsored_deposit,
        in_flight: Semaphore::new(CONCURRENT_SPONSORSHIPS),
    }));
    Router::new()
        .route(SPONSOR_PATH, post(sponsor_open))
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
        .with_state(state)
}

async fn sponsor_open(State(sponsor): State<Arc<Option<Sponsor>>>, body: Bytes) -> Response {
    let Some(sponsor) = sponsor.as_ref() else {
        return refusal(
            StatusCode::NOT_FOUND,
            "batch_settlement_not_offered",
            "this node does not accept x402 batch-settlement channels on Solana \
             ([settlement.solana.batch_settlement] is not configured), so it sponsors no open"
                .to_string(),
        );
    };
    let request: SponsorRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(error) => {
            return refusal(
                StatusCode::BAD_REQUEST,
                "request_malformed",
                format!("expected {{\"transaction\": \"<base64>\"}}: {error}"),
            )
        }
    };
    let Ok(_slot) = sponsor.in_flight.try_acquire() else {
        return refusal(
            StatusCode::SERVICE_UNAVAILABLE,
            "sponsor_busy",
            format!("{CONCURRENT_SPONSORSHIPS} sponsorships are already in flight; retry shortly"),
        );
    };
    match sponsor
        .backend
        .sponsor_open(&request.transaction, sponsor.min_sponsored_deposit)
        .await
    {
        Ok(opened) => {
            tracing::info!(
                channel = %opened.channel,
                payer = %opened.payer,
                deposit = opened.deposit,
                transaction = %opened.signature,
                "sponsored an x402 batch-settlement open"
            );
            (
                StatusCode::OK,
                Json(json!({
                    "channelId": opened.channel.to_string(),
                    "transaction": opened.signature.to_string(),
                    "payer": opened.payer.to_string(),
                    "deposit": opened.deposit.to_string(),
                })),
            )
                .into_response()
        }
        Err(error) => {
            tracing::info!(refusal = error.name(), %error, "refused to sponsor an open");
            refusal(status_of(&error), error.name(), error.to_string())
        }
    }
}

/// A refusal's HTTP status: its [`RefusalClass`].
fn status_of(error: &SponsorRefusal) -> StatusCode {
    match error.class() {
        RefusalClass::Malformed => StatusCode::BAD_REQUEST,
        RefusalClass::Refused => StatusCode::UNPROCESSABLE_ENTITY,
        RefusalClass::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        RefusalClass::Failed => StatusCode::BAD_GATEWAY,
    }
}

fn refusal(status: StatusCode, name: &str, detail: String) -> Response {
    (status, Json(json!({ "error": name, "detail": detail }))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    async fn post_to(app: Router, body: &'static str) -> (StatusCode, serde_json::Value) {
        let response = app
            .oneshot(
                Request::post(SPONSOR_PATH)
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .expect("request"),
            )
            .await
            .expect("response");
        let status = response.status();
        let bytes = hyper::body::to_bytes(response.into_body())
            .await
            .expect("body");
        (status, serde_json::from_slice(&bytes).expect("JSON"))
    }

    /// ADR 0074 decision 1: off unless configured, and refused by name.
    #[tokio::test]
    async fn a_node_that_has_not_opted_in_refuses_by_name() {
        let (status, body) = post_to(router(None), r#"{"transaction":"AA=="}"#).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"], "batch_settlement_not_offered");
    }

    #[tokio::test]
    async fn a_body_larger_than_any_transaction_is_not_read() {
        let app = router(None);
        let response = app
            .oneshot(
                Request::post(SPONSOR_PATH)
                    .body(Body::from(vec![b'a'; BODY_LIMIT + 1]))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[test]
    fn each_class_of_refusal_has_its_status() {
        assert_eq!(
            status_of(&SponsorRefusal::NotBase64("x".into())),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            status_of(&SponsorRefusal::PayerSignatureInvalid),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(
            status_of(&SponsorRefusal::ChainUnavailable("x".into())),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            status_of(&SponsorRefusal::SubmissionFailed("x".into())),
            StatusCode::BAD_GATEWAY
        );
    }
}
