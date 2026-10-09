use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use punchlist_api::ErrorBody;
use punchlist_core::Refusal;
use uuid::Uuid;

/// An error the API returns, with its status and a stable code.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("missing or unknown bearer token")]
    Unauthorized,
    #[error("issue {0} not found")]
    IssueNotFound(String),
    #[error("{0}")]
    Forbidden(String),
    #[error("run {0} not found")]
    RunNotFound(Uuid),
    #[error("runner {0} not found")]
    RunnerNotFound(Uuid),
    #[error("{0}")]
    ClaimTaken(String),
    #[error("{0}")]
    StaleAttempt(String),
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Refused(#[from] Refusal),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

impl ApiError {
    fn status(&self) -> StatusCode {
        match self {
            ApiError::Unauthorized => StatusCode::UNAUTHORIZED,
            ApiError::IssueNotFound(_) | ApiError::RunNotFound(_) | ApiError::RunnerNotFound(_) => {
                StatusCode::NOT_FOUND
            }
            ApiError::Forbidden(_) => StatusCode::FORBIDDEN,
            ApiError::ClaimTaken(_) | ApiError::StaleAttempt(_) => StatusCode::CONFLICT,
            ApiError::Invalid(_) => StatusCode::UNPROCESSABLE_ENTITY,
            ApiError::Refused(_) => StatusCode::CONFLICT,
            ApiError::Database(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn code(&self) -> &'static str {
        match self {
            ApiError::Unauthorized => "unauthorized",
            ApiError::IssueNotFound(_) | ApiError::RunNotFound(_) | ApiError::RunnerNotFound(_) => {
                "not_found"
            }
            ApiError::Forbidden(_) => "forbidden",
            ApiError::ClaimTaken(_) => "claim_taken",
            ApiError::StaleAttempt(_) => "stale_attempt",
            ApiError::Invalid(_) => "invalid",
            ApiError::Refused(refusal) => refusal.code(),
            ApiError::Database(_) => "internal",
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let message = match &self {
            ApiError::Database(error) => {
                tracing::error!(%error, "database error");
                "internal error".to_string()
            }
            other => other.to_string(),
        };
        let body = ErrorBody {
            code: self.code().to_string(),
            message,
        };
        (self.status(), Json(body)).into_response()
    }
}
