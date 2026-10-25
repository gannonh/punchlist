//! `/api/runs`: claim an issue, append logs, finish or resume an attempt, and read runs.

use std::collections::HashMap;
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use punchlist_api::{
    AppendLog, AttemptInfo, Claim, ClaimRequest, ErrorBody, FinishRun, GithubToken, Issue, LogLine,
    RefreshGithubToken, Repository, ResumeRun, Run, RunLog, RunOutcome, RunnerRef, WaitForPerson,
};
use punchlist_core::{IN_PROGRESS, Role, START, Workflow, agent_display_name, branch_name};
use sqlx::PgPool;
use tokio::sync::broadcast::error::RecvError;
use tokio::time::Instant;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use crate::auth::{Actor, hash_token, new_token};
use crate::events::record_row_event;
use crate::issues::{check_move, record_comment, record_transition, signed_by_agent};
use crate::rows::issue;
use crate::runners::{RunnerRow, runner_of};
use crate::workflow::{active_workflow, dispatch_workflow, lock_workspace};
use crate::{ApiError, AppState};

const MAX_WAIT_SECONDS: u32 = 30;
/// How often a waiting claim looks again with no wake: a run ended by the merge's webhook or
/// the lease sweep, or a dispatch past its `not_before`, wakes no runner.
// ponytail: each waiting claim runs one claim transaction a second. Upgrade path: wake the
// runners from every place a run ends or a dispatch comes due.
const CLAIM_RETRY: Duration = Duration::from_secs(1);
const MAX_LOG_LINES: usize = 1000;
const MAX_LOG_LINE_BYTES: usize = 64 * 1024;
const LOG_TAIL: i64 = 20;
const MAX_WAITING_REASON_CHARS: usize = 500;
/// How many runs one entry into a status gets before the issue waits on a person.
// ponytail: one limit for every status of every workspace. Upgrade path: a `[dispatch]`
// field in the workflow file, read from the version of the entering transition.
pub(crate) const RUNS_PER_ENTRY: i32 = 3;
/// How many attempts one run gets when its runner loses them, such as in a restart, before
/// the run fails and the issue is dispatched again.
// ponytail: one limit for every run of every workspace. Upgrade path: a `[dispatch]` field
// in the workflow file, beside `RUNS_PER_ENTRY`'s.
pub(crate) const ATTEMPTS_PER_RUN: i32 = 2;
/// How long an issue whose run hit its agent's usage limit, and the runner's claims for that
/// agent, wait when the agent did not say when the limit resets.
const USAGE_LIMIT_WAIT: chrono::TimeDelta = chrono::TimeDelta::minutes(15);

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(claim))
        .routes(routes!(append_log))
        .routes(routes!(finish_run))
        .routes(routes!(resume_run))
        .routes(routes!(github_token))
        .routes(routes!(wait_for_person))
        .routes(routes!(run_log))
        .routes(routes!(issue_runs))
}

enum Target {
    /// The claimable issue furthest along the workflow that no other claim has locked.
    Any,
    /// The same, after a notification named this issue: it entered Start or a status with a
    /// dispatch rule, or a run of it ended. The claim waits for a claim that holds it, and
    /// says when another runner took it.
    Issue(String),
}

fn is_unique_violation(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .is_some_and(|e| e.is_unique_violation())
}

/// What a claim found to run: the issue, locked, the status it is in, and the agent. A run
/// that lost its attempt and waits for its next one is `run_id`.
struct Claimable {
    id: String,
    from: String,
    agent: String,
    run_id: Option<Uuid>,
}

/// Selects what a runner that can run `agents` can claim and locks its issue row: an issue no
/// other claim has locked that is in Start, when the runner can run the agent In Progress
/// dispatches, or whose dispatch waits in the status it entered, names an agent the runner can
/// run, is past its `not_before` (a run that hit the usage limit set it), and is not
/// behind a run of the issue that is still running (one run works on an issue at a time;
/// `finish_run` wakes the runners when that run ends), or whose running run for the status it
/// is in has no running attempt and an agent the runner can run: its attempt was lost
/// (`settle_run`). The three kinds are ordered together: the status latest in the workflow's
/// `statuses` first, so work near Done does not wait behind work not started, then the
/// lowest-numbered issue. An issue in a status the workflow locks to agents or runners is
/// never claimed, even with a dispatch waiting there from an older workflow. A new run in a
/// status whose dispatch rule is `serial` waits while any run in that status is running in
/// the workspace, which has one repository.
async fn select_claimable(
    tx: &mut sqlx::PgConnection,
    workspace_id: Uuid,
    workflow: &Workflow,
    agents: &[String],
    target: &Target,
) -> sqlx::Result<Option<Claimable>> {
    // The agent a claim from Start runs, if this runner can run it.
    let start_agent = workflow
        .dispatch_for(IN_PROGRESS)
        .map(|dispatch| dispatch.agent.as_str())
        .filter(|agent| agents.iter().any(|a| a == agent));
    let locked: Vec<&str> = workflow
        .locks()
        .iter()
        .filter(|(_, roles)| roles.contains(&Role::Agent) || roles.contains(&Role::Runner))
        .map(|(status, _)| status.as_str())
        .collect();
    let serial: Vec<&str> = workflow
        .statuses()
        .iter()
        .map(|status| status.as_str())
        .filter(|status| workflow.dispatch_for(status).is_some_and(|d| d.serial))
        .collect();
    let order: Vec<&str> = workflow.statuses().iter().map(|s| s.as_str()).collect();
    if let Target::Issue(id) = target {
        // Waits for a claim that holds the issue, so the selection reads what it left. The
        // selection still takes the first work in the claim order, which may be another issue.
        sqlx::query!(
            "SELECT FROM issue WHERE id = $1 AND workspace_id = $2 FOR UPDATE",
            id,
            workspace_id,
        )
        .fetch_optional(&mut *tx)
        .await?;
    }
    // A waiting dispatch is always for the status its issue is in: `queue_dispatch`.
    sqlx::query_as!(
        Claimable,
        r#"SELECT i.id, i.status AS "from", r.id AS "run_id?",
                  CASE WHEN r.id IS NOT NULL THEN r.agent
                       WHEN i.status = $3 THEN $4 ELSE d.agent END AS "agent!"
           FROM issue i LEFT JOIN dispatch d ON d.issue_id = i.id AND d.run_id IS NULL
           LEFT JOIN run r ON r.issue_id = i.id AND r.outcome = 'running' AND r.status = i.status
                AND NOT EXISTS (SELECT 1 FROM attempt a WHERE a.run_id = r.id
                                AND a.state = 'running')
           WHERE i.workspace_id = $1
             AND NOT i.status = ANY($5)
             AND CASE WHEN r.id IS NOT NULL THEN r.agent = ANY($2)
                      WHEN i.status = $3 THEN $4::text IS NOT NULL
                      ELSE d.agent = ANY($2)
                           AND (d.not_before IS NULL OR d.not_before <= now())
                           AND NOT EXISTS (SELECT 1 FROM run WHERE run.issue_id = i.id
                                           AND run.outcome = 'running')
                 END
             AND (r.id IS NOT NULL
                  OR NOT CASE WHEN i.status = $3 THEN $6 ELSE i.status END = ANY($7)
                  OR NOT EXISTS (SELECT 1 FROM run s WHERE s.workspace_id = $1
                                 AND s.outcome = 'running'
                                 AND s.status = CASE WHEN i.status = $3 THEN $6
                                                     ELSE i.status END))
           ORDER BY array_position($8::text[], i.status) DESC NULLS LAST, i.number
           LIMIT 1 FOR UPDATE OF i SKIP LOCKED"#,
        workspace_id,
        agents,
        START,
        start_agent,
        &locked as &[&str],
        IN_PROGRESS,
        &serial as &[&str],
        &order as &[&str],
    )
    .fetch_optional(&mut *tx)
    .await
}

/// Claims one issue for the runner in one transaction: selects it, moves it from Start to
/// In Progress as the runner if that is where it is, then creates the run, attempt 1 and the
/// agent actor the run acts as, leases the attempt, and marks the issue's dispatch claimed.
/// A run waiting for its next attempt gets that attempt instead, on the run's branch with the
/// run's prompt, and no dispatch is touched. Claims nothing while the workspace runs the
/// workflow's `[dispatch] max_concurrent` attempts, and nothing for an agent kind while the
/// workspace runs its `[dispatch.agent.<kind>] max_concurrent` attempts, counted by
/// `run.agent`.
async fn try_claim(
    state: &AppState,
    actor: &Actor,
    runner: &RunnerRow,
    target: &Target,
) -> Result<Claimed, ApiError> {
    let mut tx = state.pool.begin().await?;
    // Before the workflow is read, so a claim waiting behind a load uses the new one.
    lock_workspace(&mut tx, actor.workspace_id).await?;
    // A claim that waited through a re-registration of its runner holds the old token: the
    // runner that registered since gets the work.
    let current = sqlx::query_scalar!("SELECT token_hash FROM actor WHERE id = $1", actor.id)
        .fetch_one(&mut *tx)
        .await?;
    if current.is_none() || current != actor.token_hash {
        return Err(ApiError::Unauthorized);
    }
    let (workflow, _) = active_workflow(&mut tx, actor.workspace_id).await?;
    // Under the workspace lock, so two claims cannot both take the last slot.
    let running = sqlx::query!(
        r#"SELECT r.agent, count(*) AS "n!" FROM attempt a JOIN run r ON r.id = a.run_id
           WHERE r.workspace_id = $1 AND a.state = 'running' GROUP BY r.agent"#,
        actor.workspace_id,
    )
    .fetch_all(&mut *tx)
    .await?;
    if let Some(max) = workflow.max_concurrent()
        && running.iter().map(|r| r.n).sum::<i64>() >= i64::from(max)
    {
        tx.rollback().await?;
        return Ok(Claimed::AtLimit);
    }
    // An agent kind running `[dispatch.agent.<kind>] max_concurrent` attempts is not offered.
    let full: Vec<&str> = (running.iter())
        .filter(|r| workflow.agent_limit(&r.agent).is_some_and(|max| r.n >= i64::from(max)))
        .map(|r| r.agent.as_str())
        .collect();
    // An agent kind whose usage limit the runner hit is claimed again once it resets.
    let paused = sqlx::query_scalar!(
        "SELECT agent FROM runner_pause WHERE runner_id = $1 AND until > now()",
        runner.id,
    )
    .fetch_all(&mut *tx)
    .await?;
    let agents: Vec<String> = (runner.agents.iter())
        .filter(|agent| !paused.contains(agent) && !full.contains(&agent.as_str()))
        .cloned()
        .collect();
    let claimable =
        select_claimable(&mut tx, actor.workspace_id, &workflow, &agents, target).await?;
    let Some(Claimable {
        id,
        from,
        agent,
        run_id: waiting,
    }) = claimable
    else {
        tx.rollback().await?;
        // A run of a full agent kind that ends frees a slot for any issue.
        if runner.agents.iter().any(|agent| full.contains(&agent.as_str())) {
            return Ok(Claimed::AtLimit);
        }
        // A runner woken for an issue that a run is working on, with no dispatch waiting
        // behind that run, lost the race for it. Otherwise there is nothing here for this
        // runner, or not yet.
        let Target::Issue(id) = target else {
            return Ok(Claimed::Nothing);
        };
        let holder = sqlx::query_scalar!(
            "SELECT r.name FROM attempt a JOIN runner r ON r.id = a.runner_id
             WHERE a.issue_id = $1 AND a.state = 'running'
               AND NOT EXISTS (SELECT 1 FROM dispatch d
                               WHERE d.issue_id = a.issue_id AND d.run_id IS NULL)
             LIMIT 1",
            id,
        )
        .fetch_optional(&state.pool)
        .await?;
        return match holder {
            Some(holder) => Err(ApiError::ClaimTaken(format!(
                "{id} was claimed by runner {holder}"
            ))),
            None => Ok(Claimed::Nothing),
        };
    };
    let merged_branches = merged_branches(&mut tx, &id).await?;
    let agent_token = new_token();
    let (run_id, attempt, branch, prompt, issue) = match waiting {
        // The lost attempt's agent held the old token: it can do nothing now.
        Some(run_id) => {
            let run = sqlx::query!(
                r#"SELECT branch, prompt,
                          (SELECT max(attempt) FROM attempt WHERE run_id = $1) AS "last!"
                   FROM run WHERE id = $1"#,
                run_id,
            )
            .fetch_one(&mut *tx)
            .await?;
            set_agent_token(&mut tx, run_id, &agent_token).await?;
            let issue = issue_row(&mut tx, actor.workspace_id, &id).await?;
            (run_id, run.last + 1, run.branch, run.prompt, issue)
        }
        None => {
            let (run_id, branch, prompt, issue) = new_run(
                &mut tx,
                actor,
                runner,
                &workflow,
                (id.clone(), from, agent.clone()),
                &merged_branches,
                &agent_token,
            )
            .await?;
            (run_id, 1, branch, prompt, issue)
        }
    };
    let leased = sqlx::query!(
        "INSERT INTO attempt (run_id, issue_id, runner_id, attempt, lease_expires_at, model)
         VALUES ($1, $2, $3, $4, now() + make_interval(secs => $5),
                 (SELECT coalesce(r.model, n.models ->> r.agent)
                  FROM runner n, run r WHERE n.id = $3 AND r.id = $1))
         RETURNING lease_expires_at, session_id, model,
                   (SELECT effort FROM run WHERE id = $1)",
        run_id,
        id,
        runner.id,
        attempt,
        state.lease.as_secs_f64(),
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
    record_row_event(&mut tx, actor.workspace_id, &id, actor.id, "run").await?;
    let repository = repository(&mut tx, actor.workspace_id).await?;
    tx.commit().await?;
    Ok(Claimed::Claim(Box::new(Claim {
        run_id,
        attempt,
        agent,
        issue,
        repository,
        branch,
        merged_branches,
        lease_expires_at: leased.lease_expires_at,
        prompt,
        agent_token,
        session_id: leased.session_id,
        model: leased.model,
        effort: leased.effort,
        next_line: 1,
        github: None,
        github_error: None,
    })))
}

/// What `try_claim` did.
enum Claimed {
    Claim(Box<Claim>),
    Nothing,
    /// The workspace runs `max_concurrent` attempts already, or found nothing while an agent
    /// kind of the runner's ran its own `max_concurrent`.
    AtLimit,
}

/// A new run for what `select_claimable` found, `(issue, status it is in, agent)`, and the
/// agent actor it acts as, with `agent_token`. Returns the run, its branch, the prompt it
/// stores for all its attempts, and the issue in the status the run is for.
async fn new_run(
    tx: &mut sqlx::PgConnection,
    actor: &Actor,
    runner: &RunnerRow,
    workflow: &Workflow,
    (id, from, agent): (String, String, String),
    merged_branches: &[String],
    agent_token: &str,
) -> Result<(Uuid, String, String, Issue), ApiError> {
    // The status the run is for, the issue in that status, and the prompt of the workflow
    // version that named the agent.
    let (status, issue, prompt, rule) = if from == START {
        let gates = check_move(&mut *tx, workflow, actor, &id, START, IN_PROGRESS).await?;
        let moved = record_transition(
            &mut *tx,
            actor,
            &id,
            from,
            IN_PROGRESS,
            None,
            workflow,
            &gates,
        )
        .await?;
        let prompt = workflow.prompt_for(IN_PROGRESS);
        let rule = workflow.dispatch_for(IN_PROGRESS).cloned();
        (IN_PROGRESS.to_string(), moved.issue, prompt, rule)
    } else {
        let issue = issue_row(&mut *tx, actor.workspace_id, &id).await?;
        let queued_under = dispatch_workflow(&mut *tx, actor.workspace_id, &id).await?;
        let prompt = queued_under.prompt_for(&from);
        let rule = queued_under.dispatch_for(&from).cloned();
        (from, issue, prompt, rule)
    };
    let (model, effort) = rule.map_or((None, None), |rule| (rule.model, rule.effort));
    // A branch whose pull request merged is finished: the next lifecycle gets the next
    // name, which still names the issue. An open pull request keeps its branch, and so does
    // a retry, because nothing has merged on the branch it was given. A run dispatched in a
    // later status of the same lifecycle, such as Agent Review, gets the same branch for
    // the same reason, and the runner that has its worktree reuses it.
    let base = branch_name(&id);
    let mut branch = base.clone();
    let mut lifecycle = 1;
    while merged_branches.contains(&branch) {
        lifecycle += 1;
        branch = format!("{base}-{lifecycle}");
    }
    let run_id = sqlx::query_scalar!(
        "INSERT INTO run (workspace_id, issue_id, status, agent, branch, prompt, model, effort)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING id",
        actor.workspace_id,
        id,
        status,
        agent,
        branch,
        prompt,
        model,
        effort,
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
    // Exactly one run per dispatch: the issue's lock is held, and `dispatch.run_id` is set
    // once. The move from Start wrote the dispatch this claims in the same transaction.
    sqlx::query!(
        "UPDATE dispatch SET run_id = $2 WHERE issue_id = $1 AND run_id IS NULL",
        id,
        run_id,
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        "INSERT INTO actor (workspace_id, name, role, token_hash, run_id, kind, owner_id)
         VALUES ($1, $2, 'agent', $3, $4, $5,
                 (SELECT registered_by FROM runner WHERE id = $6))",
        actor.workspace_id,
        agent_display_name(&agent),
        hash_token(agent_token),
        run_id,
        agent,
        runner.id,
    )
    .execute(&mut *tx)
    .await?;
    Ok((run_id, branch, prompt, issue))
}

/// The issue's branches with a merged pull request and no open one, sorted.
// ponytail: "merged" is judged by head branch name alone, so a pull request merged into a
// non-default base, or from a fork with the same branch name, also finishes the branch.
// Upgrade path: record the base ref and head repository on `pull_request`.
async fn merged_branches(conn: &mut sqlx::PgConnection, id: &str) -> sqlx::Result<Vec<String>> {
    sqlx::query_scalar!(
        "SELECT branch FROM pull_request WHERE issue_id = $1
         GROUP BY branch HAVING bool_or(state = 'merged') AND NOT bool_or(state = 'open')
         ORDER BY branch",
        id,
    )
    .fetch_all(&mut *conn)
    .await
}

/// The issue a run is for, which is in the runner's workspace.
async fn issue_row(
    conn: &mut sqlx::PgConnection,
    workspace_id: Uuid,
    id: &str,
) -> sqlx::Result<Issue> {
    issue(conn, workspace_id, id)
        .await?
        .ok_or(sqlx::Error::RowNotFound)
}

async fn repository(conn: &mut sqlx::PgConnection, workspace_id: Uuid) -> sqlx::Result<Repository> {
    sqlx::query_as!(
        Repository,
        "SELECT owner, name, default_branch FROM repository WHERE workspace_id = $1",
        workspace_id,
    )
    .fetch_one(&mut *conn)
    .await
}

/// Gives the run's agent actor a new token. Every claim and resume of the run does, so only
/// the agent of the attempt that is running now can act as the run.
async fn set_agent_token(
    conn: &mut sqlx::PgConnection,
    run_id: Uuid,
    token: &str,
) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE actor SET token_hash = $2 WHERE run_id = $1",
        run_id,
        hash_token(token),
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Claim an issue: one in Start, or one waiting in a status it entered whose dispatch rule
/// names an agent the runner can run. Runner token. 204 when nothing is claimable, or when the
/// workspace runs the workflow's `[dispatch] max_concurrent` attempts, or the agent kinds that
/// have work run their `[dispatch.agent.<kind>] max_concurrent`; with `wait_seconds` the
/// request waits for one. When several runners wait, each issue goes to
/// one of them and the others get 409 `claim_taken`.
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
    // Subscribe before the first attempt so a move in between is not missed.
    let mut starts = state.starts.subscribe();
    let mut claimed = try_claim(&state, &actor, &runner, &Target::Any).await?;
    let deadline = Instant::now() + wait;
    loop {
        let at_limit = match claimed {
            Claimed::Claim(claim) => {
                return Ok(Json(with_github_token(&state, *claim).await).into_response());
            }
            Claimed::Nothing => false,
            Claimed::AtLimit => true,
        };
        let target = tokio::select! {
            biased;
            () = tokio::time::sleep_until(deadline) => break,
            woken = starts.recv() => match woken {
                Err(RecvError::Closed) => break,
                Ok((workspace_id, _)) if workspace_id != actor.workspace_id => continue,
                // At the limit a wake may be a run that ended, which frees a slot for any
                // issue, so no other runner took the one the wake names.
                Ok(_) if at_limit => Target::Any,
                Ok((_, issue_id)) => Target::Issue(issue_id),
                Err(RecvError::Lagged(_)) => Target::Any,
            },
            () = tokio::time::sleep(CLAIM_RETRY) => Target::Any,
        };
        claimed = try_claim(&state, &actor, &runner, &target).await?;
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
    // A first reported model writes an event. Take its workspace lock before the
    // attempt lock, in the same order as finish and lease expiry.
    lock_workspace(&mut tx, actor.workspace_id).await?;
    // Locking the attempt row serializes appends with finish and with lease expiry.
    let Some(held) = sqlx::query!(
        "SELECT issue_id, reported_model FROM attempt
         WHERE run_id = $1 AND attempt = $2 AND runner_id = $3 AND state = 'running'
             AND lease_expires_at > now()
         FOR UPDATE",
        id,
        request.attempt,
        runner.id,
    )
    .fetch_optional(&mut *tx)
    .await?
    else {
        return Err(stale(id, request.attempt));
    };
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
    // The first model the agent names is the session's; a subagent's comes later.
    if held.reported_model.is_none()
        && let Some(model) = fresh.iter().find_map(|line| reported_model(line))
    {
        sqlx::query!(
            "UPDATE attempt SET reported_model = $3 WHERE run_id = $1 AND attempt = $2",
            id,
            request.attempt,
            model,
        )
        .execute(&mut *tx)
        .await?;
        record_row_event(&mut tx, actor.workspace_id, &held.issue_id, actor.id, "run").await?;
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The model an agent's output line says it runs on: Claude Code's `system`/`init` line or an
/// assistant message of the session (not a subagent's), or Codex's `thread/started`. Only a
/// plain name of at most 100 characters counts, since people read it outside any fence.
fn reported_model(line: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    let model = match (value["type"].as_str(), value["method"].as_str()) {
        (Some("system"), _) if value["subtype"] == "init" => &value["model"],
        (Some("assistant"), _) if value["parent_tool_use_id"].is_null() => {
            &value["message"]["model"]
        }
        (_, Some("thread/started")) => &value["params"]["thread"]["model"],
        _ => return None,
    }
    .as_str()?;
    let plain = (1..=100).contains(&model.len())
        && model
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-_:/@[]".contains(&b));
    plain.then(|| model.to_string())
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

/// Record the outcome of the runner's running attempt and of its run. A lost attempt keeps
/// the run running for its next attempt while the run has one left (`settle_run`).
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
        RunOutcome::Succeeded if request.lost => {
            return Err(ApiError::Invalid(
                "only a failed attempt can be lost".into(),
            ));
        }
        RunOutcome::Succeeded if request.usage_limit.is_some() => {
            return Err(ApiError::Invalid(
                "only a failed attempt can hit the usage limit".into(),
            ));
        }
        _ if request.lost && request.usage_limit.is_some() => {
            return Err(ApiError::Invalid(
                "a lost attempt did not hit the usage limit".into(),
            ));
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
    if request
        .cost_usd
        .as_deref()
        .is_some_and(|c| !plain_decimal(c))
    {
        return Err(ApiError::Invalid(
            "cost_usd must be a decimal such as 0.0453".into(),
        ));
    }
    ensure_run(&state.pool, &actor, id).await?;
    let failure_reason = match request.outcome {
        RunOutcome::Failed => reason,
        _ => None,
    };
    let mut tx = state.pool.begin().await?;
    // The workspace, then the issue, before the attempt, as a claim, a move and a pull
    // request close lock them. They serialize the end of the run with a workflow load and
    // with a move of the issue, so the dispatch queued below is for the status the issue is
    // in, under the workflow that is active.
    lock_workspace(&mut tx, actor.workspace_id).await?;
    let issue_id = sqlx::query_scalar!(
        "SELECT id FROM issue WHERE id = (SELECT issue_id FROM run WHERE id = $1) FOR UPDATE",
        id,
    )
    .fetch_one(&mut *tx)
    .await?;
    let updated = sqlx::query_scalar!(
        "UPDATE attempt SET state = $4, finished_at = now(), failure_reason = $5,
                duration_ms = $6, input_tokens = $7, output_tokens = $8,
                cost_usd = $9::text::numeric
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
        request.cost_usd,
    )
    .fetch_optional(&mut *tx)
    .await?;
    if updated.is_none() {
        return Err(stale(id, request.attempt));
    }
    let lost = request.lost.then_some(request.attempt);
    // A reset in the past, or none reported, would have the issue claimed into the same
    // limit at once: it waits `USAGE_LIMIT_WAIT` instead.
    let resets_at = request.usage_limit.map(|limit| {
        let now = Utc::now();
        (limit.resets_at)
            .filter(|at| *at > now)
            .unwrap_or(now + USAGE_LIMIT_WAIT)
    });
    if let Some(until) = resets_at {
        sqlx::query!(
            "INSERT INTO runner_pause (runner_id, agent, until)
             SELECT $1, agent, $3 FROM run WHERE id = $2
             ON CONFLICT (runner_id, agent) DO UPDATE SET until = excluded.until",
            runner.id,
            id,
            until,
        )
        .execute(&mut *tx)
        .await?;
    }
    settle_run(
        &mut tx,
        actor.workspace_id,
        id,
        request.outcome,
        lost,
        resets_at,
    )
    .await?;
    record_row_event(&mut tx, actor.workspace_id, &issue_id, actor.id, "run").await?;
    tx.commit().await?;
    let run = load_runs(&state.pool, &actor, None, Some(id))
        .await?
        .into_iter()
        .next()
        .ok_or(ApiError::RunNotFound(id))?;
    // A dispatch that waited for this run to end, the one its end queued, the run's next
    // attempt, or the next dispatch a `serial` rule held back can be claimed now: wake the
    // runners. The claim finds out whether there is one.
    let _ = state
        .starts
        .send((actor.workspace_id, run.issue_id.clone()));
    Ok(Json(run))
}

/// Resume the runner's running attempt after the runner restarted: renews its lease, gives
/// the run's agent a new token, and answers the attempt's claim again, with the same agent
/// session and the number of the attempt's next log line. No attempt is created. Runner
/// token; 409 `stale_attempt` unless the attempt is the runner's running attempt and its
/// lease has not lapsed.
#[utoipa::path(
    post,
    path = "/api/runs/{id}/resume",
    params(("id" = Uuid, Path, description = "Run id")),
    request_body = ResumeRun,
    responses(
        (status = 200, body = Claim),
        (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody),
        (status = 404, body = ErrorBody),
        (status = 409, body = ErrorBody, description = "The attempt is not the runner's running attempt."),
    )
)]
async fn resume_run(
    State(state): State<AppState>,
    actor: Actor,
    Path(id): Path<Uuid>,
    Json(request): Json<ResumeRun>,
) -> Result<Json<Claim>, ApiError> {
    let runner = runner_of(&state, &actor).await?;
    ensure_run(&state.pool, &actor, id).await?;
    let mut tx = state.pool.begin().await?;
    let Some(held) = sqlx::query!(
        "UPDATE attempt SET lease_expires_at = now() + make_interval(secs => $4)
         WHERE run_id = $1 AND attempt = $2 AND runner_id = $3 AND state = 'running'
             AND lease_expires_at > now()
         RETURNING issue_id, lease_expires_at, session_id, model",
        id,
        request.attempt,
        runner.id,
        state.lease.as_secs_f64(),
    )
    .fetch_optional(&mut *tx)
    .await?
    else {
        return Err(stale(id, request.attempt));
    };
    let run = sqlx::query!(
        "SELECT agent, branch, prompt, effort FROM run WHERE id = $1",
        id
    )
    .fetch_one(&mut *tx)
    .await?;
    // `append_log` skips lines numbered at or below the stored maximum.
    let next_line = sqlx::query_scalar!(
        r#"SELECT coalesce(max(line_no), 0) + 1 AS "next!" FROM run_log
           WHERE run_id = $1 AND attempt = $2"#,
        id,
        request.attempt,
    )
    .fetch_one(&mut *tx)
    .await?;
    let agent_token = new_token();
    set_agent_token(&mut tx, id, &agent_token).await?;
    let issue = issue_row(&mut tx, actor.workspace_id, &held.issue_id).await?;
    let merged_branches = merged_branches(&mut tx, &held.issue_id).await?;
    let repository = repository(&mut tx, actor.workspace_id).await?;
    tx.commit().await?;
    let claim = Claim {
        run_id: id,
        attempt: request.attempt,
        agent: run.agent,
        issue,
        repository,
        branch: run.branch,
        merged_branches,
        lease_expires_at: held.lease_expires_at,
        prompt: run.prompt,
        agent_token,
        session_id: held.session_id,
        model: held.model,
        effort: run.effort,
        next_line,
        github: None,
        github_error: None,
    };
    Ok(Json(with_github_token(&state, claim).await))
}

/// `claim` with the token its run acts on GitHub with, or why there is none. Minted after
/// the claim committed, so no GitHub request holds the workspace's lock; the runner fails an
/// attempt whose token could not be minted.
async fn with_github_token(state: &AppState, mut claim: Claim) -> Claim {
    let Some(github) = &state.github else {
        return claim;
    };
    let Repository { owner, name, .. } = &claim.repository;
    match github.run_token(owner, name).await {
        Ok(token) => claim.github = Some(token),
        Err(error) => {
            tracing::warn!(
                "run {}: cannot mint its GitHub token: {error:#}",
                claim.run_id
            );
            claim.github_error = Some(format!("{error:#}"));
        }
    }
    claim
}

/// A fresh GitHub token for the runner's running attempt, before the one it has expires.
/// Runner token; 409 `stale_attempt` unless the attempt is the runner's running attempt and
/// its lease has not lapsed; 503 when the server has no App credentials or GitHub refused.
#[utoipa::path(
    post,
    path = "/api/runs/{id}/github-token",
    params(("id" = Uuid, Path, description = "Run id")),
    request_body = RefreshGithubToken,
    responses(
        (status = 200, body = GithubToken),
        (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody),
        (status = 404, body = ErrorBody),
        (status = 409, body = ErrorBody, description = "The attempt is not the runner's running attempt."),
        (status = 503, body = ErrorBody, description = "No token could be minted."),
    )
)]
async fn github_token(
    State(state): State<AppState>,
    actor: Actor,
    Path(id): Path<Uuid>,
    Json(request): Json<RefreshGithubToken>,
) -> Result<Json<GithubToken>, ApiError> {
    let runner = runner_of(&state, &actor).await?;
    ensure_run(&state.pool, &actor, id).await?;
    hold_attempt(&state.pool, id, request.attempt, &runner).await?;
    let github = (state.github.as_ref()).ok_or_else(|| {
        ApiError::GithubTokenUnavailable("the server has no GitHub App credentials".into())
    })?;
    let mut conn = state.pool.acquire().await?;
    let Repository { owner, name, .. } = repository(&mut conn, actor.workspace_id).await?;
    drop(conn);
    let token = (github.run_token(&owner, &name).await)
        .map_err(|error| ApiError::GithubTokenUnavailable(format!("{error:#}")))?;
    Ok(Json(token))
}

/// Say that the run cannot go on without a person, and what the person must do: stored on the
/// run, so its end dispatches the issue nothing more (`queue_redispatch`), and posted on the
/// issue as the agent's comment. Only the run's own agent, while its run runs: another run is
/// 403, and its own run once ended is 409 (`Actor` lets that token through to this route
/// alone). Nobody in a role the issue's status locks may say it, as for comments.
#[utoipa::path(
    post,
    path = "/api/runs/{id}/waiting",
    params(("id" = Uuid, Path, description = "Run id")),
    request_body = WaitForPerson,
    responses(
        (status = 204, description = "Stored, and commented."),
        (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody, description = "Not this run's agent."),
        (status = 404, body = ErrorBody),
        (status = 409, body = ErrorBody, description = "The run has ended, or the issue's status is locked to agents."),
        (status = 422, body = ErrorBody, description = "The reason is empty or too long."),
    )
)]
async fn wait_for_person(
    State(state): State<AppState>,
    actor: Actor,
    Path(id): Path<Uuid>,
    Json(request): Json<WaitForPerson>,
) -> Result<StatusCode, ApiError> {
    let reason = request.reason.trim();
    if reason.is_empty() || reason.chars().count() > MAX_WAITING_REASON_CHARS {
        return Err(ApiError::Invalid(format!(
            "a reason is 1 to {MAX_WAITING_REASON_CHARS} characters"
        )));
    }
    ensure_run(&state.pool, &actor, id).await?;
    let mut tx = state.pool.begin().await?;
    // The workspace, then the issue, as `finish_run` locks them, so the run cannot end
    // between the check and the write.
    lock_workspace(&mut tx, actor.workspace_id).await?;
    let run = sqlx::query!(
        "SELECT i.id AS issue_id, i.status AS issue_status, r.outcome FROM run r
         JOIN issue i ON i.id = r.issue_id JOIN actor a ON a.run_id = r.id
         WHERE r.id = $1 AND a.id = $2
         FOR UPDATE OF i",
        id,
        actor.id,
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::Forbidden(format!("only run {id}'s agent says it waits")))?;
    if run.outcome != RunOutcome::Running.as_str() {
        return Err(ApiError::StaleAttempt(format!("run {id} has ended")));
    }
    let (workflow, _) = active_workflow(&mut tx, actor.workspace_id).await?;
    workflow.check_unlocked(&run.issue_status, actor.role)?;
    sqlx::query!(
        "UPDATE run SET waiting_reason = $2 WHERE id = $1",
        id,
        reason
    )
    .execute(&mut *tx)
    .await?;
    record_comment(
        &mut tx,
        &actor,
        &run.issue_id,
        &signed_by_agent(reason),
        None,
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// After an attempt of the run ended with `outcome`: ends the run with it and dispatches the
/// issue again (`queue_redispatch`), or keeps the run running for its next attempt. A run is
/// kept when the attempt was lost, `lost` is its number and it is not the run's
/// `ATTEMPTS_PER_RUN`th, and the issue is still in the status the run is for, which the
/// active workflow does not lock to agents or runners: a person can move it, which ends the
/// run (`queue_dispatch`). A kept run has no running attempt, so `select_claimable` offers
/// its next one; its agent actor gets a token nobody holds, so the lost attempt's agent can
/// do nothing. The caller holds the workspace's lock, then the issue's, and has just ended
/// the attempt. `finish_run` and `expire_lease` both decide here. An attempt that hit its
/// agent's usage limit ends the run, with `resets_at` for `queue_redispatch`.
async fn settle_run(
    tx: &mut sqlx::PgConnection,
    workspace_id: Uuid,
    run_id: Uuid,
    outcome: RunOutcome,
    lost: Option<i32>,
    resets_at: Option<DateTime<Utc>>,
) -> sqlx::Result<()> {
    if lost.is_some_and(|attempt| attempt < ATTEMPTS_PER_RUN) {
        let run = sqlx::query!(
            "SELECT r.status, r.prompt, i.status AS issue_status FROM run r JOIN issue i ON i.id = r.issue_id
             WHERE r.id = $1",
            run_id,
        )
        .fetch_one(&mut *tx)
        .await?;
        // A workflow this server cannot read ends the run; `queue_redispatch` logs it. A run
        // from before prompts were stored has none to give its next attempt, so it ends too.
        let kept = run.status == run.issue_status
            && !run.prompt.is_empty()
            && active_workflow(tx, workspace_id)
                .await
                .is_ok_and(|(active, _)| {
                    [Role::Agent, Role::Runner]
                        .into_iter()
                        .all(|role| active.check_unlocked(&run.status, role).is_ok())
                });
        if kept {
            return set_agent_token(tx, run_id, &new_token()).await;
        }
    }
    sqlx::query!(
        "UPDATE run SET outcome = $2, finished_at = now() WHERE id = $1",
        run_id,
        outcome.as_str(),
    )
    .execute(&mut *tx)
    .await?;
    queue_redispatch(tx, workspace_id, run_id, resets_at).await
}

/// Dispatch again. The caller has just ended the run in this transaction, whatever the
/// outcome, and holds the workspace's lock and then its issue's. When the issue is still
/// where the run's dispatch put it (that dispatch's transition is the issue's latest),
/// queues the next dispatch of that transition, for the same agent; `dispatch_workflow`
/// gives its run the prompt of the transition's workflow version. Nothing is queued for a
/// run without a dispatch, an issue that moved, a status the active workflow now locks to
/// agents or runners or gives no dispatch rule (the test `queue_dispatch` makes on entry), or
/// run `RUNS_PER_ENTRY` of the entry, or a run whose agent said it waits on a person
/// (`wait_for_person`): that issue waits on a person. Nor when the active workflow cannot be
/// read, which is logged. `get_issue` and the inbox read the same three rules, the latest
/// transition, the limit (`runs_ended`) and the waiting reason, to say so; change them together.
///
/// A run that ended on its agent's usage limit, `resets_at`, was not the issue's failure and
/// takes no run of the entry: its own dispatch waits again, claimable from `resets_at`, so the
/// next run has the same number.
///
/// Once per run: an attempt leaves `running` once, under its row lock, so one transaction
/// gets here; and `dispatch` is unique on the transition and the run number.
async fn queue_redispatch(
    tx: &mut sqlx::PgConnection,
    workspace_id: Uuid,
    run_id: Uuid,
    resets_at: Option<DateTime<Utc>>,
) -> sqlx::Result<()> {
    let run = sqlx::query!(
        "SELECT status, waiting_reason FROM run WHERE id = $1",
        run_id
    )
    .fetch_one(&mut *tx)
    .await?;
    if run.waiting_reason.is_some() {
        return Ok(());
    }
    let status = run.status;
    // Ending the run must commit whatever the stored workflow looks like: one this server
    // cannot read queues nothing, and the run still ends.
    let active = match active_workflow(tx, workspace_id).await {
        Ok((active, _)) => active,
        Err(error) => {
            tracing::error!(run = %run_id, %error, "not dispatched again: the active workflow cannot be read");
            return Ok(());
        }
    };
    let unlocked = |role| active.check_unlocked(&status, role).is_ok();
    if active.dispatch_for(&status).is_none() || !unlocked(Role::Agent) || !unlocked(Role::Runner) {
        return Ok(());
    }
    if let Some(resets_at) = resets_at {
        sqlx::query!(
            "UPDATE dispatch d SET run_id = NULL, not_before = $2
             WHERE d.run_id = $1
               AND d.transition_id = (SELECT e.transition_id FROM event e
                                      WHERE e.issue_id = d.issue_id AND e.kind = 'transition'
                                      ORDER BY e.seq DESC LIMIT 1)",
            run_id,
            resets_at,
        )
        .execute(&mut *tx)
        .await?;
        return Ok(());
    }
    sqlx::query!(
        "INSERT INTO dispatch (issue_id, transition_id, agent, run_number)
         SELECT d.issue_id, d.transition_id, d.agent, d.run_number + 1 FROM dispatch d
         WHERE d.run_id = $1 AND d.run_number < $2
           AND d.transition_id = (SELECT e.transition_id FROM event e
                                  WHERE e.issue_id = d.issue_id AND e.kind = 'transition'
                                  ORDER BY e.seq DESC LIMIT 1)",
        run_id,
        RUNS_PER_ENTRY,
    )
    .execute(&mut *tx)
    .await?;
    Ok(())
}

/// Fails every running attempt whose lease has lapsed as lost, and settles its run
/// (`settle_run`): the run gets its next attempt, or fails and the issue is dispatched again
/// if it has not moved. Returns how many attempts expired.
/// Nothing wakes a waiting claim for the attempt or dispatch this makes claimable: it finds
/// it at its next look, within `CLAIM_RETRY`.
pub async fn expire_leases(pool: &PgPool) -> sqlx::Result<u64> {
    let lapsed = sqlx::query!(
        "SELECT a.run_id, a.issue_id, r.workspace_id FROM attempt a JOIN run r ON r.id = a.run_id
         WHERE a.state = 'running' AND a.lease_expires_at < now()"
    )
    .fetch_all(pool)
    .await?;
    let mut expired = 0;
    // One transaction per attempt: the workspace and issue rows are locked before the
    // attempt row, as everywhere, which one UPDATE over every lapsed attempt cannot do. One
    // attempt's error is logged and the sweep goes on to the rest.
    for attempt in lapsed {
        match expire_lease(
            pool,
            attempt.workspace_id,
            &attempt.issue_id,
            attempt.run_id,
        )
        .await
        {
            Ok(true) => expired += 1,
            Ok(false) => {}
            Err(error) => tracing::error!(run = %attempt.run_id, %error, "expire lease"),
        }
    }
    Ok(expired)
}

/// `expire_leases` for one run. False when its attempt finished, or a heartbeat renewed it,
/// since it was listed.
async fn expire_lease(
    pool: &PgPool,
    workspace_id: Uuid,
    issue_id: &str,
    run_id: Uuid,
) -> sqlx::Result<bool> {
    let mut tx = pool.begin().await?;
    lock_workspace(&mut tx, workspace_id).await?;
    sqlx::query!("SELECT FROM issue WHERE id = $1 FOR UPDATE", issue_id)
        .fetch_one(&mut *tx)
        .await?;
    let failed = sqlx::query!(
        "UPDATE attempt SET state = 'failed', finished_at = now(),
                failure_reason = 'lease expired: runner ' || runner.name
                                 || ' stopped sending heartbeats'
         FROM runner
         WHERE runner.id = attempt.runner_id AND attempt.run_id = $1
           AND attempt.state = 'running' AND attempt.lease_expires_at < now()
         RETURNING attempt.attempt, runner.actor_id",
        run_id,
    )
    .fetch_optional(&mut *tx)
    .await?;
    let Some(failed) = failed else {
        return Ok(false);
    };
    settle_run(
        &mut tx,
        workspace_id,
        run_id,
        RunOutcome::Failed,
        Some(failed.attempt),
        None,
    )
    .await?;
    record_row_event(&mut tx, workspace_id, issue_id, failed.actor_id, "run").await?;
    tx.commit().await?;
    Ok(true)
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
    model: Option<String>,
    reported_model: Option<String>,
    runner_id: Uuid,
    runner_name: String,
    failure_reason: Option<String>,
    duration_ms: Option<i64>,
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    cost_usd: Option<String>,
}

/// `[0-9]+` with an optional `.[0-9]+`, at most 30 characters: no sign, exponent, `NaN` or
/// spaces, so Postgres stores the digits as they are.
fn plain_decimal(text: &str) -> bool {
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    text.len() <= 30
        && match text.split_once('.') {
            Some((whole, fraction)) => digits(whole) && digits(fraction),
            None => digits(text),
        }
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

/// Runs of the workspace, newest first, each with its latest attempt, every attempt and its
/// log tail. Filter by issue or by run id.
async fn load_runs(
    pool: &PgPool,
    actor: &Actor,
    issue_id: Option<&str>,
    run_id: Option<Uuid>,
) -> Result<Vec<Run>, ApiError> {
    let rows = sqlx::query_as!(
        RunRow,
        r#"SELECT r.id, r.issue_id, r.agent, r.branch, r.outcome, r.started_at, r.finished_at,
                  a.attempt AS "attempt!", a.model, a.reported_model, a.runner_id AS "runner_id!",
                  n.name AS "runner_name!", a.failure_reason, a.duration_ms,
                  a.input_tokens, a.output_tokens, a.cost_usd::text AS cost_usd
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
    let run_ids: Vec<Uuid> = rows.iter().map(|row| row.id).collect();
    let mut attempts: HashMap<Uuid, Vec<AttemptInfo>> = HashMap::new();
    for row in sqlx::query!(
        "SELECT a.run_id, a.attempt, a.runner_id, n.name AS runner_name, a.state, a.model,
                a.reported_model, a.failure_reason, a.started_at, a.finished_at, a.duration_ms, a.input_tokens,
                a.output_tokens, a.cost_usd::text AS cost_usd
         FROM attempt a JOIN runner n ON n.id = a.runner_id
         WHERE a.run_id = ANY($1) ORDER BY a.run_id, a.attempt",
        &run_ids,
    )
    .fetch_all(pool)
    .await?
    {
        attempts.entry(row.run_id).or_default().push(AttemptInfo {
            attempt: row.attempt,
            model: row.model,
            reported_model: row.reported_model,
            runner: RunnerRef {
                id: row.runner_id,
                name: row.runner_name,
            },
            duration_ms: row.duration_ms,
            input_tokens: row.input_tokens,
            output_tokens: row.output_tokens,
            cost_usd: row.cost_usd,
            state: parse_outcome(&row.state)?,
            failure_reason: row.failure_reason,
            started_at: row.started_at,
            finished_at: row.finished_at,
        });
    }
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
        let outcome = parse_outcome(&row.outcome)?;
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
            model: row.model,
            reported_model: row.reported_model,
            runner: RunnerRef {
                id: row.runner_id,
                name: row.runner_name,
            },
            failure_reason: row.failure_reason,
            duration_ms: row.duration_ms,
            input_tokens: row.input_tokens,
            output_tokens: row.output_tokens,
            cost_usd: row.cost_usd,
            log_tail: tail.into_iter().map(LogLine::from).collect(),
            attempts: attempts.remove(&row.id).unwrap_or_default(),
        });
    }
    Ok(runs)
}

fn parse_outcome(value: &str) -> sqlx::Result<RunOutcome> {
    RunOutcome::parse(value)
        .ok_or_else(|| sqlx::Error::Decode(format!("unknown run outcome `{value}`").into()))
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

#[cfg(test)]
mod tests {
    use super::reported_model;

    #[test]
    fn reported_model_reads_each_agents_line_and_nothing_else() {
        let cases = [
            (
                r#"{"type":"system","subtype":"init","session_id":"s","model":"claude-opus-5-5"}"#,
                Some("claude-opus-5-5"),
            ),
            (
                r#"{"type":"assistant","message":{"model":"claude-haiku-5-5","content":[]},"parent_tool_use_id":null}"#,
                Some("claude-haiku-5-5"),
            ),
            (
                r#"{"type":"assistant","message":{"model":"claude-sonnet-5-5"},"parent_tool_use_id":"toolu_1"}"#,
                None,
            ),
            (
                r#"{"method":"thread/started","params":{"thread":{"id":"t","modelProvider":"openai","model":"gpt-6-luna"}}}"#,
                Some("gpt-6-luna"),
            ),
            (r#"{"type":"system","subtype":"hook_started"}"#, None),
            (
                r#"{"type":"system","subtype":"init","model":"a b\u001b[31m"}"#,
                None,
            ),
            (
                r#"{"id":2,"result":{"thread":{"model":"gpt-6-luna"}}}"#,
                None,
            ),
            ("disabled the 3 skills Codex found", None),
        ];
        for (line, model) in cases {
            assert_eq!(reported_model(line).as_deref(), model, "{line}");
        }
    }
}
