//! `/api/issues`: create, list, show and move issues, and read their timelines.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use punchlist_api::{
    CreateIssue, ErrorBody, Event, EventDetail, Issue, IssueList, MoveIssue, Moved,
};
use punchlist_core::{default_workflow, display_name};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use crate::auth::{Actor, parse_role};
use crate::{ApiError, AppState};

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(create_issue, list_issues))
        .routes(routes!(get_issue))
        .routes(routes!(move_issue))
        .routes(routes!(issue_events))
}

struct IssueRow {
    id: String,
    title: String,
    body: String,
    status: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl From<IssueRow> for Issue {
    fn from(row: IssueRow) -> Issue {
        Issue {
            status_name: display_name(&row.status),
            id: row.id,
            title: row.title,
            body: row.body,
            status: row.status,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

struct EventRow {
    seq: i64,
    issue_id: String,
    created_at: DateTime<Utc>,
    actor_id: Uuid,
    actor_name: String,
    actor_role: String,
    from_status: String,
    to_status: String,
    workflow_version: String,
}

impl TryFrom<EventRow> for Event {
    type Error = sqlx::Error;

    fn try_from(row: EventRow) -> Result<Event, sqlx::Error> {
        Ok(Event {
            seq: row.seq,
            issue_id: row.issue_id,
            created_at: row.created_at,
            actor: punchlist_api::Actor {
                id: row.actor_id,
                name: row.actor_name,
                role: parse_role(&row.actor_role)?,
            },
            detail: EventDetail::transition(row.from_status, row.to_status, row.workflow_version),
        })
    }
}

/// Create an issue in the workflow's first status.
#[utoipa::path(
    post,
    path = "/api/issues",
    request_body = CreateIssue,
    responses(
        (status = 201, body = Issue),
        (status = 401, body = ErrorBody),
        (status = 422, body = ErrorBody, description = "The title is empty."),
    )
)]
async fn create_issue(
    State(state): State<AppState>,
    actor: Actor,
    Json(request): Json<CreateIssue>,
) -> Result<(StatusCode, Json<Issue>), ApiError> {
    let title = request.title.trim();
    if title.is_empty() {
        return Err(ApiError::Invalid("an issue needs a title".into()));
    }
    let mut tx = state.pool.begin().await?;
    let next = sqlx::query!(
        r#"UPDATE workspace SET next_issue_number = next_issue_number + 1 WHERE id = $1
           RETURNING issue_prefix, next_issue_number - 1 AS "number!""#,
        actor.workspace_id,
    )
    .fetch_one(&mut *tx)
    .await?;
    let row = sqlx::query_as!(
        IssueRow,
        "INSERT INTO issue (id, workspace_id, number, title, body, status, created_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         RETURNING id, title, body, status, created_at, updated_at",
        format!("{}-{}", next.issue_prefix, next.number),
        actor.workspace_id,
        next.number,
        title,
        request.body,
        default_workflow().initial_status().as_str(),
        actor.id,
    )
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(row.into())))
}

/// List the workspace's issues, oldest first.
#[utoipa::path(
    get,
    path = "/api/issues",
    responses((status = 200, body = IssueList), (status = 401, body = ErrorBody))
)]
async fn list_issues(
    State(state): State<AppState>,
    actor: Actor,
) -> Result<Json<IssueList>, ApiError> {
    // One snapshot for the list and the sequence number (ADR 0006).
    let mut tx = state.pool.begin().await?;
    sqlx::query!("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await?;
    let rows = sqlx::query_as!(
        IssueRow,
        "SELECT id, title, body, status, created_at, updated_at
         FROM issue WHERE workspace_id = $1 ORDER BY number",
        actor.workspace_id,
    )
    .fetch_all(&mut *tx)
    .await?;
    let last_event_seq = sqlx::query_scalar!(
        "SELECT last_event_seq FROM workspace WHERE id = $1",
        actor.workspace_id
    )
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(IssueList {
        issues: rows.into_iter().map(Issue::from).collect(),
        last_event_seq,
    }))
}

/// Show one issue.
#[utoipa::path(
    get,
    path = "/api/issues/{id}",
    params(("id" = String, Path, description = "Issue id, such as PL-1")),
    responses(
        (status = 200, body = Issue),
        (status = 401, body = ErrorBody),
        (status = 404, body = ErrorBody),
    )
)]
async fn get_issue(
    State(state): State<AppState>,
    actor: Actor,
    Path(id): Path<String>,
) -> Result<Json<Issue>, ApiError> {
    let row = sqlx::query_as!(
        IssueRow,
        "SELECT id, title, body, status, created_at, updated_at
         FROM issue WHERE id = $1 AND workspace_id = $2",
        id,
        actor.workspace_id,
    )
    .fetch_optional(&state.pool)
    .await?
    .ok_or(ApiError::IssueNotFound(id))?;
    Ok(Json(row.into()))
}

/// Move an issue to another status. The workflow's rules are checked, and the status,
/// the transition and its event are written, in one transaction.
#[utoipa::path(
    post,
    path = "/api/issues/{id}/transitions",
    params(("id" = String, Path, description = "Issue id, such as PL-1")),
    request_body = MoveIssue,
    responses(
        (status = 200, body = Moved),
        (status = 401, body = ErrorBody),
        (status = 404, body = ErrorBody),
        (status = 409, body = ErrorBody, description = "Refused; the message names the rule."),
    )
)]
async fn move_issue(
    State(state): State<AppState>,
    actor: Actor,
    Path(id): Path<String>,
    Json(request): Json<MoveIssue>,
) -> Result<Json<Moved>, ApiError> {
    let workflow = default_workflow();
    let mut tx = state.pool.begin().await?;
    let from = sqlx::query_scalar!(
        "SELECT status FROM issue WHERE id = $1 AND workspace_id = $2 FOR UPDATE",
        id,
        actor.workspace_id,
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::IssueNotFound(id.clone()))?;
    workflow.check_transition(&from, &request.to, actor.role)?;

    let issue = sqlx::query_as!(
        IssueRow,
        "UPDATE issue SET status = $2, updated_at = now() WHERE id = $1
         RETURNING id, title, body, status, created_at, updated_at",
        id,
        request.to,
    )
    .fetch_one(&mut *tx)
    .await?;
    let transition_id = sqlx::query_scalar!(
        "INSERT INTO transition (issue_id, from_status, to_status, actor_id, workflow_version)
         VALUES ($1, $2, $3, $4, $5) RETURNING id",
        id,
        from,
        request.to,
        actor.id,
        workflow.version(),
    )
    .fetch_one(&mut *tx)
    .await?;
    let seq = sqlx::query_scalar!(
        "UPDATE workspace SET last_event_seq = last_event_seq + 1 WHERE id = $1
         RETURNING last_event_seq",
        actor.workspace_id,
    )
    .fetch_one(&mut *tx)
    .await?;
    let created_at = sqlx::query_scalar!(
        "INSERT INTO event (workspace_id, seq, issue_id, kind, actor_id, transition_id)
         VALUES ($1, $2, $3, 'transition', $4, $5) RETURNING created_at",
        actor.workspace_id,
        seq,
        id,
        actor.id,
        transition_id,
    )
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(Moved {
        issue: issue.into(),
        event: Event {
            seq,
            issue_id: id,
            actor: (&actor).into(),
            created_at,
            detail: EventDetail::transition(from, request.to, workflow.version().to_string()),
        },
    }))
}

/// An issue's timeline, oldest first.
#[utoipa::path(
    get,
    path = "/api/issues/{id}/events",
    params(("id" = String, Path, description = "Issue id, such as PL-1")),
    responses(
        (status = 200, body = Vec<Event>),
        (status = 401, body = ErrorBody),
        (status = 404, body = ErrorBody),
    )
)]
async fn issue_events(
    State(state): State<AppState>,
    actor: Actor,
    Path(id): Path<String>,
) -> Result<Json<Vec<Event>>, ApiError> {
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
        EventRow,
        "SELECT e.seq, e.issue_id, e.created_at,
                a.id AS actor_id, a.name AS actor_name, a.role AS actor_role,
                t.from_status, t.to_status, t.workflow_version
         FROM event e
         JOIN actor a ON a.id = e.actor_id
         JOIN transition t ON t.id = e.transition_id
         WHERE e.workspace_id = $1 AND e.issue_id = $2
         ORDER BY e.seq",
        actor.workspace_id,
        id,
    )
    .fetch_all(&mut *conn)
    .await?;
    let events = rows
        .into_iter()
        .map(Event::try_from)
        .collect::<Result<_, _>>()?;
    Ok(Json(events))
}
