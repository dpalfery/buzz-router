//! The HTTP API (design section 8, DD-22, R42, R43).
//!
//! Two axum routers share one [`ApiState`]:
//!
//! - [`loopback_router`], served on `api_bind`: the wake-token routes and the admin routes;
//! - [`tailnet_router`], served on `tailnet_bind` when it is set: the wake-token routes only.
//!
//! Every other path, admin paths on the tailnet included, gets 404 `not_found`. Handlers
//! authenticate nothing about wakes themselves: they send an [`ApiRequest`] to the core and
//! answer with what it returns, so every state change stays serialised in the core actor.
//!
//! [`ApiRequest`]: crate::core::ApiRequest

mod admin;
pub mod admin_token;
mod error;
mod token;

use std::collections::BTreeSet;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::rejection::BytesRejection;
use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post};
use axum::Router;
use router_core::config::Roster;
use router_core::ids::BotName;

use crate::core::CoreHandle;
pub use error::{ApiError, HttpError};

/// The largest request body accepted, in bytes (design section 8).
pub const BODY_LIMIT: usize = 70 * 1024;
/// The longest `text` accepted by `/v1/post` and `/v1/eta`, in bytes (R37.6).
pub const MAX_TEXT_BYTES: usize = 64 * 1024;

/// What the handlers share.
#[derive(Clone)]
pub struct ApiState {
    core: CoreHandle,
    admin_token: Arc<str>,
    roster_bots: Arc<BTreeSet<BotName>>,
}

impl ApiState {
    /// State over `core`, accepting `admin_token` on the admin routes and naming only `roster`'s
    /// bots in admin controls.
    pub fn new(core: CoreHandle, admin_token: String, roster: &Roster) -> Self {
        Self {
            core,
            admin_token: admin_token.into(),
            roster_bots: Arc::new(roster.bots.keys().cloned().collect()),
        }
    }
}

/// The router for `api_bind`: wake-token and admin routes.
pub fn loopback_router(state: ApiState) -> Router {
    token_routes()
        .route("/v1/status", get(admin::status))
        .route("/v1/stop", post(admin::stop))
        .route("/v1/resume", post(admin::resume))
        .route("/v1/cancel", post(admin::cancel))
        .fallback(not_found)
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
        .with_state(state)
}

/// The router for `tailnet_bind`: wake-token routes only (R43.3).
pub fn tailnet_router(state: ApiState) -> Router {
    token_routes()
        .fallback(not_found)
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
        .with_state(state)
}

fn token_routes() -> Router<ApiState> {
    Router::new()
        .route("/v1/post", post(token::post))
        .route("/v1/pass", post(token::pass))
        .route("/v1/eta", post(token::eta))
}

async fn not_found() -> HttpError {
    HttpError::not_found()
}

/// The body, or 400 when it can't be read, for example when it is over [`BODY_LIMIT`].
fn body_bytes(body: Result<Bytes, BytesRejection>) -> Result<Bytes, HttpError> {
    body.map_err(|rejection| HttpError::bad_request(rejection.body_text()))
}

/// The bearer token of a request, if it has one.
fn bearer(headers: &axum::http::HeaderMap) -> Option<&str> {
    headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::trim)
        .filter(|token| !token.is_empty())
}
