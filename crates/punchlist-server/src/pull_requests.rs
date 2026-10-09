//! `/api/issues/{id}/pull-requests` and `/api/pull-requests/unlinked`: the pull requests
//! that GitHub's events recorded, read by issue or as the ones no issue claimed.

use axum::Json;
use axum::extract::{Path, State};
use chrono::{DateTime, Utc};
use punchlist_api::{ChecksState, ErrorBody, PullRequest, PullRequestState};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::auth::Actor;
use crate::{ApiError, AppState};

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(issue_pull_requests))
        .routes(routes!(unlinked_pull_requests))
}

struct PullRequestRow {
    number: i64,
    title: String,
    branch: String,
    url: String,
    author_login: String,
    state: String,
    draft: bool,
    merge_state: String,
    checks: String,
    checks_total: i32,
    checks_passed: i32,
    open_threads: i32,
    issue_id: Option<String>,
    updated_at: DateTime<Utc>,
}

impl TryFrom<PullRequestRow> for PullRequest {
    type Error = sqlx::Error;

    fn try_from(row: PullRequestRow) -> Result<PullRequest, sqlx::Error> {
        // The columns' CHECK constraints allow only known values, so an unknown one is
        // corrupt data, not a bad request.
        let state = PullRequestState::parse(&row.state).ok_or_else(|| {
            sqlx::Error::Decode(format!("unknown pull request state `{}`", row.state).into())
        })?;
        let checks = ChecksState::parse(&row.checks).ok_or_else(|| {
            sqlx::Error::Decode(format!("unknown checks state `{}`", row.checks).into())
        })?;
        Ok(PullRequest {
            number: row.number,
            title: row.title,
            branch: row.branch,
            url: row.url,
            author_login: row.author_login,
            state,
            draft: row.draft,
            merge_state: row.merge_state,
            checks,
            checks_total: row.checks_total,
            checks_passed: row.checks_passed,
            open_threads: row.open_threads,
            issue_id: row.issue_id,
            updated_at: row.updated_at,
        })
    }
}

/// An issue's pull requests, newest number first.
#[utoipa::path(
    get,
    path = "/api/issues/{id}/pull-requests",
    params(("id" = String, Path, description = "Issue id, such as PL-1")),
    responses(
        (status = 200, body = Vec<PullRequest>),
        (status = 401, body = ErrorBody),
        (status = 404, body = ErrorBody),
    )
)]
async fn issue_pull_requests(
    State(state): State<AppState>,
    actor: Actor,
    Path(id): Path<String>,
) -> Result<Json<Vec<PullRequest>>, ApiError> {
    let mut conn = state.pool.acquire().await?;
    let exists = sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM issue WHERE id = $1 AND workspace_id = $2) AS "exists!""#,
        id,
        actor.workspace_id,
    )
    .fetch_one(&mut *conn)
    .await?;
    if !exists {
        return Err(ApiError::IssueNotFound(id));
    }
    let rows = sqlx::query_as!(
        PullRequestRow,
        "SELECT number, title, branch, url, author_login, state, draft, merge_state, checks,
                checks_total, checks_passed, open_threads, issue_id, updated_at
         FROM pull_request WHERE issue_id = $1
         ORDER BY number DESC",
        id,
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(PullRequest::try_from)
            .collect::<Result<_, _>>()?,
    ))
}

/// The workspace repository's pull requests that name no issue, newest number first.
#[utoipa::path(
    get,
    path = "/api/pull-requests/unlinked",
    responses((status = 200, body = Vec<PullRequest>), (status = 401, body = ErrorBody))
)]
async fn unlinked_pull_requests(
    State(state): State<AppState>,
    actor: Actor,
) -> Result<Json<Vec<PullRequest>>, ApiError> {
    let rows = sqlx::query_as!(
        PullRequestRow,
        "SELECT p.number, p.title, p.branch, p.url, p.author_login, p.state, p.draft,
                p.merge_state, p.checks, p.checks_total, p.checks_passed, p.open_threads,
                p.issue_id, p.updated_at
         FROM pull_request p
         JOIN repository r ON r.id = p.repository_id
         WHERE r.workspace_id = $1 AND p.issue_id IS NULL
         ORDER BY p.number DESC",
        actor.workspace_id,
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(PullRequest::try_from)
            .collect::<Result<_, _>>()?,
    ))
}
