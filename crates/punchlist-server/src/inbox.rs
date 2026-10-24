//! `/api/inbox`: the issues waiting on the caller, a person, and which of them they have
//! opened (PRD R16).

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use punchlist_api::{ErrorBody, InboxItem, InboxList, MarkInboxRead, PullRequestIssue};
use punchlist_core::display_name;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::actors::person_only;
use crate::auth::Actor;
use crate::workflow::active_workflow;
use crate::{ApiError, AppState};

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(inbox))
        .routes(routes!(mark_read))
}

/// The workspace's issues in a status that waits on a person (`Workflow::person_statuses`)
/// that are unassigned, assigned to the caller or to an agent the caller owns, newest
/// `since_seq` first. A person's token.
#[utoipa::path(
    get,
    path = "/api/inbox",
    responses(
        (status = 200, body = InboxList),
        (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody, description = "Not a person's token."),
    )
)]
pub(crate) async fn inbox(
    State(state): State<AppState>,
    actor: Actor,
) -> Result<Json<InboxList>, ApiError> {
    person_only(&actor)?;
    let mut conn = state.pool.acquire().await?;
    let (workflow, _) = active_workflow(&mut conn, actor.workspace_id).await?;
    let statuses: Vec<String> = (workflow.person_statuses().into_iter())
        .map(str::to_string)
        .collect();
    // An issue from before `created` events has none until its first move.
    let rows = sqlx::query!(
        r#"SELECT i.id, i.title, i.status,
                  coalesce(e.seq, 0) AS "since_seq!",
                  coalesce(e.created_at, i.created_at) AS "since!",
                  (r.since_seq IS NULL OR r.since_seq < coalesce(e.seq, 0)) AS "unread!"
           FROM issue i
           LEFT JOIN actor a ON a.id = i.assignee_id
           LEFT JOIN LATERAL (
               SELECT seq, created_at FROM event
               WHERE issue_id = i.id AND kind IN ('transition', 'created')
               ORDER BY seq DESC LIMIT 1
           ) e ON true
           LEFT JOIN inbox_read r
                  ON r.actor_id = $2 AND r.issue_id = i.id AND r.kind = 'waiting'
           WHERE i.workspace_id = $1 AND i.status = ANY($3)
             AND (i.assignee_id IS NULL OR i.assignee_id = $2 OR a.owner_id = $2)
           ORDER BY 4 DESC, i.number DESC"#,
        actor.workspace_id,
        actor.id,
        &statuses,
    )
    .fetch_all(&mut *conn)
    .await?;
    let unread = rows.iter().filter(|row| row.unread).count() as i64;
    let items = rows
        .into_iter()
        .map(|row| InboxItem::Waiting {
            issue: PullRequestIssue {
                id: row.id,
                title: row.title,
                status_name: display_name(&row.status),
                status: row.status,
            },
            since: row.since,
            since_seq: row.since_seq,
            unread: row.unread,
        })
        .collect();
    Ok(Json(InboxList { items, unread }))
}

/// The caller opened an inbox item: it is read until an event newer than `since_seq` makes
/// it again. The larger `since_seq` is kept, so an older one changes nothing. A person's
/// token.
#[utoipa::path(
    post,
    path = "/api/inbox/read",
    request_body = MarkInboxRead,
    responses(
        (status = 204),
        (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody, description = "Not a person's token."),
        (status = 404, body = ErrorBody, description = "No such issue in the workspace."),
    )
)]
pub(crate) async fn mark_read(
    State(state): State<AppState>,
    actor: Actor,
    Json(request): Json<MarkInboxRead>,
) -> Result<StatusCode, ApiError> {
    person_only(&actor)?;
    let written = sqlx::query!(
        "INSERT INTO inbox_read (actor_id, issue_id, kind, since_seq)
         SELECT $1, id, $3, $4 FROM issue WHERE id = $2 AND workspace_id = $5
         ON CONFLICT (actor_id, issue_id, kind)
         DO UPDATE SET since_seq = greatest(inbox_read.since_seq, EXCLUDED.since_seq)",
        actor.id,
        request.issue,
        request.kind.as_str(),
        request.since_seq,
        actor.workspace_id,
    )
    .execute(&state.pool)
    .await?;
    if written.rows_affected() == 0 {
        return Err(ApiError::IssueNotFound(request.issue));
    }
    Ok(StatusCode::NO_CONTENT)
}
