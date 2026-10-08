//! API errors: the HTTP error body (DD-22) and the admin-token file errors.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

use crate::core::ApiFailure;

/// Why the admin token could not be read or created.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    /// The file could not be read or written.
    #[error("admin token file {path}: {source}")]
    Io {
        /// The file.
        path: std::path::PathBuf,
        /// The cause.
        source: std::io::Error,
    },
    /// No random bytes were available.
    #[error("cannot generate the admin token: {0}")]
    Random(String),
}

/// An HTTP error: a status and the body `{"error": <code>, "message": <detail>}` (DD-22).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl HttpError {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    /// 401: no token, or a token that authorises nothing here (A13).
    pub fn unauthorized() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "a valid bearer token is required",
        )
    }

    /// 400: the request is malformed.
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "bad_request", message)
    }

    /// 404: no such route here.
    pub fn not_found() -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", "no such route")
    }

    /// 501: the route exists but isn't served yet.
    pub fn not_implemented() -> Self {
        Self::new(
            StatusCode::NOT_IMPLEMENTED,
            "not_implemented",
            "not implemented",
        )
    }
}

impl From<ApiFailure> for HttpError {
    fn from(failure: ApiFailure) -> Self {
        let (status, code) = match &failure {
            ApiFailure::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized"),
            ApiFailure::Halted => (StatusCode::LOCKED, "halted"),
            ApiFailure::WakeEnded => (StatusCode::GONE, "wake_ended"),
            ApiFailure::TooManyPosts => (StatusCode::TOO_MANY_REQUESTS, "too_many_posts"),
            ApiFailure::PublishFailed(_) => (StatusCode::BAD_GATEWAY, "publish_failed"),
            ApiFailure::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
        };
        Self::new(status, code, failure.to_string())
    }
}

impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        let body = serde_json::json!({ "error": self.code, "message": self.message });
        (self.status, Json(body)).into_response()
    }
}
