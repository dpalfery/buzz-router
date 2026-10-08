//! The wake-token routes: `/v1/post`, `/v1/pass` and `/v1/eta` (design section 8, R42).

use axum::body::Bytes;
use axum::extract::rejection::BytesRejection;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{bearer, body_bytes, ApiState, HttpError, MAX_TEXT_BYTES};
use crate::core::{ApiRequest, ApiResponse};

/// The body of `/v1/post` and `/v1/eta`.
#[derive(Deserialize)]
struct TextBody {
    text: String,
}

/// `POST /v1/post {"text"}`: 200 `{"event_id"}`.
pub(super) async fn post(
    State(state): State<ApiState>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<Value>, HttpError> {
    let token = token(&headers)?;
    let text = text(body)?;
    match state.core.api(ApiRequest::Post { token, text }).await {
        ApiResponse::Posted { event_id } => Ok(Json(json!({ "event_id": event_id }))),
        other => Err(failure(other)),
    }
}

/// `POST /v1/pass`: 200 `{}`.
pub(super) async fn pass(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<Value>, HttpError> {
    let token = token(&headers)?;
    done(state.core.api(ApiRequest::Pass { token }).await)
}

/// `POST /v1/eta {"text"}`: 200 `{}`.
pub(super) async fn eta(
    State(state): State<ApiState>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<Value>, HttpError> {
    let token = token(&headers)?;
    let text = text(body)?;
    done(state.core.api(ApiRequest::Eta { token, text }).await)
}

fn token(headers: &HeaderMap) -> Result<String, HttpError> {
    bearer(headers)
        .map(str::to_owned)
        .ok_or_else(HttpError::unauthorized)
}

/// The `text` of a JSON body, at most [`MAX_TEXT_BYTES`] long.
fn text(body: Result<Bytes, BytesRejection>) -> Result<String, HttpError> {
    let bytes = body_bytes(body)?;
    let body: TextBody = serde_json::from_slice(&bytes).map_err(|error| {
        HttpError::bad_request(format!("expected {{\"text\": string}}: {error}"))
    })?;
    if body.text.len() > MAX_TEXT_BYTES {
        return Err(HttpError::bad_request(format!(
            "text is over {MAX_TEXT_BYTES} bytes"
        )));
    }
    Ok(body.text)
}

fn done(response: ApiResponse) -> Result<Json<Value>, HttpError> {
    match response {
        ApiResponse::Done => Ok(Json(json!({}))),
        other => Err(failure(other)),
    }
}

fn failure(response: ApiResponse) -> HttpError {
    match response {
        ApiResponse::Failed(failure) => failure.into(),
        ApiResponse::Posted { .. } | ApiResponse::Done => HttpError::from(
            crate::core::ApiFailure::Internal("the core gave an unexpected answer".to_owned()),
        ),
    }
}
