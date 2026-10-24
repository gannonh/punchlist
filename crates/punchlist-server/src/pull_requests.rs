//! `/api/issues/{id}/pull-requests`, `/api/pull-requests` and `/api/pull-requests/unlinked`:
//! the pull requests that GitHub's events recorded, read by issue, as the open ones, or as
//! the ones no issue claimed.
//! `/api/issues/{id}/pull-requests/refresh` reads GitHub first, and adds what the reviewers
//! wrote; `/api/issues/{id}/review-threads` answers what they wrote as last recorded.

use axum::Json;
use axum::extract::{Path, State};
use chrono::{DateTime, Utc};
use punchlist_api::{
    ChecksState, ErrorBody, OpenPullRequest, PullRequest, PullRequestIssue, PullRequestState,
    PullRequestsNow, ReviewThread, plain_pull_request_url,
};
use punchlist_core::{Status, display_name};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::actors::person_only;
use crate::auth::Actor;
use crate::github::{newest_check_runs, refresh_pull_requests};
use crate::workflow::active_workflow;
use crate::{ApiError, AppState};

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(issue_pull_requests))
        .routes(routes!(refresh_issue_pull_requests))
        .routes(routes!(issue_review_threads))
        .routes(routes!(unlinked_pull_requests))
        .routes(routes!(open_pull_requests))
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
            check_runs: Vec::new(),
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
    Ok(Json(recorded(&mut conn, &actor, &id).await?))
}

/// An issue's pull requests and the unresolved review threads of the open ones, after
/// reading both from GitHub as a gated request does: a new review thread sends no webhook,
/// so this is how an agent reads what its reviewers wrote. The thread text is from outside
/// the team. When GitHub cannot be read, or there is no GitHub App, `refreshed` is false
/// and the answer is what is recorded.
#[utoipa::path(
    post,
    path = "/api/issues/{id}/pull-requests/refresh",
    params(("id" = String, Path, description = "Issue id, such as PL-1")),
    responses(
        (status = 200, body = PullRequestsNow),
        (status = 401, body = ErrorBody),
        (status = 404, body = ErrorBody),
    )
)]
async fn refresh_issue_pull_requests(
    State(state): State<AppState>,
    actor: Actor,
    Path(id): Path<String>,
) -> Result<Json<PullRequestsNow>, ApiError> {
    // The issue is checked before GitHub is read, and the connection is not held meanwhile.
    recorded(&mut *state.pool.acquire().await?, &actor, &id).await?;
    let refreshed = match &state.github {
        Some(github) => refresh_pull_requests(github, &state.pool, actor.workspace_id, &id)
            .await
            .inspect_err(|error| tracing::warn!(issue_id = id, "not refreshed: {error}"))
            .is_ok(),
        None => false,
    };
    let mut conn = state.pool.acquire().await?;
    Ok(Json(PullRequestsNow {
        pull_requests: recorded(&mut conn, &actor, &id).await?,
        review_threads: open_review_threads(&mut conn, &id).await?,
        refreshed,
    }))
}

/// The unresolved review threads of an issue's open pull requests, as last recorded: a thread
/// event, a gated request and `get_issue` record them. Reads nothing from GitHub. The thread
/// text is from outside the team.
#[utoipa::path(
    get,
    path = "/api/issues/{id}/review-threads",
    params(("id" = String, Path, description = "Issue id, such as PL-1")),
    responses(
        (status = 200, body = Vec<ReviewThread>),
        (status = 401, body = ErrorBody),
        (status = 404, body = ErrorBody),
    )
)]
async fn issue_review_threads(
    State(state): State<AppState>,
    actor: Actor,
    Path(id): Path<String>,
) -> Result<Json<Vec<ReviewThread>>, ApiError> {
    let mut conn = state.pool.acquire().await?;
    issue_exists(&mut conn, &actor, &id).await?;
    Ok(Json(open_review_threads(&mut conn, &id).await?))
}

/// Only an open pull request's threads (draft included): those are what `no_open_threads`
/// reads. Newest pull request first, then by place.
async fn open_review_threads(
    conn: &mut sqlx::PgConnection,
    id: &str,
) -> sqlx::Result<Vec<ReviewThread>> {
    let threads = sqlx::query!(
        r#"SELECT t.node_id, p.number, t.path, t.line, t.url,
                  coalesce(t.comments, '[]') AS "comments!",
                  coalesce(t.comments_total, 0) AS "comments_total!"
           FROM review_thread t JOIN pull_request p ON p.id = t.pull_request_id
           WHERE p.issue_id = $1 AND p.state = 'open' AND NOT t.resolved
           ORDER BY p.number DESC, t.path, t.line, t.node_id"#,
        id,
    )
    .fetch_all(&mut *conn)
    .await?;
    let threads = threads.into_iter().map(|row| {
        Ok(ReviewThread {
            id: row.node_id,
            pull_request: row.number,
            path: row.path,
            line: row.line,
            comments: serde_json::from_value(row.comments)
                .map_err(|error| sqlx::Error::Decode(error.into()))?,
            comments_total: row.comments_total,
            url: row.url,
        })
    });
    threads.collect()
}

/// A missing issue, or one of another workspace, is `IssueNotFound`.
async fn issue_exists(
    conn: &mut sqlx::PgConnection,
    actor: &Actor,
    id: &str,
) -> Result<(), ApiError> {
    let exists = sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM issue WHERE id = $1 AND workspace_id = $2) AS "exists!""#,
        id,
        actor.workspace_id,
    )
    .fetch_one(&mut *conn)
    .await?;
    if !exists {
        return Err(ApiError::IssueNotFound(id.to_string()));
    }
    Ok(())
}

/// The recorded pull requests of an issue of the actor's workspace, newest number first.
async fn recorded(
    conn: &mut sqlx::PgConnection,
    actor: &Actor,
    id: &str,
) -> Result<Vec<PullRequest>, ApiError> {
    issue_exists(conn, actor, id).await?;
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
    let mut pull_requests: Vec<PullRequest> = rows
        .into_iter()
        .map(PullRequest::try_from)
        .collect::<Result<_, _>>()?;
    let heads = sqlx::query!(
        "SELECT number, repository_id, head_sha FROM pull_request WHERE issue_id = $1",
        id,
    )
    .fetch_all(&mut *conn)
    .await?;
    for pull_request in &mut pull_requests {
        if let Some(head) = heads.iter().find(|head| head.number == pull_request.number) {
            pull_request.check_runs =
                newest_check_runs(conn, head.repository_id, &head.head_sha).await?;
        }
    }
    Ok(pull_requests)
}

/// The workspace repository's pull requests that name no issue, newest number first.
#[utoipa::path(
    get,
    path = "/api/pull-requests/unlinked",
    responses((status = 200, body = Vec<PullRequest>), (status = 401, body = ErrorBody))
)]
pub(crate) async fn unlinked_pull_requests(
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

/// The workspace repository's open and draft pull requests, newest number first, each with
/// the issue it names and whether that issue waits on its merge. Reads no GitHub. People
/// only.
#[utoipa::path(
    get,
    path = "/api/pull-requests",
    responses(
        (status = 200, body = Vec<OpenPullRequest>),
        (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody, description = "Not a person's token."),
    )
)]
pub(crate) async fn open_pull_requests(
    State(state): State<AppState>,
    actor: Actor,
) -> Result<Json<Vec<OpenPullRequest>>, ApiError> {
    person_only(&actor)?;
    let mut conn = state.pool.acquire().await?;
    let (workflow, _) = active_workflow(&mut conn, actor.workspace_id).await?;
    let rows = sqlx::query!(
        r#"SELECT p.number, p.title, p.branch, p.url, p.draft, p.merge_state, p.checks_passed,
                  p.checks_total, p.open_threads, p.updated_at, i.id AS "issue_id?",
                  i.title AS "issue_title?", i.status AS "issue_status?"
           FROM pull_request p
           JOIN repository r ON r.id = p.repository_id
           LEFT JOIN issue i ON i.id = p.issue_id
           WHERE r.workspace_id = $1 AND p.state = 'open'
           ORDER BY p.number DESC"#,
        actor.workspace_id,
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|row| {
                let issue = match (row.issue_id, row.issue_title, row.issue_status) {
                    (Some(id), Some(title), Some(status)) => Some(PullRequestIssue {
                        id,
                        title,
                        status_name: display_name(&status),
                        on_merge_to: workflow.on_merge_to(&status).map(Status::display_name),
                        status,
                    }),
                    _ => None,
                };
                let waits_on_merge = !row.draft
                    && issue
                        .as_ref()
                        .is_some_and(|issue| issue.on_merge_to.is_some());
                OpenPullRequest {
                    url: plain_pull_request_url(row.number, &row.url).map(str::to_string),
                    number: row.number,
                    title: row.title,
                    branch: row.branch,
                    draft: row.draft,
                    merge_state: row.merge_state,
                    checks_passed: row.checks_passed,
                    checks_total: row.checks_total,
                    open_threads: row.open_threads,
                    updated_at: row.updated_at,
                    issue,
                    waits_on_merge,
                }
            })
            .collect(),
    ))
}
