//! `/api/runs`: claim a started issue, append logs, finish an attempt, and read runs.

use std::time::Duration;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use punchlist_api::{
    AppendLog, Claim, ClaimRequest, ErrorBody, FinishRun, LogLine, Repository, Run, RunLog,
    RunOutcome, RunnerRef,
};
use punchlist_core::{agent_display_name, branch_name};
use sqlx::PgPool;
use tokio::sync::broadcast::error::RecvError;
use tokio::time::Instant;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use crate::auth::{Actor, hash_token, new_token};
use crate::issues::{check_move, record_transition};
use crate::runners::{RunnerRow, runner_of};
use crate::workflow::{active_workflow, lock_workspace};
use crate::{ApiError, AppState};

const MAX_WAIT_SECONDS: u32 = 30;
const MAX_LOG_LINES: usize = 1000;
const MAX_LOG_LINE_BYTES: usize = 64 * 1024;
const LOG_TAIL: i64 = 20;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(claim))
        .routes(routes!(append_log))
        .routes(routes!(finish_run))
        .routes(routes!(run_log))
        .routes(routes!(issue_runs))
}

enum Target {
    /// The lowest-numbered issue in Start that no other claim has locked.
    Any,
    /// The issue a "moved to Start" notification named.
    Issue(String),
}

fn is_unique_violation(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .is_some_and(|e| e.is_unique_violation())
}

/// Claims one issue for the runner in one transaction: moves it Start to In Progress as the
/// runner, creates the run, attempt 1 and the agent actor the run acts as, and leases the
/// attempt.
async fn try_claim(
    state: &AppState,
    actor: &Actor,
    runner: &RunnerRow,
    target: &Target,
) -> Result<Option<Claim>, ApiError> {
    let mut tx = state.pool.begin().await?;
    // Before the workflow is read, so a claim waiting behind a load uses the new one.
    lock_workspace(&mut tx, actor.workspace_id).await?;
    let (workflow, _) = active_workflow(&mut tx, actor.workspace_id).await?;
    let Some(dispatch) = workflow.dispatch_for("in_progress") else {
        return Ok(None);
    };
    if !runner.agents.contains(&dispatch.agent) {
        return Ok(None);
    }
    let id = match target {
        Target::Any => {
            let id = sqlx::query_scalar!(
                "SELECT id FROM issue WHERE workspace_id = $1 AND status = 'start'
                 ORDER BY number LIMIT 1 FOR UPDATE SKIP LOCKED",
                actor.workspace_id,
            )
            .fetch_optional(&mut *tx)
            .await?;
            let Some(id) = id else { return Ok(None) };
            id
        }
        Target::Issue(id) => {
            let status = sqlx::query_scalar!(
                "SELECT status FROM issue WHERE id = $1 AND workspace_id = $2 FOR UPDATE",
                id,
                actor.workspace_id,
            )
            .fetch_optional(&mut *tx)
            .await?;
            if status.as_deref() != Some("start") {
                tx.rollback().await?;
                let holder = sqlx::query_scalar!(
                    "SELECT r.name FROM attempt a JOIN runner r ON r.id = a.runner_id
                     JOIN run ON run.id = a.run_id
                     WHERE a.issue_id = $1 ORDER BY run.started_at DESC, a.attempt DESC LIMIT 1",
                    id,
                )
                .fetch_optional(&state.pool)
                .await?
                .unwrap_or_else(|| "another runner".to_string());
                return Err(ApiError::ClaimTaken(format!(
                    "{id} was claimed by runner {holder}"
                )));
            }
            id.clone()
        }
    };
    let gates = check_move(&mut tx, &workflow, actor, &id, "start", "in_progress").await?;
    let moved = record_transition(
        &mut tx,
        actor,
        &id,
        "start".into(),
        "in_progress",
        None,
        workflow.version(),
        &gates,
    )
    .await?;
    let branch = branch_name(&id, &moved.issue.title);
    let run_id = sqlx::query_scalar!(
        "INSERT INTO run (workspace_id, issue_id, status, agent, branch)
         VALUES ($1, $2, 'in_progress', $3, $4) RETURNING id",
        actor.workspace_id,
        id,
        dispatch.agent,
        branch,
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(|error| {
        if is_unique_violation(&error) {
            ApiError::ClaimTaken(format!("{id} was claimed by another runner"))
        } else {
            error.into()
        }
    })?;
    let agent_token = new_token();
    sqlx::query!(
        "INSERT INTO actor (workspace_id, name, role, token_hash, run_id)
         VALUES ($1, $2, 'agent', $3, $4)",
        actor.workspace_id,
        agent_display_name(&dispatch.agent),
        hash_token(&agent_token),
        run_id,
    )
    .execute(&mut *tx)
    .await?;
    let lease_expires_at = sqlx::query_scalar!(
        "INSERT INTO attempt (run_id, issue_id, runner_id, attempt, lease_expires_at)
         VALUES ($1, $2, $3, 1, now() + make_interval(secs => $4))
         RETURNING lease_expires_at",
        run_id,
        id,
        runner.id,
        state.lease.as_secs_f64(),
    )
    .fetch_one(&mut *tx)
    .await?;
    let repository = sqlx::query_as!(
        Repository,
        "SELECT owner, name, default_branch FROM repository WHERE workspace_id = $1",
        actor.workspace_id,
    )
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Some(Claim {
        run_id,
        attempt: 1,
        agent: dispatch.agent.clone(),
        issue: moved.issue,
        repository,
        branch,
        lease_expires_at,
        prompt: workflow.prompt_for("in_progress"),
        agent_token,
    }))
}

/// Claim a started issue. Runner token. 204 when nothing is claimable; with `wait_seconds`
/// the request waits for an issue to enter Start. When several runners wait, each issue
/// goes to one of them and the others get 409 `claim_taken`.
#[utoipa::path(
    post,
    path = "/api/runs/claim",
    request_body = ClaimRequest,
    responses(
        (status = 200, body = Claim),
        (status = 204, description = "Nothing to claim."),
        (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody),
        (status = 409, body = ErrorBody, description = "Another runner claimed the issue first."),
    )
)]
async fn claim(
    State(state): State<AppState>,
    actor: Actor,
    request: Option<Json<ClaimRequest>>,
) -> Result<Response, ApiError> {
    let wait = Duration::from_secs(u64::from(
        request
            .map(|Json(r)| r.wait_seconds)
            .unwrap_or_default()
            .min(MAX_WAIT_SECONDS),
    ));
    let runner = runner_of(&state, &actor).await?;
    // Subscribe before the first attempt so a move to Start in between is not missed.
    let mut starts = state.starts.subscribe();
    if let Some(claim) = try_claim(&state, &actor, &runner, &Target::Any).await? {
        return Ok(Json(claim).into_response());
    }
    let deadline = Instant::now() + wait;
    loop {
        let target = match tokio::time::timeout_at(deadline, starts.recv()).await {
            Err(_) | Ok(Err(RecvError::Closed)) => break,
            Ok(Ok((workspace_id, issue_id))) => {
                if workspace_id != actor.workspace_id {
                    continue;
                }
                Target::Issue(issue_id)
            }
            Ok(Err(RecvError::Lagged(_))) => Target::Any,
        };
        if let Some(claim) = try_claim(&state, &actor, &runner, &target).await? {
            return Ok(Json(claim).into_response());
        }
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// Append lines to a run's log. Only the runner holding the running attempt may write.
#[utoipa::path(
    post,
    path = "/api/runs/{id}/log",
    params(("id" = Uuid, Path, description = "Run id")),
    request_body = AppendLog,
    responses(
        (status = 204, description = "Appended."),
        (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody),
        (status = 404, body = ErrorBody),
        (status = 409, body = ErrorBody, description = "The attempt is not the runner's running attempt."),
        (status = 422, body = ErrorBody),
    )
)]
async fn append_log(
    State(state): State<AppState>,
    actor: Actor,
    Path(id): Path<Uuid>,
    Json(request): Json<AppendLog>,
) -> Result<StatusCode, ApiError> {
    let runner = runner_of(&state, &actor).await?;
    if request.lines.len() > MAX_LOG_LINES {
        return Err(ApiError::Invalid(format!(
            "at most {MAX_LOG_LINES} lines per request"
        )));
    }
    if request
        .lines
        .iter()
        .any(|line| line.len() > MAX_LOG_LINE_BYTES)
    {
        return Err(ApiError::Invalid(format!(
            "a log line can be at most {MAX_LOG_LINE_BYTES} bytes"
        )));
    }
    if request.first_line < 1 {
        return Err(ApiError::Invalid("first_line starts at 1".into()));
    }
    ensure_run(&state.pool, &actor, id).await?;
    if request.lines.is_empty() {
        hold_attempt(&state.pool, id, request.attempt, &runner).await?;
        return Ok(StatusCode::NO_CONTENT);
    }
    let mut tx = state.pool.begin().await?;
    // Locking the attempt row serializes appends with finish and with lease expiry.
    let held = sqlx::query_scalar!(
        "SELECT attempt FROM attempt
         WHERE run_id = $1 AND attempt = $2 AND runner_id = $3 AND state = 'running'
             AND lease_expires_at > now()
         FOR UPDATE",
        id,
        request.attempt,
        runner.id,
    )
    .fetch_optional(&mut *tx)
    .await?;
    if held.is_none() {
        return Err(stale(id, request.attempt));
    }
    // Lines numbered at or below the stored maximum arrived in an earlier, retried batch.
    let stored = sqlx::query_scalar!(
        r#"SELECT coalesce(max(line_no), 0) AS "max!" FROM run_log WHERE run_id = $1 AND attempt = $2"#,
        id,
        request.attempt,
    )
    .fetch_one(&mut *tx)
    .await?;
    let skip = (stored - request.first_line + 1).clamp(0, request.lines.len() as i64) as usize;
    let fresh = &request.lines[skip..];
    if fresh.is_empty() {
        return Ok(StatusCode::NO_CONTENT);
    }
    let first_new = request.first_line + skip as i64;
    let line_nos: Vec<i64> = (first_new..first_new + fresh.len() as i64).collect();
    let count = fresh.len() as i64;
    let last = sqlx::query_scalar!(
        "UPDATE run SET last_log_seq = last_log_seq + $2 WHERE id = $1 RETURNING last_log_seq",
        id,
        count,
    )
    .fetch_one(&mut *tx)
    .await?;
    let seqs: Vec<i64> = (last - count + 1..=last).collect();
    sqlx::query!(
        "INSERT INTO run_log (run_id, seq, attempt, line_no, line)
         SELECT $1, t.seq, $2, t.line_no, t.line
         FROM UNNEST($3::bigint[], $4::bigint[], $5::text[]) AS t(seq, line_no, line)",
        id,
        request.attempt,
        &seqs,
        &line_nos,
        fresh,
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

fn stale(run_id: Uuid, attempt: i32) -> ApiError {
    ApiError::StaleAttempt(format!(
        "attempt {attempt} of run {run_id} is not running under this runner, or its lease expired"
    ))
}

async fn hold_attempt(
    pool: &PgPool,
    run_id: Uuid,
    attempt: i32,
    runner: &RunnerRow,
) -> Result<(), ApiError> {
    let held = sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM attempt
           WHERE run_id = $1 AND attempt = $2 AND runner_id = $3 AND state = 'running'
             AND lease_expires_at > now())
           AS "held!""#,
        run_id,
        attempt,
        runner.id,
    )
    .fetch_one(pool)
    .await?;
    if held {
        Ok(())
    } else {
        Err(stale(run_id, attempt))
    }
}

async fn ensure_run(pool: &PgPool, actor: &Actor, id: Uuid) -> Result<(), ApiError> {
    let exists = sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM run WHERE id = $1 AND workspace_id = $2) AS "exists!""#,
        id,
        actor.workspace_id,
    )
    .fetch_one(pool)
    .await?;
    if exists {
        Ok(())
    } else {
        Err(ApiError::RunNotFound(id))
    }
}

/// Record the outcome of the runner's running attempt and of its run.
#[utoipa::path(
    post,
    path = "/api/runs/{id}/finish",
    params(("id" = Uuid, Path, description = "Run id")),
    request_body = FinishRun,
    responses(
        (status = 200, body = Run),
        (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody),
        (status = 404, body = ErrorBody),
        (status = 409, body = ErrorBody, description = "The attempt is not the runner's running attempt."),
        (status = 422, body = ErrorBody),
    )
)]
async fn finish_run(
    State(state): State<AppState>,
    actor: Actor,
    Path(id): Path<Uuid>,
    Json(request): Json<FinishRun>,
) -> Result<Json<Run>, ApiError> {
    let runner = runner_of(&state, &actor).await?;
    let reason = request
        .reason
        .as_deref()
        .map(str::trim)
        .filter(|r| !r.is_empty());
    match request.outcome {
        RunOutcome::Running => {
            return Err(ApiError::Invalid(
                "the outcome must be succeeded or failed".into(),
            ));
        }
        RunOutcome::Failed if reason.is_none() => {
            return Err(ApiError::Invalid("a failed attempt needs a reason".into()));
        }
        _ => {}
    }
    if request.duration_ms < 0
        || request.input_tokens.is_some_and(|n| n < 0)
        || request.output_tokens.is_some_and(|n| n < 0)
    {
        return Err(ApiError::Invalid(
            "duration and token counts cannot be negative".into(),
        ));
    }
    ensure_run(&state.pool, &actor, id).await?;
    let failure_reason = match request.outcome {
        RunOutcome::Failed => reason,
        _ => None,
    };
    let mut tx = state.pool.begin().await?;
    let updated = sqlx::query_scalar!(
        "UPDATE attempt SET state = $4, finished_at = now(), failure_reason = $5,
                duration_ms = $6, input_tokens = $7, output_tokens = $8
         WHERE run_id = $1 AND attempt = $2 AND runner_id = $3 AND state = 'running'
             AND lease_expires_at > now()
         RETURNING id",
        id,
        request.attempt,
        runner.id,
        request.outcome.as_str(),
        failure_reason,
        request.duration_ms,
        request.input_tokens,
        request.output_tokens,
    )
    .fetch_optional(&mut *tx)
    .await?;
    if updated.is_none() {
        return Err(stale(id, request.attempt));
    }
    sqlx::query!(
        "UPDATE run SET outcome = $2, finished_at = now() WHERE id = $1",
        id,
        request.outcome.as_str(),
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    let run = load_runs(&state.pool, &actor, None, Some(id))
        .await?
        .into_iter()
        .next()
        .ok_or(ApiError::RunNotFound(id))?;
    Ok(Json(run))
}

/// Fails every running attempt whose lease has lapsed, and its run. The issue stays In
/// Progress; retrying is a later slice. Returns how many attempts expired.
pub async fn expire_leases(pool: &PgPool) -> sqlx::Result<u64> {
    let mut tx = pool.begin().await?;
    let expired = sqlx::query_scalar!(
        "UPDATE attempt SET state = 'failed', finished_at = now(),
                failure_reason = 'lease expired: runner ' || runner.name
                                 || ' stopped sending heartbeats'
         FROM runner
         WHERE runner.id = attempt.runner_id AND attempt.state = 'running'
           AND attempt.lease_expires_at < now()
         RETURNING attempt.run_id"
    )
    .fetch_all(&mut *tx)
    .await?;
    sqlx::query!(
        "UPDATE run SET outcome = 'failed', finished_at = now()
         WHERE id = ANY($1) AND outcome = 'running'",
        &expired,
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(expired.len() as u64)
}

struct RunRow {
    id: Uuid,
    issue_id: String,
    agent: String,
    branch: String,
    outcome: String,
    started_at: DateTime<Utc>,
    finished_at: Option<DateTime<Utc>>,
    attempt: i32,
    runner_id: Uuid,
    runner_name: String,
    failure_reason: Option<String>,
    duration_ms: Option<i64>,
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
}

struct LogRow {
    seq: i64,
    attempt: i32,
    line: String,
    created_at: DateTime<Utc>,
}

impl From<LogRow> for LogLine {
    fn from(row: LogRow) -> LogLine {
        LogLine {
            seq: row.seq,
            attempt: row.attempt,
            line: row.line,
            created_at: row.created_at,
        }
    }
}

/// Runs of the workspace, newest first, each with its latest attempt and log tail. Filter
/// by issue or by run id.
async fn load_runs(
    pool: &PgPool,
    actor: &Actor,
    issue_id: Option<&str>,
    run_id: Option<Uuid>,
) -> Result<Vec<Run>, ApiError> {
    let rows = sqlx::query_as!(
        RunRow,
        r#"SELECT r.id, r.issue_id, r.agent, r.branch, r.outcome, r.started_at, r.finished_at,
                  a.attempt AS "attempt!", a.runner_id AS "runner_id!",
                  n.name AS "runner_name!", a.failure_reason, a.duration_ms,
                  a.input_tokens, a.output_tokens
           FROM run r
           JOIN LATERAL (SELECT * FROM attempt WHERE run_id = r.id
                         ORDER BY attempt DESC LIMIT 1) a ON true
           JOIN runner n ON n.id = a.runner_id
           WHERE r.workspace_id = $1
             AND ($2::text IS NULL OR r.issue_id = $2)
             AND ($3::uuid IS NULL OR r.id = $3)
           ORDER BY r.started_at DESC, r.id"#,
        actor.workspace_id,
        issue_id,
        run_id,
    )
    .fetch_all(pool)
    .await?;
    let mut runs = Vec::with_capacity(rows.len());
    for row in rows {
        let mut tail = sqlx::query_as!(
            LogRow,
            "SELECT seq, attempt, line, created_at FROM run_log
             WHERE run_id = $1 ORDER BY seq DESC LIMIT $2",
            row.id,
            LOG_TAIL,
        )
        .fetch_all(pool)
        .await?;
        tail.reverse();
        let outcome = RunOutcome::parse(&row.outcome).ok_or_else(|| {
            sqlx::Error::Decode(format!("unknown run outcome `{}`", row.outcome).into())
        })?;
        runs.push(Run {
            id: row.id,
            issue_id: row.issue_id,
            agent_name: agent_display_name(&row.agent),
            agent: row.agent,
            branch: row.branch,
            outcome,
            started_at: row.started_at,
            finished_at: row.finished_at,
            attempt: row.attempt,
            runner: RunnerRef {
                id: row.runner_id,
                name: row.runner_name,
            },
            failure_reason: row.failure_reason,
            duration_ms: row.duration_ms,
            input_tokens: row.input_tokens,
            output_tokens: row.output_tokens,
            log_tail: tail.into_iter().map(LogLine::from).collect(),
        });
    }
    Ok(runs)
}

/// An issue's runs, newest first.
#[utoipa::path(
    get,
    path = "/api/issues/{id}/runs",
    params(("id" = String, Path, description = "Issue id, such as PL-1")),
    responses(
        (status = 200, body = Vec<Run>),
        (status = 401, body = ErrorBody),
        (status = 404, body = ErrorBody),
    )
)]
async fn issue_runs(
    State(state): State<AppState>,
    actor: Actor,
    Path(id): Path<String>,
) -> Result<Json<Vec<Run>>, ApiError> {
    let exists = sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM issue WHERE id = $1 AND workspace_id = $2) AS "exists!""#,
        id,
        actor.workspace_id,
    )
    .fetch_one(&state.pool)
    .await?;
    if !exists {
        return Err(ApiError::IssueNotFound(id));
    }
    Ok(Json(load_runs(&state.pool, &actor, Some(&id), None).await?))
}

/// Every log line of a run, oldest first.
#[utoipa::path(
    get,
    path = "/api/runs/{id}/log",
    params(("id" = Uuid, Path, description = "Run id")),
    responses(
        (status = 200, body = RunLog),
        (status = 401, body = ErrorBody),
        (status = 404, body = ErrorBody),
    )
)]
async fn run_log(
    State(state): State<AppState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> Result<Json<RunLog>, ApiError> {
    ensure_run(&state.pool, &actor, id).await?;
    let lines = sqlx::query_as!(
        LogRow,
        "SELECT seq, attempt, line, created_at FROM run_log WHERE run_id = $1 ORDER BY seq",
        id,
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(RunLog {
        run_id: id,
        lines: lines.into_iter().map(LogLine::from).collect(),
    }))
}
