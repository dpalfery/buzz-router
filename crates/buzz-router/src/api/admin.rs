//! The admin routes, loopback only: `/v1/status`, `/v1/stop`, `/v1/resume` and `/v1/cancel`
//! (design sections 6.7 and 8, R33, R43).

use std::collections::BTreeSet;

use axum::body::Bytes;
use axum::extract::rejection::BytesRejection;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::Json;
use router_core::ids::BotName;
use router_core::route::{Control, Scope};
use serde::Deserialize;
use serde_json::{json, Value};
use subtle::ConstantTimeEq;

use super::{bearer, body_bytes, ApiState, HttpError};
use crate::core::{ApiRequest, ApiResponse, Status};

/// The optional body of the control routes.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct BotsBody {
    bots: Option<Vec<String>>,
}

/// `GET /v1/status`.
pub(super) async fn status(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<Status>, HttpError> {
    authorize(&state, &headers)?;
    Ok(Json(state.core.status().await?))
}

/// `POST /v1/stop {bots?}`.
pub(super) async fn stop(
    State(state): State<ApiState>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<Value>, HttpError> {
    control(&state, &headers, body, Control::Stop).await
}

/// `POST /v1/resume {bots?}`.
pub(super) async fn resume(
    State(state): State<ApiState>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<Value>, HttpError> {
    control(&state, &headers, body, Control::Resume).await
}

/// `POST /v1/cancel {bots?}`.
pub(super) async fn cancel(
    State(state): State<ApiState>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<Value>, HttpError> {
    control(&state, &headers, body, Control::Cancel).await
}

/// Authorises, reads the scope, and has the core execute the command: 200
/// `{"scope": "all" | [names]}`.
async fn control(
    state: &ApiState,
    headers: &HeaderMap,
    body: Result<Bytes, BytesRejection>,
    command: fn(Scope) -> Control,
) -> Result<Json<Value>, HttpError> {
    authorize(state, headers)?;
    let scope = scope(state, &body_bytes(body)?)?;
    let shown = match &scope {
        Scope::All => json!("all"),
        Scope::Bots(bots) => json!(bots.iter().map(BotName::as_str).collect::<Vec<_>>()),
    };
    match state.core.api(ApiRequest::Control(command(scope))).await {
        ApiResponse::Done => Ok(Json(json!({ "scope": shown }))),
        ApiResponse::Failed(failure) => Err(failure.into()),
        ApiResponse::Posted { .. } => Err(HttpError::from(crate::core::ApiFailure::Internal(
            "the core gave an unexpected answer".to_owned(),
        ))),
    }
}

/// The admin token, compared in constant time (design section 8).
fn authorize(state: &ApiState, headers: &HeaderMap) -> Result<(), HttpError> {
    let presented = bearer(headers).ok_or_else(HttpError::unauthorized)?;
    if bool::from(presented.as_bytes().ct_eq(state.admin_token.as_bytes())) {
        Ok(())
    } else {
        Err(HttpError::unauthorized())
    }
}

/// The scope a body names: every bot when the body or its `bots` is missing or empty, else the
/// named roster bots. An unknown name is 400.
fn scope(state: &ApiState, body: &[u8]) -> Result<Scope, HttpError> {
    let parsed: BotsBody = if body.iter().all(u8::is_ascii_whitespace) {
        BotsBody::default()
    } else {
        serde_json::from_slice(body).map_err(|error| {
            HttpError::bad_request(format!("expected {{\"bots\": [name, ...]}}: {error}"))
        })?
    };
    let names = parsed.bots.unwrap_or_default();
    if names.is_empty() {
        return Ok(Scope::All);
    }
    let mut bots = BTreeSet::new();
    for name in names {
        let bot = BotName::new(name.clone())
            .ok()
            .filter(|bot| state.roster_bots.contains(bot))
            .ok_or_else(|| HttpError::bad_request(format!("unknown bot {name:?}")))?;
        bots.insert(bot);
    }
    Ok(Scope::Bots(bots))
}
