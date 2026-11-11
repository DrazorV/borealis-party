//! The server's error type. On the wire it has the same `{ code, message }` shape as the Borealis commands.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

/// A failed request.
#[derive(Debug)]
pub enum ApiError {
    /// 400.
    Invalid(String),
    /// 401: missing, expired or wrong credentials.
    Unauthorized(String),
    /// 403.
    Forbidden(String),
    /// 403 with its own code, for the client to act on: an account that may not sign in yet, say.
    Refused(&'static str, String),
    /// 404.
    NotFound(String),
    /// 409.
    Conflict(String),
    /// 429.
    TooMany,
    /// 500: logged, never shown.
    Internal(String),
    /// A Borealis command refused: its own `{ code, message, details? }`.
    Command(Value),
}

/// Result of a request handler.
pub type ApiResult<T> = Result<T, ApiError>;

impl ApiError {
    /// A 404 with the usual wording.
    pub fn group_not_found() -> Self {
        Self::NotFound("That group does not exist.".into())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, body) = match self {
            Self::Invalid(m) => (
                StatusCode::BAD_REQUEST,
                json!({ "code": "invalid_argument", "message": m }),
            ),
            Self::Unauthorized(m) => (
                StatusCode::UNAUTHORIZED,
                json!({ "code": "unauthorized", "message": m }),
            ),
            Self::Forbidden(m) => (
                StatusCode::FORBIDDEN,
                json!({ "code": "forbidden", "message": m }),
            ),
            Self::Refused(code, m) => {
                (StatusCode::FORBIDDEN, json!({ "code": code, "message": m }))
            }
            Self::NotFound(m) => (
                StatusCode::NOT_FOUND,
                json!({ "code": "not_found", "message": m }),
            ),
            Self::Conflict(m) => (
                StatusCode::CONFLICT,
                json!({ "code": "conflict", "message": m }),
            ),
            Self::TooMany => (
                StatusCode::TOO_MANY_REQUESTS,
                json!({ "code": "too_many_requests", "message": "Too many attempts. Wait a few minutes and try again." }),
            ),
            Self::Internal(detail) => {
                tracing::error!("internal error: {detail}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    json!({ "code": "internal", "message": "Something went wrong on the server." }),
                )
            }
            Self::Command(value) => {
                let status = match value.get("code").and_then(Value::as_str) {
                    Some("not_found") => StatusCode::NOT_FOUND,
                    Some("internal" | "io_error" | "content_error") => {
                        StatusCode::INTERNAL_SERVER_ERROR
                    }
                    _ => StatusCode::UNPROCESSABLE_ENTITY,
                };
                (status, value)
            }
        };
        (status, Json(body)).into_response()
    }
}
