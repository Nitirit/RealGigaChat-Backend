use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

/// Every handler returns `Result<T, ApiError>`.
/// This enum covers all the error cases the API can produce.
#[derive(Debug)]
pub enum ApiError {
    /// Something went wrong talking to Supabase.
    Database(String),

    /// The username or password was wrong.
    InvalidCredentials,

    /// No session cookie or the cookie is invalid.
    Unauthorized,

    /// The request body or parameters are invalid.
    BadRequest(String),

    /// A resource (profile, conversation, etc.) was not found.
    NotFound(String),

    /// Catch-all for unexpected internal errors.
    Internal(String),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiError::Database(msg) => write!(f, "Database error: {}", msg),
            ApiError::InvalidCredentials => write!(f, "Invalid username or password"),
            ApiError::Unauthorized => write!(f, "You must be logged in to do that"),
            ApiError::BadRequest(msg) => write!(f, "Bad request: {}", msg),
            ApiError::NotFound(msg) => write!(f, "Not found: {}", msg),
            ApiError::Internal(msg) => write!(f, "Internal error: {}", msg),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            ApiError::Database(_) => (StatusCode::BAD_GATEWAY, self.to_string()),
            ApiError::InvalidCredentials => (StatusCode::UNAUTHORIZED, self.to_string()),
            ApiError::Unauthorized => (StatusCode::UNAUTHORIZED, self.to_string()),
            ApiError::BadRequest(_) => (StatusCode::BAD_REQUEST, self.to_string()),
            ApiError::NotFound(_) => (StatusCode::NOT_FOUND, self.to_string()),
            ApiError::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, self.to_string()),
        };

        let body = json!({ "error": message });
        (status, Json(body)).into_response()
    }
}

// ---------------------------------------------------------------------------
// Convenient conversions so we can use `?` in handlers
// ---------------------------------------------------------------------------

impl From<serde_json::Error> for ApiError {
    fn from(err: serde_json::Error) -> Self {
        ApiError::Internal(format!("JSON error: {}", err))
    }
}

impl From<argon2::password_hash::Error> for ApiError {
    fn from(err: argon2::password_hash::Error) -> Self {
        // Don't leak hashing details to the caller.
        eprintln!("[argon2 error] {}", err);
        ApiError::Internal("Password processing failed".into())
    }
}

impl From<uuid::Error> for ApiError {
    fn from(_: uuid::Error) -> Self {
        ApiError::BadRequest("Invalid UUID format".into())
    }
}
