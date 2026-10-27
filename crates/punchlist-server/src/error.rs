use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use punchlist_api::ErrorBody;
use punchlist_core::{Evidence, GateResult, Refusal};
use uuid::Uuid;

/// An error the API returns, with its status and a stable code.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("missing or unknown bearer token")]
    Unauthorized,
    #[error("the GitHub webhook secret is not configured")]
    GithubNotConfigured,
    /// A gated request's read of GitHub failed, so its gates had no current evidence to
    /// read. Nothing was checked and nothing moved; the same request can succeed later.
    #[error(
        "GitHub could not be read, so the gates for {0} were not checked and it did not move; request again"
    )]
    GithubUnreadable(String),
    /// What a gated request read from GitHub was written again before its gates were judged,
    /// or the request lost a deadlock to such a write. The result is discarded and nothing
    /// moved; the same request can succeed later.
    #[error(
        "what GitHub has for {0} changed while its gates were being checked, so nothing was decided and it did not move; request again"
    )]
    EvidenceChanged(String),
    /// A run's GitHub token could not be minted: the server has no App credentials, or
    /// GitHub refused.
    #[error("cannot mint the run's GitHub token: {0}")]
    GithubTokenUnavailable(String),
    #[error("issue {0} not found")]
    IssueNotFound(String),
    #[error("{0}")]
    Forbidden(String),
    #[error("run {0} not found")]
    RunNotFound(Uuid),
    #[error("runner {0} not found")]
    RunnerNotFound(Uuid),
    #[error("proof {0} not found")]
    ProofNotFound(Uuid),
    #[error("project {0} not found")]
    ProjectNotFound(Uuid),
    #[error("milestone {0} not found in this project")]
    MilestoneNotFound(Uuid),
    #[error("import {0} not found in this workspace")]
    ImportNotFound(i64),
    #[error("workspace {0} not found")]
    WorkspaceNotFound(Uuid),
    #[error("session {0} not found")]
    SessionNotFound(Uuid),
    #[error("no open invite {0} in this workspace")]
    InviteNotFound(Uuid),
    /// No `pl auth login` has this code or secret.
    #[error("{0}")]
    CliLoginNotFound(String),
    /// A person's request without a workspace in its path, from an account in several.
    #[error("this account is in several workspaces ({0}); name one with /api/workspaces/{{id}}/")]
    WorkspaceNotNamed(String),
    #[error("{0}")]
    ClaimTaken(String),
    /// A Run again for an issue that does not wait on a person: something is queued or
    /// running, or nothing would dispatch it.
    #[error("{0} is not waiting on a person")]
    NotWaiting(String),
    #[error("{0}")]
    StaleAttempt(String),
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    Conflict(String),
    /// A feature the server has no configuration for, such as sign-in with GitHub.
    #[error("{0}")]
    NotConfigured(String),
    /// Sign-in with GitHub could not read GitHub, or GitHub refused it.
    #[error("{0}")]
    GithubSignIn(String),
    #[error("{0}")]
    TooLarge(String),
    #[error(transparent)]
    Refused(#[from] Refusal),
    /// A gate failed: the refusal names the first one, with every result and the evidence.
    #[error("{}", .0.refusal)]
    GateFailed(Box<GateFailure>),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    /// Reading or writing a proof file failed.
    #[error(transparent)]
    Storage(#[from] std::io::Error),
}

#[derive(Debug)]
pub struct GateFailure {
    pub refusal: Refusal,
    pub evidence: Evidence,
    pub gates: Vec<GateResult>,
}

impl ApiError {
    fn status(&self) -> StatusCode {
        match self {
            ApiError::Unauthorized => StatusCode::UNAUTHORIZED,
            ApiError::GithubNotConfigured
            | ApiError::GithubUnreadable(_)
            | ApiError::EvidenceChanged(_)
            | ApiError::GithubTokenUnavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            ApiError::IssueNotFound(_)
            | ApiError::RunNotFound(_)
            | ApiError::RunnerNotFound(_)
            | ApiError::ProofNotFound(_)
            | ApiError::ProjectNotFound(_)
            | ApiError::MilestoneNotFound(_)
            | ApiError::ImportNotFound(_)
            | ApiError::WorkspaceNotFound(_)
            | ApiError::SessionNotFound(_)
            | ApiError::InviteNotFound(_)
            | ApiError::CliLoginNotFound(_)
            | ApiError::NotConfigured(_) => StatusCode::NOT_FOUND,
            ApiError::Forbidden(_) => StatusCode::FORBIDDEN,
            ApiError::WorkspaceNotNamed(_) | ApiError::BadRequest(_) => StatusCode::BAD_REQUEST,
            ApiError::Conflict(_) => StatusCode::CONFLICT,
            ApiError::GithubSignIn(_) => StatusCode::BAD_GATEWAY,
            ApiError::ClaimTaken(_) | ApiError::StaleAttempt(_) | ApiError::NotWaiting(_) => {
                StatusCode::CONFLICT
            }
            ApiError::Invalid(_) => StatusCode::UNPROCESSABLE_ENTITY,
            ApiError::TooLarge(_) => StatusCode::PAYLOAD_TOO_LARGE,
            ApiError::Refused(_) | ApiError::GateFailed(_) => StatusCode::CONFLICT,
            ApiError::Database(_) | ApiError::Storage(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn code(&self) -> &'static str {
        match self {
            ApiError::Unauthorized => "unauthorized",
            ApiError::GithubNotConfigured => "github_not_configured",
            ApiError::GithubUnreadable(_) => "github_unreadable",
            ApiError::EvidenceChanged(_) => "evidence_changed",
            ApiError::GithubTokenUnavailable(_) => "github_token_unavailable",
            ApiError::IssueNotFound(_)
            | ApiError::RunNotFound(_)
            | ApiError::RunnerNotFound(_)
            | ApiError::ProofNotFound(_)
            | ApiError::ProjectNotFound(_)
            | ApiError::MilestoneNotFound(_)
            | ApiError::ImportNotFound(_)
            | ApiError::WorkspaceNotFound(_)
            | ApiError::SessionNotFound(_)
            | ApiError::InviteNotFound(_)
            | ApiError::CliLoginNotFound(_)
            | ApiError::NotConfigured(_) => "not_found",
            ApiError::Forbidden(_) => "forbidden",
            ApiError::BadRequest(_) => "bad_request",
            ApiError::Conflict(_) => "conflict",
            ApiError::GithubSignIn(_) => "github_sign_in_failed",
            ApiError::WorkspaceNotNamed(_) => "workspace_not_named",
            ApiError::ClaimTaken(_) => "claim_taken",
            ApiError::StaleAttempt(_) => "stale_attempt",
            ApiError::NotWaiting(_) => "not_waiting",
            ApiError::Invalid(_) => "invalid",
            ApiError::TooLarge(_) => "too_large",
            ApiError::Refused(refusal) => refusal.code(),
            ApiError::GateFailed(failure) => failure.refusal.code(),
            ApiError::Database(_) | ApiError::Storage(_) => "internal",
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.status();
        let message = match &self {
            ApiError::Database(error) => {
                tracing::error!(%error, "database error");
                "internal error".to_string()
            }
            ApiError::Storage(error) => {
                tracing::error!(%error, "proof storage error");
                "internal error".to_string()
            }
            other => other.to_string(),
        };
        let mut body = ErrorBody::new(self.code(), message);
        if let ApiError::GateFailed(failure) = self {
            let GateFailure {
                refusal,
                evidence,
                gates,
            } = *failure;
            if let Refusal::GateFailed { gate, reason, .. } = refusal {
                body.gate = Some(gate);
                body.reason = Some(reason);
            }
            body.evidence = Some(evidence);
            body.gates = gates;
        }
        (status, Json(body)).into_response()
    }
}
