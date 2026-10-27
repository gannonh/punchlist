//! `/api/inbox`: the issues waiting on the caller, a person, the runs of their agents that
//! need them, the moves their agents asked for that a gate refused, and which of them they
//! have opened (PRD R16).

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use punchlist_api::{
    Actor as ActorRef, ErrorBody, InboxItem, InboxList, MarkInboxRead, PullRequestIssue,
    RunOutcome, RunSummary,
};
use punchlist_core::{Status, Workflow, display_name};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::actors::person_only;
use crate::auth::{Actor, parse_role};
use crate::issues::{runs_ended, waiting_reasons};
use crate::rows::issue_runs;
use crate::workflow::active_workflow;
use crate::{ApiError, AppState};

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(inbox))
        .routes(routes!(mark_read))
}

/// What waits on the caller, newest `since_seq` first: the workspace's issues in a status that
/// waits on a person (`Workflow::person_statuses`) that are unassigned, assigned to the caller
/// or to an agent the caller owns; and the issues whose latest run, by an agent the caller
/// owns, ended with nothing to dispatch them again; and the issues an agent the caller owns
/// asked to move, refused by a gate, with no move since and no run running or queued. A
/// person's token.
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
    let mut items = waiting_items(&mut conn, &actor, &workflow).await?;
    items.extend(run_items(&mut conn, &actor, &workflow).await?);
    items.extend(refusal_items(&mut conn, &actor, &workflow).await?);
    // Stable, so the waiting items' issue order holds among equal `since_seq`s.
    items.sort_by_key(|(since_seq, _, _)| std::cmp::Reverse(*since_seq));
    let unread = items.iter().filter(|(_, unread, _)| *unread).count() as i64;
    let items = items.into_iter().map(|(_, _, item)| item).collect();
    Ok(Json(InboxList { items, unread }))
}

/// An item with its `since_seq` and `unread`, which the list orders and counts by.
type Sorted = (i64, bool, InboxItem);

async fn waiting_items(
    conn: &mut sqlx::PgConnection,
    actor: &Actor,
    workflow: &Workflow,
) -> Result<Vec<Sorted>, ApiError> {
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
    Ok(rows
        .into_iter()
        .map(|row| {
            let item = InboxItem::Waiting {
                issue: PullRequestIssue {
                    id: row.id,
                    title: row.title,
                    status_name: display_name(&row.status),
                    on_merge_to: workflow.on_merge_to(&row.status).map(Status::display_name),
                    status: row.status,
                },
                since: row.since,
                since_seq: row.since_seq,
                unread: row.unread,
            };
            (row.since_seq, row.unread, item)
        })
        .collect())
}

/// The issues whose latest run in their status ended and has nothing after it, by an agent the
/// caller owns: the row's run is `Failed` (`rows::issue_runs`), or `Idle` after the entry's
/// last run (`runs_ended`) or after a run whose agent said it waits on a person
/// (`waiting_reasons`), as `get_issue` says. Newest `run` event first.
async fn run_items(
    conn: &mut sqlx::PgConnection,
    actor: &Actor,
    workflow: &Workflow,
) -> Result<Vec<Sorted>, ApiError> {
    // The ended runs in their issue's current status by an agent the caller owns; the row's
    // derivation then says which is its issue's latest, and whether a run follows it. A run
    // from before dispatches was its entry's only one.
    let ended = sqlx::query!(
        r#"SELECT r.id, r.issue_id, r.agent, r.outcome,
                  coalesce(d.run_number - d.runs_before, 1) AS "runs!",
                  coalesce(e.seq, 0) AS "since_seq!",
                  coalesce(e.created_at, r.finished_at, r.started_at) AS "since!",
                  (rd.since_seq IS NULL OR rd.since_seq < coalesce(e.seq, 0)) AS "unread!"
           FROM actor a
           JOIN run r ON r.id = a.run_id
           JOIN issue i ON i.id = r.issue_id AND i.status = r.status
           LEFT JOIN dispatch d ON d.run_id = r.id
           LEFT JOIN LATERAL (
               SELECT seq, created_at FROM event
               WHERE issue_id = i.id AND kind = 'run'
               ORDER BY seq DESC LIMIT 1
           ) e ON true
           LEFT JOIN inbox_read rd
                  ON rd.actor_id = $2 AND rd.issue_id = i.id AND rd.kind = 'run'
           WHERE a.workspace_id = $1 AND a.owner_id = $2 AND r.outcome <> 'running'"#,
        actor.workspace_id,
        actor.id,
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut ids: Vec<String> = ended.iter().map(|run| run.issue_id.clone()).collect();
    ids.sort();
    ids.dedup();
    let limits = runs_ended(conn, &ids).await?;
    let mut waiting = waiting_reasons(conn, &ids).await?;
    let mut items = Vec::new();
    for (issue, summary, last) in issue_runs(conn, actor.workspace_id, &ids).await? {
        let Some(run) = last.and_then(|last| ended.iter().find(|run| run.id == last)) else {
            continue;
        };
        let limit = limits.get(&issue.id);
        let waiting_reason = waiting.remove(&issue.id).map(|(_, reason)| reason);
        let reason = match summary {
            RunSummary::Failed { reason, .. } => reason,
            RunSummary::Idle if limit.is_some() || waiting_reason.is_some() => None,
            _ => continue,
        };
        let last_outcome = RunOutcome::parse(&run.outcome).ok_or_else(|| {
            sqlx::Error::Decode(format!("unknown run outcome `{}`", run.outcome).into())
        })?;
        let item = InboxItem::Run {
            issue: PullRequestIssue {
                id: issue.id,
                title: issue.title,
                on_merge_to: workflow
                    .on_merge_to(&issue.status)
                    .map(Status::display_name),
                status: issue.status,
                status_name: issue.status_name,
            },
            agent: run.agent.clone(),
            runs: limit.copied().unwrap_or(run.runs),
            last_outcome,
            reason,
            waiting_reason,
            limit_reached: limit.is_some(),
            since: run.since,
            since_seq: run.since_seq,
            unread: run.unread,
        };
        items.push((run.since_seq, run.unread, item));
    }
    Ok(items)
}

/// Per issue, the newest refusal of a move asked for by an agent the caller owns, while the
/// issue is still in the status it was refused from and no transition came after it, and only
/// once nothing runs or waits to run there (the row's run is `Idle` or `Failed`): a refusal
/// the agent is still working through is not the person's. An issue whose agent said it waits
/// on a person (`waiting_reasons`) is that run item's alone.
async fn refusal_items(
    conn: &mut sqlx::PgConnection,
    actor: &Actor,
    workflow: &Workflow,
) -> Result<Vec<Sorted>, ApiError> {
    let rows = sqlx::query!(
        r#"SELECT DISTINCT ON (i.id)
                  i.id, i.title, i.status, r.from_status, r.to_status, r.gate, r.reason,
                  a.id AS agent_id, a.name AS agent_name, a.role AS agent_role,
                  a.kind AS agent_kind, e.seq AS since_seq, e.created_at AS since,
                  (rd.since_seq IS NULL OR rd.since_seq < e.seq) AS "unread!"
           FROM event e
           JOIN refusal r ON r.id = e.refusal_id
           JOIN actor a ON a.id = r.actor_id
           JOIN issue i ON i.id = e.issue_id AND i.status = r.from_status
           LEFT JOIN inbox_read rd
                  ON rd.actor_id = $2 AND rd.issue_id = i.id AND rd.kind = 'refusal'
           WHERE e.workspace_id = $1 AND e.kind = 'refusal' AND a.owner_id = $2
             AND NOT EXISTS (SELECT 1 FROM event t
                             WHERE t.issue_id = i.id AND t.kind = 'transition' AND t.seq > e.seq)
           ORDER BY i.id, e.seq DESC"#,
        actor.workspace_id,
        actor.id,
    )
    .fetch_all(&mut *conn)
    .await?;
    let ids: Vec<String> = rows.iter().map(|row| row.id.clone()).collect();
    let waiting = waiting_reasons(conn, &ids).await?;
    let settled: Vec<String> = (issue_runs(conn, actor.workspace_id, &ids)
        .await?
        .into_iter())
    .filter(|(issue, summary, _)| {
        matches!(summary, RunSummary::Idle | RunSummary::Failed { .. })
            && !waiting.contains_key(&issue.id)
    })
    .map(|(issue, _, _)| issue.id)
    .collect();
    let mut items = Vec::new();
    for row in rows.into_iter().filter(|row| settled.contains(&row.id)) {
        let item = InboxItem::Refusal {
            issue: PullRequestIssue {
                id: row.id,
                title: row.title,
                status_name: display_name(&row.status),
                on_merge_to: workflow.on_merge_to(&row.status).map(Status::display_name),
                status: row.status,
            },
            agent: ActorRef {
                id: row.agent_id,
                name: row.agent_name,
                role: parse_role(&row.agent_role)?,
                kind: row.agent_kind,
            },
            from_name: display_name(&row.from_status),
            from: row.from_status,
            to_name: display_name(&row.to_status),
            to: row.to_status,
            gate: row.gate,
            reason: row.reason,
            since: row.since,
            since_seq: row.since_seq,
            unread: row.unread,
        };
        items.push((row.since_seq, row.unread, item));
    }
    Ok(items)
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
