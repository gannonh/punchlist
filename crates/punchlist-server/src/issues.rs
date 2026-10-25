//! `/api/issues`: create, list, show and move issues, and read their timelines.

use std::collections::HashMap;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use punchlist_api::{
    CreateComment, CreateIssue, ErrorBody, Event, EventDetail, GateChecklist, Issue, IssueList,
    IssueWritten, MoveIssue, Moved, TransitionList, TransitionOption, UpdateIssue, Verdict,
};
use punchlist_core::{
    CANCELED, DONE, Evidence, GateResult, IN_PROGRESS, ProofEvidence, ProofKind,
    PullRequestEvidence, PullRequestState, Refusal, ReviewThreadEvidence, Role, START, Transition,
    Workflow, agent_display_name, display_name,
};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use crate::auth::{Actor, parse_role};
use crate::error::GateFailure;
use crate::events::{events, next_event_seq, record_row_event};
use crate::github::{
    PUNCHLIST_LOGIN, PullRequestRead, catch_up_merge, github_actor, lock_pull_requests,
    newest_check_runs, refresh_pull_requests, written_since,
};
use crate::rows::{issue, issue_rows};
use crate::runs::RUNS_PER_ENTRY;
use crate::workflow::{active_workflow, lock_workspace, status_info, stored_workflow};
use crate::{ApiError, AppState};

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(create_issue, list_issues))
        .routes(routes!(get_issue, update_issue))
        .routes(routes!(move_issue, issue_transitions))
        .routes(routes!(issue_events))
        .routes(routes!(issue_gates))
        .routes(routes!(add_comment))
}

/// Create an issue in the workflow's first status, and its `created` event.
#[utoipa::path(
    post,
    path = "/api/issues",
    request_body = CreateIssue,
    responses(
        (status = 201, body = IssueWritten),
        (status = 401, body = ErrorBody),
        (status = 422, body = ErrorBody, description = "The title is empty."),
    )
)]
pub(crate) async fn create_issue(
    State(state): State<AppState>,
    actor: Actor,
    Json(request): Json<CreateIssue>,
) -> Result<(StatusCode, Json<IssueWritten>), ApiError> {
    let title = request.title.trim();
    if title.is_empty() {
        return Err(ApiError::Invalid("an issue needs a title".into()));
    }
    let mut tx = state.pool.begin().await?;
    lock_workspace(&mut tx, actor.workspace_id).await?;
    let (workflow, _) = active_workflow(&mut tx, actor.workspace_id).await?;
    let next = sqlx::query!(
        r#"UPDATE workspace SET next_issue_number = next_issue_number + 1 WHERE id = $1
           RETURNING issue_prefix, next_issue_number - 1 AS "number!""#,
        actor.workspace_id,
    )
    .fetch_one(&mut *tx)
    .await?;
    let id = format!("{}-{}", next.issue_prefix, next.number);
    sqlx::query!(
        "INSERT INTO issue (id, workspace_id, number, title, body, status, created_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
        id,
        actor.workspace_id,
        next.number,
        title,
        request.body,
        workflow.initial_status().as_str(),
        actor.id,
    )
    .execute(&mut *tx)
    .await?;
    let seq = record_event(&mut tx, &actor, &id, None, None).await?;
    let written = written(&mut tx, &actor, &id, seq).await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(written)))
}

/// The issue's row and the sequence number of the write's event, read in the write's
/// transaction.
async fn written(
    tx: &mut sqlx::PgConnection,
    actor: &Actor,
    id: &str,
    seq: i64,
) -> Result<IssueWritten, ApiError> {
    let row = issue_rows(tx, actor.workspace_id, Some(&[id.to_string()]))
        .await?
        .pop()
        .ok_or_else(|| ApiError::IssueNotFound(id.to_string()))?;
    Ok(IssueWritten { row, seq })
}

/// Writes an issue's `created` event, or with `fields` its `edited` event, and returns its
/// sequence number. `at` is when it happened, when that was not now: an import's issue was
/// created in Linear.
pub(crate) async fn record_event(
    tx: &mut sqlx::PgConnection,
    actor: &Actor,
    id: &str,
    fields: Option<&[&str]>,
    at: Option<DateTime<Utc>>,
) -> sqlx::Result<i64> {
    let seq = next_event_seq(tx, actor.workspace_id).await?;
    let fields: Option<Vec<String>> = fields.map(|f| f.iter().map(|f| f.to_string()).collect());
    sqlx::query!(
        "INSERT INTO event (workspace_id, seq, issue_id, kind, actor_id, edited_fields, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, coalesce($7, now()))",
        actor.workspace_id,
        seq,
        id,
        if fields.is_some() {
            "edited"
        } else {
            "created"
        },
        actor.id,
        fields.as_deref(),
        at,
    )
    .execute(&mut *tx)
    .await?;
    Ok(seq)
}

/// List the workspace's issues, oldest first, each with its run, newest pull request and
/// next gated transition's gates.
#[utoipa::path(
    get,
    path = "/api/issues",
    responses((status = 200, body = IssueList), (status = 401, body = ErrorBody))
)]
pub(crate) async fn list_issues(
    State(state): State<AppState>,
    actor: Actor,
) -> Result<Json<IssueList>, ApiError> {
    // One snapshot for the list and the sequence number (ADR 0006).
    let mut tx = state.pool.begin().await?;
    sqlx::query!("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await?;
    let issues = issue_rows(&mut tx, actor.workspace_id, None).await?;
    let last_event_seq = sqlx::query_scalar!(
        "SELECT last_event_seq FROM workspace WHERE id = $1",
        actor.workspace_id
    )
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(IssueList {
        issues,
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
    let mut conn = state.pool.acquire().await?;
    let mut issue = issue(&mut conn, actor.workspace_id, &id)
        .await?
        .ok_or_else(|| ApiError::IssueNotFound(id.clone()))?;
    let ended = runs_ended(&mut conn, std::slice::from_ref(&id))
        .await?
        .remove(&id);
    // A run of the entry said it waits on a person (`wait_for_person`), which
    // `queue_redispatch` reads too: it is the entry's last, since its end dispatches nothing.
    let waiting = sqlx::query!(
        r#"SELECT d.agent, r.waiting_reason AS "waiting_reason!" FROM dispatch d
           JOIN run r ON r.id = d.run_id
           WHERE d.issue_id = $1 AND r.waiting_reason IS NOT NULL
             AND d.transition_id = (SELECT e.transition_id FROM event e
                                    WHERE e.issue_id = d.issue_id AND e.kind = 'transition'
                                    ORDER BY e.seq DESC LIMIT 1)"#,
        id,
    )
    .fetch_optional(&mut *conn)
    .await?;
    issue.waiting_on_person = match (waiting, ended) {
        (Some(waiting), _) => Some(format!(
            "{} is waiting on a person: {}",
            agent_display_name(&waiting.agent),
            waiting.waiting_reason
        )),
        (None, ended) => ended.map(|runs| {
            format!(
                "{runs} runs in {} ended without moving it",
                issue.status_name
            )
        }),
    };
    // A run of the entry hit its agent's usage limit and gave its dispatch back to wait for
    // the reset (`queue_redispatch`).
    let limited = sqlx::query!(
        r#"SELECT agent, not_before AS "not_before!" FROM dispatch
           WHERE issue_id = $1 AND run_id IS NULL AND not_before > now()"#,
        id,
    )
    .fetch_optional(&mut *conn)
    .await?;
    issue.waiting_for_usage_limit = limited.map(|limited| {
        format!(
            "waiting for the {} usage limit to reset at {}",
            agent_display_name(&limited.agent),
            limited.not_before.format("%Y-%m-%d %H:%M:%S UTC")
        )
    });
    Ok(Json(issue))
}

/// The issues in `ids` whose entry into their status has had its last run end, with that
/// run's number: nothing dispatches them again. The same two rules as `queue_redispatch`, the
/// latest transition and the limit; change them together. `get_issue` and the inbox read it.
pub(crate) async fn runs_ended(
    conn: &mut sqlx::PgConnection,
    ids: &[String],
) -> sqlx::Result<HashMap<String, i32>> {
    let rows = sqlx::query!(
        "SELECT d.issue_id, d.run_number FROM dispatch d JOIN run r ON r.id = d.run_id
         WHERE d.issue_id = ANY($1) AND d.run_number >= $2 AND r.outcome <> 'running'
           AND d.transition_id = (SELECT e.transition_id FROM event e
                                  WHERE e.issue_id = d.issue_id AND e.kind = 'transition'
                                  ORDER BY e.seq DESC LIMIT 1)",
        ids,
        RUNS_PER_ENTRY,
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| (row.issue_id, row.run_number))
        .collect())
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
    // The evaluation holds pull request rows that a delivery's job also wants, and Postgres
    // ends one of two transactions that wait on each other (40P01; 40001 is a serialization
    // failure). Nothing was decided for the one ended: it asks again, as when its evidence
    // changed.
    let ended = |error: &sqlx::Error| {
        let code = error.as_database_error().and_then(|error| error.code());
        matches!(code.as_deref(), Some("40P01" | "40001"))
    };
    match request_move(&state, &actor, &id, &request).await {
        Err(ApiError::Database(error)) if ended(&error) => {
            tracing::warn!(issue = id, %error, "a gated request lost a deadlock");
            Err(ApiError::EvidenceChanged(id))
        }
        answer => answer.map(Json),
    }
}

async fn request_move(
    state: &AppState,
    actor: &Actor,
    id: &str,
    request: &MoveIssue,
) -> Result<Moved, ApiError> {
    let read = refresh_evidence(state, actor, id, &request.to).await?;
    let mut tx = state.pool.begin().await?;
    lock_workspace(&mut tx, actor.workspace_id).await?;
    if let Some(read) = &read {
        lock_pull_requests(&mut tx, id, read).await?;
    }
    let from = sqlx::query_scalar!(
        "SELECT status FROM issue WHERE id = $1 AND workspace_id = $2 FOR UPDATE",
        id,
        actor.workspace_id,
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::IssueNotFound(id.to_string()))?;
    // The token was checked before the refresh, which ends the agent's run when it closes
    // the issue's pull request, as a webhook job may have meanwhile. Either holds the issue's
    // lock to do it, so under that lock the run is known to be running or not. A standalone
    // agent has no run to end.
    if actor.role == Role::Agent {
        let running = sqlx::query_scalar!(
            r#"SELECT a.run_id IS NULL OR EXISTS (SELECT 1 FROM run r
                                                 WHERE r.id = a.run_id AND r.outcome = 'running')
                   AS "running!"
               FROM actor a WHERE a.id = $1"#,
            actor.id,
        )
        .fetch_one(&mut *tx)
        .await?;
        if !running {
            return Err(ApiError::Unauthorized);
        }
    }
    let (workflow, _, invalid) = stored_workflow(&mut tx, actor.workspace_id).await?;
    let checked = check_move(&mut tx, &workflow, actor, id, &from, &request.to).await;
    // The gates judged the rows as they are now. If one changed since this request read
    // GitHub, that is not what it read: nothing is decided, pass or fail, and it asks again.
    // Any other refusal (the role, a lock, an unknown status) read no evidence, and stands.
    if let (Some(read), Ok(_) | Err(ApiError::GateFailed(_))) = (&read, &checked)
        && written_since(&mut tx, id, read).await?
    {
        tracing::info!(issue = id, "evidence changed after it was read from GitHub");
        return Err(ApiError::EvidenceChanged(id.to_string()));
    }
    // An agent's request a gate refuses goes on the timeline and in its owner's inbox, so a
    // person sees an agent that keeps being refused. Every other refusal names no gate, and a
    // person's is in the answer they got.
    let gates = match checked {
        Err(ApiError::GateFailed(failure)) if actor.role == Role::Agent => {
            if let Refusal::GateFailed {
                to, gate, reason, ..
            } = &failure.refusal
                && !repeats_refusal(&mut tx, actor, id, &from, to, gate, reason).await?
            {
                record_refusal(
                    &mut tx,
                    actor,
                    id,
                    &from,
                    to,
                    gate,
                    reason,
                    None,
                    workflow.version(),
                )
                .await?;
                tx.commit().await?;
            }
            return Err(ApiError::GateFailed(failure));
        }
        checked => checked?,
    };
    let (mut moved, _, dispatched) = write_transition(
        &mut tx,
        actor,
        id,
        from,
        &request.to,
        None,
        &workflow,
        &gates,
    )
    .await?;
    catch_up_merge(&mut tx, actor.workspace_id, &workflow, id, &request.to).await?;
    tx.commit().await?;
    if request.to == START {
        moved.warning = invalid;
    }
    if request.to == START || dispatched {
        // Wake runners that are long-polling for work. No receivers is fine.
        let _ = state.starts.send((actor.workspace_id, id.to_string()));
    }
    Ok(moved)
}

/// True when the issue's newest refusal, with no transition after it, is this one: an agent
/// retrying gets the same answer, and the timeline says it once. A changed reason is new.
async fn repeats_refusal(
    tx: &mut sqlx::PgConnection,
    actor: &Actor,
    id: &str,
    from: &str,
    to: &str,
    gate: &str,
    reason: &str,
) -> sqlx::Result<bool> {
    sqlx::query_scalar!(
        r#"SELECT EXISTS (
               SELECT 1 FROM event e JOIN refusal r ON r.id = e.refusal_id
               WHERE e.seq = (SELECT max(seq) FROM event
                              WHERE issue_id = $1 AND kind IN ('refusal', 'transition'))
                 AND e.issue_id = $1 AND e.kind = 'refusal' AND r.actor_id = $2
                 AND r.from_status = $3 AND r.to_status = $4 AND r.gate = $5 AND r.reason = $6
           ) AS "repeats!""#,
        id,
        actor.id,
        from,
        to,
        gate,
        reason,
    )
    .fetch_one(&mut *tx)
    .await
}

/// Before the issue is locked: if the move has gates, reads the issue's pull requests from
/// GitHub so they decide, not the webhooks still on their way. A pull request that GitHub
/// closed moves the issue here and ends its run, as its webhook would have, so `move_issue`
/// then answers as it would after that webhook. An issue the actor cannot move, or that does
/// not exist, is left for `move_issue` to refuse. A read that fails refuses the request
/// (`GithubUnreadable`): the recorded rows may be from before a push. Without a GitHub App
/// nothing is read, and the gates judge what the webhooks recorded.
///
/// Returns the pull requests as the read left them, or `None` when nothing was read: with
/// those `request_move` locks the rows and refuses a request whose evidence changed since.
async fn refresh_evidence(
    state: &AppState,
    actor: &Actor,
    id: &str,
    to: &str,
) -> Result<Option<Vec<PullRequestRead>>, ApiError> {
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
        let read = refresh_pull_requests(github, &state.pool, actor.workspace_id, id).await?;
        return Ok(Some(read));
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
    judge_move(tx, workflow, actor.role, id, from, to)
        .await
        .inspect_err(|error| log_gate_refusal(id, &actor.name, error))
}

/// `check_move`'s rules, read only and not logged: what a request by `role` would get. The
/// transitions list (`issue_transitions`) and every requested move both judge with this, so
/// the two agree on the same rows.
async fn judge_move(
    conn: &mut sqlx::PgConnection,
    workflow: &Workflow,
    role: Role,
    id: &str,
    from: &str,
    to: &str,
) -> Result<Vec<GateResult>, ApiError> {
    let transition = workflow.check_transition(from, to, role)?;
    judge_gates(conn, workflow, transition, id, from, None).await
}

/// Evaluates a transition's gates over the issue's recorded evidence, for a requested move
/// and for one a GitHub event causes: `closed_pull_request` is the pull request that event
/// merged or closed. Returns the results, all passed, or `ApiError::GateFailed`.
pub(crate) async fn check_gates(
    tx: &mut sqlx::PgConnection,
    workflow: &Workflow,
    transition: &Transition,
    actor_name: &str,
    id: &str,
    from: &str,
    closed_pull_request: Option<i64>,
) -> Result<Vec<GateResult>, ApiError> {
    judge_gates(tx, workflow, transition, id, from, closed_pull_request)
        .await
        .inspect_err(|error| log_gate_refusal(id, actor_name, error))
}

fn log_gate_refusal(id: &str, actor_name: &str, error: &ApiError) {
    if let ApiError::GateFailed(failure) = error {
        tracing::info!(issue = id, actor = %actor_name, "refused: {}", failure.refusal);
    }
}

async fn judge_gates(
    tx: &mut sqlx::PgConnection,
    workflow: &Workflow,
    transition: &Transition,
    id: &str,
    from: &str,
    closed_pull_request: Option<i64>,
) -> Result<Vec<GateResult>, ApiError> {
    if transition.gates.is_empty() {
        return Ok(Vec::new());
    }
    let (evidence, gates) =
        evaluate_gates(tx, workflow, transition, id, closed_pull_request).await?;
    if let Some(failed) = gates.iter().find(|result| !result.passed) {
        let refusal = Refusal::GateFailed {
            from: from.to_string(),
            to: transition.to.as_str().to_string(),
            gate: failed.gate.clone(),
            reason: failed.text(),
        };
        return Err(ApiError::GateFailed(Box::new(GateFailure {
            refusal,
            evidence,
            gates,
        })));
    }
    Ok(gates)
}

/// Each of a transition's gates, checked against the issue's recorded evidence, with that
/// evidence. The one evaluation behind a move and behind the checklist `pl issue show`
/// prints, so the two cannot disagree on the same rows.
pub(crate) async fn evaluate_gates(
    tx: &mut sqlx::PgConnection,
    workflow: &Workflow,
    transition: &Transition,
    id: &str,
    closed_pull_request: Option<i64>,
) -> sqlx::Result<(Evidence, Vec<GateResult>)> {
    let evidence = evidence(tx, id, closed_pull_request).await?;
    let gates = (transition.gates.iter())
        .map(|gate| gate.evaluate(&evidence, workflow.gate_settings()))
        .collect();
    Ok((evidence, gates))
}

/// The issue's attempt: how many times it has moved from Start to In Progress, 0 before the
/// first. Proof records the attempt it was uploaded in, and `proof_attached` reads the
/// current one. This is not the `attempt` table's number, which counts one run's leases.
pub(crate) async fn current_attempt(conn: &mut sqlx::PgConnection, id: &str) -> sqlx::Result<i32> {
    let count = sqlx::query_scalar!(
        r#"SELECT count(*) AS "count!" FROM transition
           WHERE issue_id = $1 AND from_status = $2 AND to_status = $3"#,
        id,
        START,
        IN_PROGRESS,
    )
    .fetch_one(&mut *conn)
    .await?;
    i32::try_from(count).map_err(|_| sqlx::Error::Decode("attempt count overflows".into()))
}

/// What the gates read: the pull requests linked to the issue, each open one with the check
/// runs on its head commit and its review threads, the one a GitHub event merged or closed
/// when that event is causing the move, and the issue's attempt and proof.
///
/// `written_since` in `github.rs` is its mirror: it asks whether any of these rows changed
/// after a gated request read them from GitHub. Change the two together. Proof is not read
/// from GitHub, and its upload takes the issue's row lock, so it cannot change under a move.
async fn evidence(
    tx: &mut sqlx::PgConnection,
    id: &str,
    closed_pull_request: Option<i64>,
) -> sqlx::Result<Evidence> {
    let rows = sqlx::query!(
        "SELECT id, repository_id, number, title, branch, state, draft, head_sha, merge_state
         FROM pull_request WHERE issue_id = $1 ORDER BY number DESC",
        id,
    )
    .fetch_all(&mut *tx)
    .await?;
    let mut pull_requests = Vec::with_capacity(rows.len());
    for row in rows {
        let state = PullRequestState::parse(&row.state).ok_or_else(|| {
            sqlx::Error::Decode(format!("unknown pull request state `{}`", row.state).into())
        })?;
        let (mut check_runs, mut review_threads) = (Vec::new(), Vec::new());
        if state == PullRequestState::Open {
            check_runs = newest_check_runs(tx, row.repository_id, &row.head_sha).await?;
            review_threads = sqlx::query_as!(
                ReviewThreadEvidence,
                "SELECT path, line, resolved FROM review_thread
                 WHERE pull_request_id = $1 ORDER BY path, line, node_id",
                row.id,
            )
            .fetch_all(&mut *tx)
            .await?;
        }
        pull_requests.push(PullRequestEvidence {
            number: row.number,
            title: row.title,
            branch: row.branch,
            state,
            draft: row.draft,
            head_sha: row.head_sha,
            merge_state: row.merge_state,
            check_runs,
            review_threads,
        });
    }
    let mut proof = Vec::new();
    for row in sqlx::query!("SELECT attempt, kind FROM proof WHERE issue_id = $1", id)
        .fetch_all(&mut *tx)
        .await?
    {
        let kind = ProofKind::parse(&row.kind).ok_or_else(|| {
            sqlx::Error::Decode(format!("unknown proof kind `{}`", row.kind).into())
        })?;
        proof.push(ProofEvidence {
            attempt: row.attempt,
            kind,
        });
    }
    let labels = sqlx::query_scalar!("SELECT labels FROM issue WHERE id = $1", id)
        .fetch_one(&mut *tx)
        .await?;
    Ok(Evidence {
        issue_id: id.to_string(),
        pull_requests,
        closed_pull_request,
        attempt: current_attempt(tx, id).await?,
        proof,
        labels,
    })
}

/// Writes a transition that the caller has already checked and whose issue row it has
/// locked: the new status, the transition with the workflow version and gate results, its
/// event, and the dispatch that waits for a runner when `to` has a dispatch rule. Shared by
/// `move_issue`, claims and GitHub events, which pass the webhook delivery that caused the
/// move.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn record_transition(
    tx: &mut sqlx::PgConnection,
    actor: &Actor,
    id: &str,
    from: String,
    to: &str,
    delivery_id: Option<&str>,
    workflow: &Workflow,
    gates: &[GateResult],
) -> Result<Moved, ApiError> {
    write_transition(tx, actor, id, from, to, delivery_id, workflow, gates)
        .await
        .map(|(moved, _, _)| moved)
}

/// Dispatch on entry. `end_dispatches` first, then `to` gets its own, for this transition,
/// when it has a dispatch rule and the workflow does not lock agents or runners out of it.
/// True when one was queued.
async fn queue_dispatch(
    tx: &mut sqlx::PgConnection,
    actor: &Actor,
    workflow: &Workflow,
    id: &str,
    to: &str,
    transition_id: Uuid,
) -> sqlx::Result<bool> {
    end_dispatches(tx, actor, id).await?;
    let unlocked = |role| workflow.check_unlocked(to, role).is_ok();
    let Some(dispatch) = workflow
        .dispatch_for(to)
        .filter(|_| unlocked(Role::Agent) && unlocked(Role::Runner))
    else {
        return Ok(false);
    };
    sqlx::query!(
        "INSERT INTO dispatch (issue_id, transition_id, agent) VALUES ($1, $2, $3)",
        id,
        transition_id,
        dispatch.agent,
    )
    .execute(&mut *tx)
    .await?;
    Ok(true)
}

/// The issue has left its status, so a dispatch still waiting for it goes. A run waiting for
/// its next attempt (`settle_run`) was for that status too, so it fails, its last attempt's
/// reason saying why; else the issue would wait behind a running run that no runner holds.
pub(crate) async fn end_dispatches(
    tx: &mut sqlx::PgConnection,
    actor: &Actor,
    id: &str,
) -> sqlx::Result<()> {
    sqlx::query!(
        "DELETE FROM dispatch WHERE issue_id = $1 AND run_id IS NULL",
        id
    )
    .execute(&mut *tx)
    .await?;
    let ended = sqlx::query!(
        "UPDATE run SET outcome = 'failed', finished_at = now()
         WHERE issue_id = $1 AND outcome = 'running'
           AND NOT EXISTS (SELECT 1 FROM attempt a WHERE a.run_id = run.id
                           AND a.state = 'running')",
        id
    )
    .execute(&mut *tx)
    .await?;
    if ended.rows_affected() > 0 {
        record_row_event(tx, actor.workspace_id, id, actor.id, "run").await?;
    }
    Ok(())
}

/// Inserts a transition row, with no gate results, and returns its id. Stamped with the write
/// clock, not the transaction's start: a merge caught up on entering Merging is a second
/// transition in the same transaction, and orders after the first.
pub(crate) async fn insert_transition(
    tx: &mut sqlx::PgConnection,
    actor: &Actor,
    id: &str,
    from: &str,
    to: &str,
    delivery_id: Option<&str>,
    workflow_version: &str,
) -> sqlx::Result<Uuid> {
    sqlx::query_scalar!(
        "INSERT INTO transition
             (issue_id, from_status, to_status, actor_id, workflow_version, delivery_id,
              created_at)
         VALUES ($1, $2, $3, $4, $5, $6, clock_timestamp()) RETURNING id",
        id,
        from,
        to,
        actor.id,
        workflow_version,
        delivery_id,
    )
    .fetch_one(&mut *tx)
    .await
}

/// Writes a transition's event; returns its sequence number and time.
pub(crate) async fn record_transition_event(
    tx: &mut sqlx::PgConnection,
    actor: &Actor,
    id: &str,
    transition_id: Uuid,
) -> sqlx::Result<(i64, DateTime<Utc>)> {
    let seq = next_event_seq(tx, actor.workspace_id).await?;
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
    Ok((seq, created_at))
}

/// `record_transition`, also returning the new transition's id and whether a dispatch was
/// queued for the new status.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn write_transition(
    tx: &mut sqlx::PgConnection,
    actor: &Actor,
    id: &str,
    from: String,
    to: &str,
    delivery_id: Option<&str>,
    workflow: &Workflow,
    gates: &[GateResult],
) -> Result<(Moved, Uuid, bool), ApiError> {
    let workflow_version = workflow.version();
    sqlx::query!(
        "UPDATE issue SET status = $2, updated_at = now() WHERE id = $1",
        id,
        to,
    )
    .execute(&mut *tx)
    .await?;
    let issue = issue(tx, actor.workspace_id, id)
        .await?
        .ok_or_else(|| ApiError::IssueNotFound(id.to_string()))?;
    let transition_id =
        insert_transition(tx, actor, id, &from, to, delivery_id, workflow_version).await?;
    for (position, result) in gates.iter().enumerate() {
        sqlx::query!(
            "INSERT INTO transition_gate (transition_id, position, gate, result, reason)
             VALUES ($1, $2, $3, $4, $5)",
            transition_id,
            position as i32,
            result.gate,
            if result.passed { "pass" } else { "fail" },
            result.text(),
        )
        .execute(&mut *tx)
        .await?;
    }
    let dispatched = queue_dispatch(tx, actor, workflow, id, to, transition_id).await?;
    let (seq, created_at) = record_transition_event(tx, actor, id, transition_id).await?;
    if let (DONE | CANCELED, Some(parent)) = (to, &issue.parent) {
        children_done(tx, actor.workspace_id, workflow, parent).await?;
    }
    let mut detail = EventDetail::transition(from, to.to_string(), workflow_version.to_string());
    if let EventDetail::Transition {
        delivery_id: detail_delivery,
        ..
    } = &mut detail
    {
        *detail_delivery = delivery_id.map(str::to_string);
    }
    let moved = Moved {
        issue,
        event: Event {
            seq,
            issue_id: id.to_string(),
            actor: actor.into(),
            created_at,
            detail,
        },
        warning: None,
    };
    Ok((moved, transition_id, dispatched))
}

/// The `children_done` event: once a child's move to `DONE` or `CANCELED` leaves every child
/// of its parent in one of the two, one at least `DONE`, the parent takes its transition on
/// the event from its status, if the workflow has one and its gates pass, as the workspace's
/// `Punchlist` actor, in the child's transaction. `write_transition` then checks the parent's
/// own parent. The parent's row is locked before its children are read, so of two children
/// closing at once the second sees the first's status. A parent with no children stays.
pub(crate) async fn children_done(
    tx: &mut sqlx::PgConnection,
    workspace_id: Uuid,
    workflow: &Workflow,
    parent: &str,
) -> Result<(), ApiError> {
    let parent = sqlx::query!(
        "SELECT id, status FROM issue WHERE id = $1 FOR UPDATE",
        parent,
    )
    .fetch_one(&mut *tx)
    .await?;
    let Some(transition) = workflow.transition_on(&parent.status, "children_done") else {
        return Ok(());
    };
    let done = sqlx::query_scalar!(
        r#"SELECT COALESCE(bool_and(status IN ($2, $3)) AND bool_or(status = $2), false) AS "done!"
           FROM issue WHERE parent_id = $1"#,
        parent.id,
        DONE,
        CANCELED,
    )
    .fetch_one(&mut *tx)
    .await?;
    if !done {
        return Ok(());
    }
    let actor = Actor {
        id: github_actor(tx, workspace_id, Some(PUNCHLIST_LOGIN)).await?,
        workspace_id,
        name: "Punchlist".to_string(),
        role: Role::Github,
        kind: None,
        token_hash: None,
        account: None,
    };
    let checked = check_gates(
        tx,
        workflow,
        transition,
        &actor.name,
        &parent.id,
        &parent.status,
        None,
    );
    let gates = match checked.await {
        Ok(gates) => gates,
        Err(ApiError::GateFailed(_)) => return Ok(()),
        Err(error) => return Err(error),
    };
    let to = transition.to.as_str();
    Box::pin(write_transition(
        tx,
        &actor,
        &parent.id,
        parent.status,
        to,
        None,
        workflow,
        &gates,
    ))
    .await?;
    Ok(())
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
    let events = events(&mut conn, actor.workspace_id, Some(&id), 0, i64::MAX).await?;
    Ok(Json(events))
}

/// The gates of the issue's next gated transition, each checked against the evidence
/// recorded now. Nothing is read from GitHub and nothing is written: a request for the
/// transition reads GitHub first, so it can answer differently. 204 when no transition an
/// actor can request from the issue's status has gates.
#[utoipa::path(
    get,
    path = "/api/issues/{id}/gates",
    params(("id" = String, Path, description = "Issue id, such as PL-1")),
    responses(
        (status = 200, body = GateChecklist),
        (status = 204, description = "The issue's status has no gated transition."),
        (status = 401, body = ErrorBody),
        (status = 404, body = ErrorBody),
    )
)]
async fn issue_gates(
    State(state): State<AppState>,
    actor: Actor,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let mut conn = state.pool.acquire().await?;
    let status = sqlx::query_scalar!(
        "SELECT status FROM issue WHERE id = $1 AND workspace_id = $2",
        id,
        actor.workspace_id,
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| ApiError::IssueNotFound(id.clone()))?;
    let (workflow, _) = active_workflow(&mut conn, actor.workspace_id).await?;
    let Some(transition) = workflow.next_gated(&status) else {
        return Ok(StatusCode::NO_CONTENT.into_response());
    };
    let (_, gates) = evaluate_gates(&mut conn, &workflow, transition, &id, None).await?;
    let to = transition.to.as_str().to_string();
    Ok(Json(GateChecklist {
        from_name: display_name(&status),
        from: status,
        to_name: display_name(&to),
        to,
        gates,
    })
    .into_response())
}

/// The transitions that can be requested from the issue's status, each with the verdict the
/// caller's request would get: the same rules and gates `POST /api/issues/{id}/transitions`
/// checks, over the evidence recorded now. Nothing is read from GitHub and nothing is
/// written, so a request, which reads GitHub first, can still answer differently.
#[utoipa::path(
    get,
    path = "/api/issues/{id}/transitions",
    params(("id" = String, Path, description = "Issue id, such as PL-1")),
    responses(
        (status = 200, body = TransitionList),
        (status = 401, body = ErrorBody),
        (status = 404, body = ErrorBody),
    )
)]
async fn issue_transitions(
    State(state): State<AppState>,
    actor: Actor,
    Path(id): Path<String>,
) -> Result<Json<TransitionList>, ApiError> {
    let mut conn = state.pool.acquire().await?;
    let from = sqlx::query_scalar!(
        "SELECT status FROM issue WHERE id = $1 AND workspace_id = $2",
        id,
        actor.workspace_id,
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| ApiError::IssueNotFound(id.clone()))?;
    let (workflow, _) = active_workflow(&mut conn, actor.workspace_id).await?;
    let status = workflow.status(&from).ok_or_else(|| {
        sqlx::Error::Decode(format!("issue {id} is in `{from}`, not in its workflow").into())
    })?;
    let mut transitions = Vec::new();
    for transition in workflow.requestable(&from) {
        let to = transition.to.as_str();
        let verdict = match judge_move(&mut conn, &workflow, actor.role, &id, &from, to).await {
            Ok(_) => Verdict::Allowed,
            Err(ApiError::GateFailed(failure)) => Verdict::Blocked(
                (failure.gates.into_iter())
                    .find(|result| !result.passed)
                    .expect("a gate failure has a failed gate"),
            ),
            Err(ApiError::Refused(refusal)) => Verdict::Refused {
                code: refusal.code().to_string(),
                message: refusal.to_string(),
            },
            Err(error) => return Err(error),
        };
        transitions.push(TransitionOption {
            to: status_info(&workflow, &transition.to),
            by: transition.by.clone(),
            verdict,
        });
    }
    Ok(Json(TransitionList {
        status: status_info(&workflow, status),
        viewer_locked: workflow.check_unlocked(&from, actor.role).is_err(),
        transitions,
    }))
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
    let seq = next_event_seq(tx, actor.workspace_id).await?;
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

/// Writes the refusal of a transition, one a GitHub event would have made or an agent asked
/// for, and its event: `gate` failed for `reason`, so the issue stays in `from`.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn record_refusal(
    tx: &mut sqlx::PgConnection,
    actor: &Actor,
    id: &str,
    from: &str,
    to: &str,
    gate: &str,
    reason: &str,
    delivery_id: Option<&str>,
    workflow_version: &str,
) -> sqlx::Result<()> {
    let refusal_id = sqlx::query_scalar!(
        "INSERT INTO refusal
             (issue_id, from_status, to_status, gate, reason, actor_id, workflow_version,
              delivery_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING id",
        id,
        from,
        to,
        gate,
        reason,
        actor.id,
        workflow_version,
        delivery_id,
    )
    .fetch_one(&mut *tx)
    .await?;
    let seq = next_event_seq(tx, actor.workspace_id).await?;
    sqlx::query!(
        "INSERT INTO event (workspace_id, seq, issue_id, kind, actor_id, refusal_id)
         VALUES ($1, $2, $3, 'refusal', $4, $5)",
        actor.workspace_id,
        seq,
        id,
        actor.id,
        refusal_id,
    )
    .execute(&mut *tx)
    .await?;
    Ok(())
}

/// True when the comment's last non-blank line is `(agent)`: an agent wrote it.
pub(crate) fn is_signed_by_agent(body: &str) -> bool {
    body.trim_end().lines().last().map(str::trim) == Some("(agent)")
}

/// An agent's comment ends with `(agent)` on its own line. Punchlist adds it when missing.
pub(crate) fn signed_by_agent(body: &str) -> String {
    let body = body.trim_end();
    if is_signed_by_agent(body) {
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
    // Workspace before issue, as everywhere; the comment event updates the workspace row.
    lock_workspace(&mut tx, actor.workspace_id).await?;
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

/// The most labels an issue has, and the longest one, in characters.
const MAX_LABELS: usize = 20;
const MAX_LABEL_CHARS: usize = 50;

/// A `PATCH` request, parsed: the title trimmed and not empty, the labels trimmed, each
/// neither empty nor too long, duplicates dropped in order, and the parent and the blockers
/// trimmed and not empty, the blockers' duplicates dropped.
#[derive(Debug, PartialEq)]
struct Edit {
    title: Option<String>,
    body: Option<String>,
    labels: Option<Vec<String>>,
    assignee: Option<Option<Uuid>>,
    parent: Option<Option<String>>,
    project: Option<Option<Uuid>>,
    milestone: Option<Option<Uuid>>,
    blocked_by: Option<Vec<String>>,
}

fn parse_edit(request: UpdateIssue) -> Result<Edit, ApiError> {
    let title = match request.title.as_deref().map(str::trim) {
        Some("") => return Err(ApiError::Invalid("an issue needs a title".into())),
        title => title.map(str::to_string),
    };
    let labels = match request.labels {
        Some(raw) => {
            let mut labels: Vec<String> = Vec::new();
            for label in raw.iter().map(|l| l.trim()) {
                if label.is_empty() {
                    return Err(ApiError::Invalid("a label cannot be empty".into()));
                }
                if label.chars().count() > MAX_LABEL_CHARS {
                    return Err(ApiError::Invalid(format!(
                        "a label is at most {MAX_LABEL_CHARS} characters"
                    )));
                }
                if !labels.iter().any(|l| l == label) {
                    labels.push(label.to_string());
                }
            }
            if labels.len() > MAX_LABELS {
                return Err(ApiError::Invalid(format!(
                    "an issue has at most {MAX_LABELS} labels"
                )));
            }
            Some(labels)
        }
        None => None,
    };
    let parent = match request.parent {
        Some(Some(parent)) if parent.trim().is_empty() => {
            return Err(ApiError::Invalid(
                "a parent is an issue id such as PL-1, or null for none".into(),
            ));
        }
        parent => parent.map(|p| p.map(|p| p.trim().to_string())),
    };
    let blocked_by = match request.blocked_by {
        Some(raw) => {
            let mut blockers: Vec<String> = Vec::new();
            for blocker in raw.iter().map(|b| b.trim()) {
                if blocker.is_empty() {
                    return Err(ApiError::Invalid(
                        "a blocker is an issue id such as PL-1".into(),
                    ));
                }
                if !blockers.iter().any(|b| b == blocker) {
                    blockers.push(blocker.to_string());
                }
            }
            Some(blockers)
        }
        None => None,
    };
    Ok(Edit {
        title,
        body: request.body,
        labels,
        assignee: request.assignee,
        parent,
        project: request.project,
        milestone: request.milestone,
        blocked_by,
    })
}

/// Refuses a parent that is not an issue of the workspace, or that is the issue itself or
/// one of the issues under it. The caller holds the workspace lock, which every edit takes
/// first, so no other edit can close a cycle meanwhile.
async fn check_parent(
    tx: &mut sqlx::PgConnection,
    workspace_id: Uuid,
    id: &str,
    parent: &str,
) -> Result<(), ApiError> {
    if parent == id {
        return Err(ApiError::Invalid(format!("{id} cannot be its own parent")));
    }
    // The parent, then each issue above it.
    let above = sqlx::query_scalar!(
        r#"WITH RECURSIVE above (id, parent_id, depth) AS (
               SELECT id, parent_id, 0 FROM issue WHERE id = $1 AND workspace_id = $2
               UNION ALL
               SELECT i.id, i.parent_id, above.depth + 1
               FROM issue i JOIN above ON i.id = above.parent_id
           ) CYCLE id SET looped USING path
           SELECT id AS "id!" FROM above WHERE NOT looped ORDER BY depth"#,
        parent,
        workspace_id,
    )
    .fetch_all(&mut *tx)
    .await?;
    if above.is_empty() {
        return Err(ApiError::Invalid(format!(
            "no issue {parent} in this workspace"
        )));
    }
    if let Some(at) = above.iter().position(|issue| issue == id) {
        let chain: Vec<&str> = above[..=at].iter().rev().map(String::as_str).collect();
        return Err(ApiError::Invalid(format!(
            "{parent} is under {id} ({}), so it cannot be {id}'s parent",
            chain.join(" → ")
        )));
    }
    Ok(())
}

/// Refuses blockers that are not issues of the workspace, that are the issue itself, or that
/// the issue already blocks, directly or through others: the shortest such chain is named.
/// The caller holds the workspace lock, as for `check_parent`.
async fn check_blockers(
    tx: &mut sqlx::PgConnection,
    workspace_id: Uuid,
    id: &str,
    blockers: &[String],
) -> Result<(), ApiError> {
    if blockers.iter().any(|b| b == id) {
        return Err(ApiError::Invalid(format!(
            "{id} cannot be blocked by itself"
        )));
    }
    let found = sqlx::query_scalar!(
        r#"SELECT id AS "id!" FROM issue WHERE id = ANY($1) AND workspace_id = $2"#,
        blockers,
        workspace_id,
    )
    .fetch_all(&mut *tx)
    .await?;
    if let Some(missing) = blockers.iter().find(|b| !found.contains(b)) {
        return Err(ApiError::Invalid(format!(
            "no issue {missing} in this workspace"
        )));
    }
    // ponytail: walks every path from the blockers, so a dense graph of hundreds of relations
    // costs paths, not edges. Upgrade path: a breadth-first walk keeping each issue once.
    let chain = sqlx::query_scalar!(
        r#"WITH RECURSIVE chain (id, path) AS (
               SELECT b, ARRAY[b] FROM unnest($1::text[]) b
               UNION ALL
               SELECT ib.blocker_id, chain.path || ib.blocker_id
               FROM issue_blocker ib JOIN chain ON ib.issue_id = chain.id
               WHERE chain.id <> $2
           ) CYCLE id SET looped USING visited
           SELECT path AS "path!" FROM chain WHERE id = $2 AND NOT looped
           ORDER BY cardinality(path), path LIMIT 1"#,
        blockers,
        id,
    )
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(chain) = chain {
        return Err(ApiError::Invalid(format!(
            "{id} cannot be blocked by {}: {} is already blocked by {id} ({})",
            chain[0],
            chain[0],
            chain.join(" → ")
        )));
    }
    Ok(())
}

/// Edit an issue's title, body, labels, assignee, parent, project, milestone or blockers. Only the
/// fields that change are written, with one `edited` event naming them; a request that
/// changes nothing writes nothing and answers the workspace's latest sequence number. Nobody
/// in a role the issue's status locks may edit it.
#[utoipa::path(
    patch,
    path = "/api/issues/{id}",
    params(("id" = String, Path, description = "Issue id, such as PL-1")),
    request_body = UpdateIssue,
    responses(
        (status = 200, body = IssueWritten),
        (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody, description = "An agent edited another issue than its run's."),
        (status = 404, body = ErrorBody),
        (status = 409, body = ErrorBody, description = "The issue's status is locked to the actor's role."),
        (status = 422, body = ErrorBody, description = "An empty title, a bad label, an assignee that is not a person or agent of the workspace, a parent or a blocker that is not an issue of the workspace or would make a cycle, a project or milestone that is not the workspace's, or a milestone outside the issue's project."),
    )
)]
async fn update_issue(
    State(state): State<AppState>,
    actor: Actor,
    Path(id): Path<String>,
    Json(request): Json<UpdateIssue>,
) -> Result<Json<IssueWritten>, ApiError> {
    let edit = parse_edit(request)?;
    let mut tx = state.pool.begin().await?;
    lock_workspace(&mut tx, actor.workspace_id).await?;
    let current = sqlx::query!(
        "SELECT status, title, body, labels, assignee_id, parent_id, project_id, milestone_id
         FROM issue WHERE id = $1 AND workspace_id = $2 FOR UPDATE",
        id,
        actor.workspace_id,
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::IssueNotFound(id.clone()))?;
    let (workflow, _) = active_workflow(&mut tx, actor.workspace_id).await?;
    workflow.check_unlocked(&current.status, actor.role)?;
    if let Some(Some(assignee)) = edit.assignee {
        let role = sqlx::query_scalar!(
            "SELECT role FROM actor WHERE id = $1 AND workspace_id = $2",
            assignee,
            actor.workspace_id,
        )
        .fetch_optional(&mut *tx)
        .await?;
        let assignable = role
            .map(|role| parse_role(&role))
            .transpose()?
            .is_some_and(|role| matches!(role, Role::Person | Role::Agent));
        if !assignable {
            return Err(ApiError::Invalid(format!(
                "{assignee} is not a person or an agent of this workspace"
            )));
        }
    }
    if let Some(Some(parent)) = &edit.parent {
        check_parent(&mut tx, actor.workspace_id, &id, parent).await?;
    }
    let current_blockers = sqlx::query_scalar!(
        "SELECT blocker_id FROM issue_blocker WHERE issue_id = $1 ORDER BY blocker_id",
        id,
    )
    .fetch_all(&mut *tx)
    .await?;
    let blocked_by = match edit.blocked_by {
        Some(mut blockers) => {
            check_blockers(&mut tx, actor.workspace_id, &id, &blockers).await?;
            blockers.sort();
            (blockers != current_blockers).then_some(blockers)
        }
        None => None,
    };
    if let Some(Some(project)) = edit.project {
        let found = sqlx::query_scalar!(
            r#"SELECT EXISTS (SELECT 1 FROM project WHERE id = $1 AND workspace_id = $2)
                   AS "found!""#,
            project,
            actor.workspace_id,
        )
        .fetch_one(&mut *tx)
        .await?;
        if !found {
            return Err(ApiError::Invalid(format!(
                "no project {project} in this workspace"
            )));
        }
    }
    let parent = edit.parent.unwrap_or_else(|| current.parent_id.clone());
    let project = edit.project.unwrap_or(current.project_id);
    let milestone = match edit.milestone {
        Some(Some(milestone)) => {
            let found = sqlx::query!(
                r#"SELECT m.name, m.project_id, p.name AS project_name
                   FROM milestone m JOIN project p ON p.id = m.project_id
                   WHERE m.id = $1 AND p.workspace_id = $2"#,
                milestone,
                actor.workspace_id,
            )
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| {
                ApiError::Invalid(format!("no milestone {milestone} in this workspace"))
            })?;
            if project != Some(found.project_id) {
                return Err(ApiError::Invalid(format!(
                    "milestone {} is in project {}; give {id} that project to set it",
                    found.name, found.project_name
                )));
            }
            Some(milestone)
        }
        Some(None) => None,
        // A milestone is in its project, so a new project leaves none.
        None if project != current.project_id => None,
        None => current.milestone_id,
    };
    let title = edit.title.filter(|t| *t != current.title);
    let body = edit.body.filter(|b| *b != current.body);
    let labels = edit.labels.filter(|l| *l != current.labels);
    let assignee = edit.assignee.filter(|a| *a != current.assignee_id);
    let fields: Vec<&str> = [
        ("title", title.is_some()),
        ("body", body.is_some()),
        ("labels", labels.is_some()),
        ("assignee", assignee.is_some()),
        ("parent", parent != current.parent_id),
        ("project", project != current.project_id),
        ("milestone", milestone != current.milestone_id),
        ("blocked_by", blocked_by.is_some()),
    ]
    .into_iter()
    .filter_map(|(field, changed)| changed.then_some(field))
    .collect();
    let seq = if fields.is_empty() {
        sqlx::query_scalar!(
            "SELECT last_event_seq FROM workspace WHERE id = $1",
            actor.workspace_id
        )
        .fetch_one(&mut *tx)
        .await?
    } else {
        sqlx::query!(
            "UPDATE issue SET title = coalesce($2, title), body = coalesce($3, body),
                    labels = coalesce($4, labels),
                    assignee_id = CASE WHEN $5 THEN $6 ELSE assignee_id END,
                    parent_id = $7, project_id = $8, milestone_id = $9, updated_at = now()
             WHERE id = $1",
            id,
            title,
            body,
            labels.as_deref(),
            assignee.is_some(),
            assignee.flatten(),
            parent,
            project,
            milestone,
        )
        .execute(&mut *tx)
        .await?;
        if let Some(blockers) = &blocked_by {
            sqlx::query!("DELETE FROM issue_blocker WHERE issue_id = $1", id)
                .execute(&mut *tx)
                .await?;
            sqlx::query!(
                "INSERT INTO issue_blocker (workspace_id, issue_id, blocker_id)
                 SELECT $1, $2, unnest($3::text[])",
                actor.workspace_id,
                id,
                blockers,
            )
            .execute(&mut *tx)
            .await?;
        }
        record_event(&mut tx, &actor, &id, Some(&fields), None).await?
    };
    let written = written(&mut tx, &actor, &id, seq).await?;
    tx.commit().await?;
    Ok(Json(written))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_edit_trims_the_title_and_cleans_the_labels() {
        let edit = parse_edit(UpdateIssue {
            title: Some("  New title ".into()),
            labels: Some(vec![" ui ".into(), "bug".into(), "ui".into()]),
            ..UpdateIssue::default()
        })
        .unwrap();
        assert_eq!(
            edit,
            Edit {
                title: Some("New title".into()),
                body: None,
                labels: Some(vec!["ui".into(), "bug".into()]),
                assignee: None,
                parent: None,
                project: None,
                milestone: None,
                blocked_by: None,
            }
        );
        let refused = |request: UpdateIssue| parse_edit(request).unwrap_err().to_string();
        let labels = |labels: Vec<String>| UpdateIssue {
            labels: Some(labels),
            ..UpdateIssue::default()
        };
        assert_eq!(refused(labels(vec![" ".into()])), "a label cannot be empty");
        assert_eq!(
            refused(labels(vec!["x".repeat(51)])),
            "a label is at most 50 characters"
        );
        assert_eq!(
            refused(labels((0..21).map(|n| n.to_string()).collect())),
            "an issue has at most 20 labels"
        );
        assert_eq!(
            parse_edit(labels(vec!["x".repeat(50)])).unwrap().labels,
            Some(vec!["x".repeat(50)])
        );
    }

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
