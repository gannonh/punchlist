//! `/api/issues`: create, list, show and move issues, and read their timelines.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use punchlist_api::{
    CreateComment, CreateIssue, ErrorBody, Event, EventDetail, Issue, IssueList, MoveIssue, Moved,
};
use punchlist_core::{
    Evidence, GateResult, PullRequestEvidence, PullRequestState, Refusal, Role, Workflow,
    display_name,
};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use crate::auth::{Actor, parse_role};
use crate::error::GateFailure;
use crate::github::{PullRequestData, close_issues, read_pull_requests, record_pull_requests};
use crate::workflow::active_workflow;
use crate::{ApiError, AppState};

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(create_issue, list_issues))
        .routes(routes!(get_issue))
        .routes(routes!(move_issue))
        .routes(routes!(issue_events))
        .routes(routes!(add_comment))
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
    kind: String,
    from_status: Option<String>,
    to_status: Option<String>,
    workflow_version: Option<String>,
    delivery_id: Option<String>,
    comment_body: Option<String>,
}

impl TryFrom<EventRow> for Event {
    type Error = sqlx::Error;

    fn try_from(row: EventRow) -> Result<Event, sqlx::Error> {
        let corrupt = |what: &str| sqlx::Error::Decode(format!("{what} event row").into());
        let detail = match (row.kind.as_str(), row.comment_body) {
            ("comment", Some(body)) => EventDetail::Comment { body },
            ("transition", None) => match (row.from_status, row.to_status, row.workflow_version) {
                (Some(from), Some(to), Some(version)) => EventDetail::Transition {
                    from_name: display_name(&from),
                    to_name: display_name(&to),
                    from,
                    to,
                    workflow_version: version,
                    delivery_id: row.delivery_id,
                },
                _ => return Err(corrupt("incomplete transition")),
            },
            _ => return Err(corrupt("unknown")),
        };
        Ok(Event {
            seq: row.seq,
            issue_id: row.issue_id,
            created_at: row.created_at,
            actor: punchlist_api::Actor {
                id: row.actor_id,
                name: row.actor_name,
                role: parse_role(&row.actor_role)?,
            },
            detail,
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
    let (workflow, _) = active_workflow(&mut tx, actor.workspace_id).await?;
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
        workflow.initial_status().as_str(),
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

/// Move an issue to another status. The workflow's lock, role and gate rules are checked,
/// and the status, the transition, its gate results and its event are written, in one
/// transaction.
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
    let fresh = fresh_evidence(&state, &actor, &id, &request.to).await?;
    let mut tx = state.pool.begin().await?;
    let closed = match &fresh {
        Some((repository_id, fresh)) => {
            record_pull_requests(&mut tx, actor.workspace_id, *repository_id, fresh).await?
        }
        None => Vec::new(),
    };
    let from = sqlx::query_scalar!(
        "SELECT status FROM issue WHERE id = $1 AND workspace_id = $2 FOR UPDATE",
        id,
        actor.workspace_id,
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::IssueNotFound(id.clone()))?;
    let (workflow, _) = active_workflow(&mut tx, actor.workspace_id).await?;
    let moved = match check_move(&mut tx, &workflow, &actor, &id, &from, &request.to).await {
        Ok(gates) => Ok(record_transition(
            &mut tx,
            &actor,
            &id,
            from,
            &request.to,
            None,
            workflow.version(),
            &gates,
        )
        .await?),
        // A failed gate wrote nothing, and what GitHub reported is still recorded.
        Err(refused @ ApiError::GateFailed(_)) => Err(refused),
        Err(other) => return Err(other),
    };
    // After the request, which was made of the issue as it stood: a pull request closed on
    // GitHub fails `pr_open` by name, then moves the issue as its webhook would have.
    close_issues(&mut tx, actor.workspace_id, &closed).await?;
    tx.commit().await?;
    let moved = moved?;
    if request.to == "start" {
        // Wake runners that are long-polling for work. No receivers is fine.
        let _ = state.starts.send((actor.workspace_id, id));
    }
    Ok(Json(moved))
}

/// Before the transaction: if the move has gates, reads the issue's pull requests from
/// GitHub so they decide, not the webhooks still on their way. An issue the actor cannot
/// move, or that does not exist, is left for `move_issue` to refuse.
async fn fresh_evidence(
    state: &AppState,
    actor: &Actor,
    id: &str,
    to: &str,
) -> Result<Option<(Uuid, Vec<PullRequestData>)>, ApiError> {
    let Some(github) = &state.github else {
        return Ok(None);
    };
    let Some(from) = sqlx::query_scalar!(
        "SELECT status FROM issue WHERE id = $1 AND workspace_id = $2",
        id,
        actor.workspace_id,
    )
    .fetch_optional(&state.pool)
    .await?
    else {
        return Ok(None);
    };
    let mut conn = state.pool.acquire().await?;
    let (workflow, _) = active_workflow(&mut conn, actor.workspace_id).await?;
    drop(conn);
    if workflow
        .check_transition(&from, to, actor.role)
        .is_ok_and(|transition| !transition.gates.is_empty())
    {
        return Ok(read_pull_requests(github, &state.pool, actor.workspace_id, id).await?);
    }
    Ok(None)
}

/// Checks a requested move against the workflow: the lock, the role, then each gate over
/// the issue's recorded evidence. Returns the gate results, all passed. The caller holds
/// the issue row's lock, so the evidence and the move are read and written together.
pub(crate) async fn check_move(
    tx: &mut sqlx::PgConnection,
    workflow: &Workflow,
    actor: &Actor,
    id: &str,
    from: &str,
    to: &str,
) -> Result<Vec<GateResult>, ApiError> {
    let transition = workflow.check_transition(from, to, actor.role)?;
    if transition.gates.is_empty() {
        return Ok(Vec::new());
    }
    let evidence = evidence(tx, id).await?;
    let gates: Vec<GateResult> = transition
        .gates
        .iter()
        .map(|gate| gate.evaluate(&evidence))
        .collect();
    if let Some(failed) = gates.iter().find(|result| !result.passed) {
        let refusal = Refusal::GateFailed {
            from: from.to_string(),
            to: to.to_string(),
            gate: failed.gate.clone(),
            reason: failed.reason.clone(),
        };
        tracing::info!(issue = id, actor = %actor.name, "refused: {refusal}");
        return Err(ApiError::GateFailed(Box::new(GateFailure {
            refusal,
            evidence,
            gates,
        })));
    }
    Ok(gates)
}

/// What the gates read: the pull requests linked to the issue.
async fn evidence(tx: &mut sqlx::PgConnection, id: &str) -> sqlx::Result<Evidence> {
    let rows = sqlx::query!(
        "SELECT number, title, branch, state, draft FROM pull_request
         WHERE issue_id = $1 ORDER BY number DESC",
        id,
    )
    .fetch_all(&mut *tx)
    .await?;
    let pull_requests = rows
        .into_iter()
        .map(|row| {
            let state = PullRequestState::parse(&row.state).ok_or_else(|| {
                sqlx::Error::Decode(format!("unknown pull request state `{}`", row.state).into())
            })?;
            Ok(PullRequestEvidence {
                number: row.number,
                title: row.title,
                branch: row.branch,
                state,
                draft: row.draft,
            })
        })
        .collect::<sqlx::Result<_>>()?;
    Ok(Evidence {
        issue_id: id.to_string(),
        pull_requests,
    })
}

/// Writes a transition that the caller has already checked and whose issue row it has
/// locked: the new status, the transition with the workflow version and gate results, and
/// its event. Shared by `move_issue`, claims and GitHub events, which pass the webhook
/// delivery that caused the move.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn record_transition(
    tx: &mut sqlx::PgConnection,
    actor: &Actor,
    id: &str,
    from: String,
    to: &str,
    delivery_id: Option<&str>,
    workflow_version: &str,
    gates: &[GateResult],
) -> Result<Moved, ApiError> {
    write_transition(
        tx,
        actor,
        id,
        from,
        to,
        delivery_id,
        workflow_version,
        gates,
    )
    .await
    .map(|(moved, _)| moved)
}

/// `record_transition`, also returning the new transition's id.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn write_transition(
    tx: &mut sqlx::PgConnection,
    actor: &Actor,
    id: &str,
    from: String,
    to: &str,
    delivery_id: Option<&str>,
    workflow_version: &str,
    gates: &[GateResult],
) -> Result<(Moved, Uuid), ApiError> {
    let issue = sqlx::query_as!(
        IssueRow,
        "UPDATE issue SET status = $2, updated_at = now() WHERE id = $1
         RETURNING id, title, body, status, created_at, updated_at",
        id,
        to,
    )
    .fetch_one(&mut *tx)
    .await?;
    let transition_id = sqlx::query_scalar!(
        "INSERT INTO transition
             (issue_id, from_status, to_status, actor_id, workflow_version, delivery_id)
         VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
        id,
        from,
        to,
        actor.id,
        workflow_version,
        delivery_id,
    )
    .fetch_one(&mut *tx)
    .await?;
    for (position, result) in gates.iter().enumerate() {
        sqlx::query!(
            "INSERT INTO transition_gate (transition_id, position, gate, result, reason)
             VALUES ($1, $2, $3, $4, $5)",
            transition_id,
            position as i32,
            result.gate,
            if result.passed { "pass" } else { "fail" },
            result.reason,
        )
        .execute(&mut *tx)
        .await?;
    }
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
    let mut detail = EventDetail::transition(from, to.to_string(), workflow_version.to_string());
    if let EventDetail::Transition {
        delivery_id: detail_delivery,
        ..
    } = &mut detail
    {
        *detail_delivery = delivery_id.map(str::to_string);
    }
    let moved = Moved {
        issue: issue.into(),
        event: Event {
            seq,
            issue_id: id.to_string(),
            actor: actor.into(),
            created_at,
            detail,
        },
    };
    Ok((moved, transition_id))
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
        r#"SELECT e.seq, e.issue_id, e.created_at,
                a.id AS actor_id, a.name AS actor_name, a.role AS actor_role,
                e.kind, t.from_status AS "from_status?", t.to_status AS "to_status?",
                t.workflow_version AS "workflow_version?", t.delivery_id AS "delivery_id?",
                c.body AS "comment_body?"
         FROM event e
         JOIN actor a ON a.id = e.actor_id
         LEFT JOIN transition t ON t.id = e.transition_id
         LEFT JOIN comment c ON c.id = e.comment_id
         WHERE e.workspace_id = $1 AND e.issue_id = $2
         ORDER BY e.seq"#,
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

/// Writes a comment and its event.
pub(crate) async fn record_comment(
    tx: &mut sqlx::PgConnection,
    actor: &Actor,
    id: &str,
    body: &str,
    transition_id: Option<Uuid>,
) -> sqlx::Result<Event> {
    let comment_id = sqlx::query_scalar!(
        "INSERT INTO comment (issue_id, actor_id, body, transition_id)
         VALUES ($1, $2, $3, $4) RETURNING id",
        id,
        actor.id,
        body,
        transition_id,
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
        "INSERT INTO event (workspace_id, seq, issue_id, kind, actor_id, comment_id)
         VALUES ($1, $2, $3, 'comment', $4, $5) RETURNING created_at",
        actor.workspace_id,
        seq,
        id,
        actor.id,
        comment_id,
    )
    .fetch_one(&mut *tx)
    .await?;
    Ok(Event {
        seq,
        issue_id: id.to_string(),
        actor: actor.into(),
        created_at,
        detail: EventDetail::Comment {
            body: body.to_string(),
        },
    })
}

/// An agent's comment ends with `(agent)` on its own line. Punchlist adds it when missing.
fn signed_by_agent(body: &str) -> String {
    let body = body.trim_end();
    if body.lines().last().map(str::trim) == Some("(agent)") {
        body.to_string()
    } else {
        format!("{body}\n\n(agent)")
    }
}

/// Comment on an issue. An agent's comment ends with `(agent)` on its own line. Nobody
/// in a role the issue's status locks may comment.
#[utoipa::path(
    post,
    path = "/api/issues/{id}/comments",
    params(("id" = String, Path, description = "Issue id, such as PL-1")),
    request_body = CreateComment,
    responses(
        (status = 201, body = Event),
        (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody, description = "An agent commented on another issue than its run's."),
        (status = 404, body = ErrorBody),
        (status = 409, body = ErrorBody, description = "The issue's status is locked to the actor's role."),
        (status = 422, body = ErrorBody, description = "The body is empty."),
    )
)]
async fn add_comment(
    State(state): State<AppState>,
    actor: Actor,
    Path(id): Path<String>,
    Json(request): Json<CreateComment>,
) -> Result<(StatusCode, Json<Event>), ApiError> {
    if request.body.trim().is_empty() {
        return Err(ApiError::Invalid("a comment needs a body".into()));
    }
    let mut tx = state.pool.begin().await?;
    let status = sqlx::query_scalar!(
        "SELECT status FROM issue WHERE id = $1 AND workspace_id = $2 FOR UPDATE",
        id,
        actor.workspace_id,
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::IssueNotFound(id.clone()))?;
    let (workflow, _) = active_workflow(&mut tx, actor.workspace_id).await?;
    workflow.check_unlocked(&status, actor.role)?;
    let body = if actor.role == Role::Agent {
        signed_by_agent(&request.body)
    } else {
        request.body.trim_end().to_string()
    };
    let event = record_comment(&mut tx, &actor, &id, &body, None).await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(event)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_comments_end_with_the_agent_line() {
        assert_eq!(signed_by_agent("Opened #12.\n"), "Opened #12.\n\n(agent)");
        assert_eq!(
            signed_by_agent("Opened #12.\n\n(agent)\n"),
            "Opened #12.\n\n(agent)"
        );
        assert_eq!(
            signed_by_agent("Not (agent) here"),
            "Not (agent) here\n\n(agent)"
        );
    }
}
