// Copied from kata-symphony apps/symphony/src/orchestrator.rs at e7c160f5; trimmed: only the failure-retry backoff (`FAILURE_RETRY_BASE_MS`, `retry_delay_ms`) is copied. The claim loop, concurrency limit, heartbeat and attempt lifecycle below are written for Punchlist against the server API (the original polls a tracker); tracker polling, supervisor, triage, docker/ssh workers, shared context, notifications and the workflow store are dropped.
//! The claim loop: register, resume or release the attempts this runner held before it
//! restarted, heartbeat, claim up to `max_concurrent` issues and run each attempt, backing
//! off on server errors.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use punchlist_api::{
    AppendLog, Claim, FinishRun, Heartbeat, HeldAttempt, Lease, RegisterRunner, RunOutcome,
};
use punchlist_client::{Client, ClientError};
use punchlist_core::unknown_agent_kind;
use tokio::sync::{Semaphore, mpsc, watch};
use tokio::task::JoinSet;
use uuid::Uuid;

use crate::RunnerConfig;
use crate::agent::{
    ATTEMPT_GRACE, ATTEMPT_MARKER, AgentExit, AgentResult, build_prompt, claude_args, end_attempt,
    kill_runner, run_claude, write_mcp_config,
};
use crate::codex::{CodexHome, mcp_servers, run_codex, thread_params};
use crate::github::{GithubAccess, refresh_at};
use crate::worktree::Worktrees;

/// The agent keys this runner has an adapter for.
const AGENTS: [&str; 2] = ["claude-code", "codex"];

/// What a resumed Claude Code session is told on stdin.
const RESUME_PROMPT: &str = "Punchlist's runner restarted while you were working on this issue. \
Carry on from where you stopped.";

pub const FAILURE_RETRY_BASE_MS: i64 = 10_000;
pub const MAX_RETRY_BACKOFF_MS: i64 = 300_000;

/// Delay before retry number `attempt` (1-based) after a failure: the base doubled per
/// attempt, capped at `max_backoff_ms`.
pub fn retry_delay_ms(attempt: u32, max_backoff_ms: i64) -> i64 {
    let safe_attempt = attempt.max(1);
    let power = safe_attempt.saturating_sub(1).min(10);
    let exponential = FAILURE_RETRY_BASE_MS.saturating_mul(1_i64 << power);
    exponential.min(max_backoff_ms)
}

const LOG_FLUSH_INTERVAL: Duration = Duration::from_millis(500);
const LOG_BATCH_LINES: usize = 50;
/// A batch closes once its lines reach this many bytes, well under the server's 2 MiB
/// request body limit even with one more clipped line.
const LOG_BATCH_BYTES: usize = 1024 * 1024;
/// A heartbeat that has not answered by now is abandoned so the next tick can renew the
/// leases (30 s) in time.
const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(10);
/// The server refuses lines over 64 KiB; longer ones are cut to this many bytes.
const LOG_LINE_MAX_BYTES: usize = 60 * 1024;
/// Tries per batch before it is dropped, so a server that keeps failing cannot hold the
/// flusher (and with it the attempt) forever.
const LOG_BATCH_TRIES: u32 = 8;
/// Lines buffered between the agent's pipes and the log flusher. When the server is slow the
/// readers wait here, which pauses the agent's output instead of growing memory.
const LOG_CHANNEL_LINES: usize = 1000;

/// Cuts a line the server would refuse for its size, on a character boundary.
fn clip(mut line: String) -> String {
    if line.len() > LOG_LINE_MAX_BYTES {
        line.truncate(line.floor_char_boundary(LOG_LINE_MAX_BYTES));
        line.push_str("… [truncated]");
    }
    line
}

/// A 4xx other than a refusal: sending the same batch again gets the same answer.
fn is_permanent(error: &ClientError) -> bool {
    matches!(error, ClientError::Api { status, .. } if status.is_client_error())
}

fn is_code(error: &ClientError, wanted: &str) -> bool {
    matches!(error, ClientError::Refused(body) if body.code == wanted)
}

/// The attempts this runner is working on, each with the signal that stops its agent. The
/// heartbeat renews exactly these leases, and stops the ones the server answers are stale.
type Held = Arc<Mutex<HashMap<(Uuid, i32), Arc<watch::Sender<bool>>>>>;

/// Removes its attempt from the held set when dropped, however the attempt ends.
struct HeldGuard {
    held: Held,
    key: (Uuid, i32),
}

impl HeldGuard {
    fn new(held: Held, key: (Uuid, i32), stop: Arc<watch::Sender<bool>>) -> HeldGuard {
        held.lock().expect("held set").insert(key, stop);
        HeldGuard { held, key }
    }
}

impl Drop for HeldGuard {
    fn drop(&mut self) {
        self.held.lock().expect("held set").remove(&self.key);
    }
}

struct Context {
    config: RunnerConfig,
    runner_id: String,
    client: Client,
    worktrees: Worktrees,
    shutdown: watch::Receiver<bool>,
}

pub async fn run(
    config: RunnerConfig,
    shutdown: impl std::future::Future<Output = ()> + Send,
) -> anyhow::Result<()> {
    run_with_worktrees(Worktrees::new(config.state_dir.clone()), config, shutdown).await
}

pub(crate) async fn run_with_worktrees(
    worktrees: Worktrees,
    config: RunnerConfig,
    shutdown: impl std::future::Future<Output = ()> + Send,
) -> anyhow::Result<()> {
    // A word that is no agent kind is a typo; a kind with no adapter here is a missing feature.
    if let Some(unknown) = (config.agents.iter()).find_map(|a| unknown_agent_kind(a)) {
        anyhow::bail!(unknown);
    }
    if let Some(unsupported) = (config.agents.iter()).find(|a| !AGENTS.contains(&a.as_str())) {
        anyhow::bail!(
            "unsupported agent `{unsupported}`; this runner can run {}",
            AGENTS.join(" and ")
        );
    }
    for dir in ["repos", "worktrees", "mcp", "codex-home"] {
        let dir = config.state_dir.join(dir);
        std::fs::create_dir_all(&dir)
            .map_err(|e| anyhow::anyhow!("cannot create {}: {e}", dir.display()))?;
    }
    let mut person = Client::new(&config.server_url, &config.token)?;
    if let Some(workspace) = &config.workspace {
        person = person.in_workspace(workspace);
    }
    let registered = person
        .register_runner(&RegisterRunner {
            name: config.name.clone(),
            agents: config.agents.clone(),
            models: [
                ("claude-code", &config.model),
                ("codex", &config.codex_model),
            ]
            .into_iter()
            .filter_map(|(agent, model)| Some((agent.to_string(), model.clone()?)))
            .collect(),
        })
        .await?;
    let runner_id = registered.runner.id.to_string();
    tracing::info!(
        "registered runner {} ({runner_id}), lease {}s",
        registered.runner.name,
        registered.lease_seconds
    );
    let client = Client::new(&config.server_url, &registered.token)?;

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    // Set when the server stops taking this runner's token: another `pl runner start`
    // registered its name and holds the runner now, so this one stops before two processes
    // run agents in one worktree.
    let (displaced_tx, displaced) = watch::channel(false);
    let held: Held = Arc::default();
    let heartbeat = {
        let client = client.clone();
        let held = held.clone();
        let interval = config.heartbeat_interval;
        let name = config.name.clone();
        let runner_id = runner_id.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                let snapshot = Heartbeat {
                    held: held
                        .lock()
                        .expect("held set")
                        .keys()
                        .map(|&(run_id, attempt)| HeldAttempt { run_id, attempt })
                        .collect(),
                };
                match tokio::time::timeout(
                    HEARTBEAT_TIMEOUT,
                    client.heartbeat(&runner_id, &snapshot),
                )
                .await
                {
                    // The lease lapsed, so the issue may already have another run: stop
                    // this one's agent now, not at its next log batch.
                    Ok(Ok(reply)) => {
                        let held = held.lock().expect("held set");
                        for stale in reply.stale {
                            if let Some(stop) = held.get(&(stale.run_id, stale.attempt)) {
                                tracing::warn!(
                                    "run {}: lease lost; stopping the agent",
                                    stale.run_id
                                );
                                let _ = stop.send(true);
                            }
                        }
                    }
                    Ok(Err(ClientError::Api { status, .. })) if status.as_u16() == 401 => {
                        tracing::error!(
                            "the server no longer accepts this runner's token: another `pl runner start` registered the name `{name}`; stopping every agent and shutting down"
                        );
                        let _ = displaced_tx.send(true);
                        return;
                    }
                    Ok(Err(e)) => tracing::warn!("heartbeat failed: {e}"),
                    Err(_) => {
                        tracing::warn!("heartbeat timed out after {}s", HEARTBEAT_TIMEOUT.as_secs())
                    }
                }
            }
        })
    };

    let max_concurrent = config.max_concurrent.max(1);
    let claim_wait = config.claim_wait.as_secs().min(30) as u32;
    let ctx = Arc::new(Context {
        config,
        runner_id: runner_id.clone(),
        client,
        worktrees,
        shutdown: shutdown_rx,
    });
    let permits = Arc::new(Semaphore::new(max_concurrent));
    let mut attempts = JoinSet::new();
    let mut failures: u32 = 0;
    let shutdown = {
        let mut displaced = displaced.clone();
        async move {
            tokio::select! {
                () = shutdown => {}
                _ = displaced.wait_for(|d| *d) => {}
            }
        }
    };
    tokio::pin!(shutdown);

    // The attempts this runner held when it last stopped. Each Claude Code one whose
    // worktree is still on its branch carries on in its session; the rest are reported lost,
    // so their runs get their next attempts now rather than when the leases lapse.
    // Every agent an earlier process of this runner started goes first, lease lapsed or not:
    // it may still be at work in a worktree or on a branch another attempt is about to use.
    let left = kill_runner(&runner_id);
    if left > 0 {
        tracing::info!("killed {left} process(es) left from before the restart");
    }
    match ctx.client.leases(&runner_id).await {
        Ok(leases) => {
            for lease in leases {
                let run_id = lease.run_id.to_string();
                let worktree = match lease.agent.as_str() {
                    "claude-code" => {
                        (ctx.worktrees)
                            .existing(&lease.repository, &lease.branch, &lease.issue_id)
                            .await
                    }
                    _ => None,
                };
                let permit = permits.clone().try_acquire_owned().ok();
                let (Some(worktree), Some(permit)) = (worktree, permit) else {
                    release_lost(&ctx.client, &lease).await;
                    continue;
                };
                match ctx.client.resume_run(&run_id, lease.attempt).await {
                    Ok(claim) => {
                        let ctx = ctx.clone();
                        let stop = Arc::new(watch::channel(false).0);
                        let key = (claim.run_id, claim.attempt);
                        let guard = HeldGuard::new(held.clone(), key, stop.clone());
                        attempts.spawn(async move {
                            run_attempt(&ctx, claim, stop, Some(worktree)).await;
                            drop(guard);
                            drop(permit);
                        });
                    }
                    Err(e) if is_code(&e, "stale_attempt") => {
                        tracing::info!("run {run_id}: the lease lapsed before it was resumed")
                    }
                    Err(e) => {
                        tracing::warn!("run {run_id}: cannot resume: {e}; the lease will expire")
                    }
                }
            }
        }
        Err(e) => tracing::warn!("cannot read this runner's leases: {e}; they will expire"),
    }

    loop {
        let permit = tokio::select! {
            _ = &mut shutdown => break,
            Some(done) = attempts.join_next(), if !attempts.is_empty() => {
                if let Err(e) = done {
                    tracing::error!("attempt task failed: {e}");
                }
                continue;
            }
            permit = permits.clone().acquire_owned() => permit.expect("semaphore is open"),
        };
        let claimed = tokio::select! {
            _ = &mut shutdown => break,
            claimed = ctx.client.claim(claim_wait) => claimed,
        };
        match claimed {
            Ok(Some(claim)) => {
                failures = 0;
                let ctx = ctx.clone();
                let stop = Arc::new(watch::channel(false).0);
                let key = (claim.run_id, claim.attempt);
                let guard = HeldGuard::new(held.clone(), key, stop.clone());
                attempts.spawn(async move {
                    run_attempt(&ctx, claim, stop, None).await;
                    drop(guard);
                    drop(permit);
                });
            }
            Ok(None) => failures = 0,
            Err(e) if is_code(&e, "claim_taken") => {
                failures = 0;
                if let ClientError::Refused(body) = &e {
                    tracing::info!("claim taken: {}", body.message);
                }
            }
            Err(e) => {
                failures = failures.saturating_add(1);
                let delay = retry_delay_ms(failures, MAX_RETRY_BACKOFF_MS) as u64;
                tracing::warn!("claim failed: {e}; retrying in {delay} ms");
                drop(permit);
                tokio::select! {
                    _ = &mut shutdown => break,
                    _ = tokio::time::sleep(Duration::from_millis(delay)) => {}
                }
            }
        }
    }

    tracing::info!(
        "shutting down; stopping {} running attempt(s)",
        attempts.len()
    );
    let _ = shutdown_tx.send(true);
    while let Some(done) = attempts.join_next().await {
        if let Err(e) = done {
            tracing::error!("attempt task failed: {e}");
        }
    }
    heartbeat.abort();
    if *displaced.borrow() {
        anyhow::bail!(
            "another `pl runner start` registered runner `{}`; this one stopped",
            ctx.config.name
        );
    }
    Ok(())
}

/// Reports a held attempt this runner cannot carry on after a restart as lost, once. A
/// report that fails leaves the attempt to its lease.
async fn release_lost(client: &Client, lease: &Lease) {
    let run_id = lease.run_id.to_string();
    let request = FinishRun {
        attempt: lease.attempt,
        outcome: RunOutcome::Failed,
        reason: Some("runner restarted".into()),
        duration_ms: 0,
        input_tokens: None,
        output_tokens: None,
        cost_usd: None,
        lost: true,
        usage_limit: None,
    };
    match client.finish_run(&run_id, &request).await {
        Ok(_) => tracing::info!(
            "run {run_id}: attempt {} of {} lost in the restart",
            lease.attempt,
            lease.issue_id
        ),
        Err(e) => tracing::warn!("run {run_id}: cannot report the attempt lost: {e}"),
    }
}

/// Batches log lines: a flush after 500 ms, 50 lines or 1 MiB. A batch the server refuses
/// as invalid is dropped; one that keeps failing is retried with backoff, then dropped after
/// `LOG_BATCH_TRIES`. Once shutdown starts, a send gets `final_timeout`, and the first
/// failure abandons the rest of the log. Lines are numbered from `first_line`. Returns true
/// when the server said the attempt is stale, after signalling `stop`.
#[allow(clippy::too_many_arguments)]
async fn flush_logs(
    client: Client,
    run_id: String,
    attempt: i32,
    mut first_line: i64,
    mut lines: mpsc::Receiver<String>,
    stop: Arc<watch::Sender<bool>>,
    mut shutdown: watch::Receiver<bool>,
    final_timeout: Duration,
) -> bool {
    let mut pending: Vec<String> = Vec::new();
    // `first_line` is the number of the first pending line; the lines of an attempt are
    // numbered 1, 2, 3..., and a resumed attempt's go on after those the server stored.
    let mut stale = false;
    let mut closed = false;
    let mut tries: u32 = 0;
    let mut abandoned = false;
    // After the agent's output closes, keep going until the last batch is sent or dropped.
    while !closed || !pending.is_empty() {
        if pending.is_empty() {
            match lines.recv().await {
                Some(line) => pending.push(clip(line)),
                None => break,
            }
        }
        let deadline = tokio::time::Instant::now() + LOG_FLUSH_INTERVAL;
        let mut bytes: usize = pending.iter().map(String::len).sum();
        while !closed && pending.len() < LOG_BATCH_LINES && bytes < LOG_BATCH_BYTES {
            match tokio::time::timeout_at(deadline, lines.recv()).await {
                Ok(Some(line)) => {
                    let line = clip(line);
                    bytes += line.len();
                    pending.push(line);
                }
                Ok(None) => {
                    closed = true;
                    break;
                }
                Err(_) => break,
            }
        }
        let request = AppendLog {
            attempt,
            first_line,
            lines: pending.clone(),
        };
        let Some(sent) = until_shutdown_grace(
            client.append_log(&run_id, &request),
            &shutdown,
            final_timeout,
        )
        .await
        else {
            tracing::warn!("run {run_id}: shutting down; abandoning the rest of the log");
            abandoned = true;
            break;
        };
        match sent {
            Ok(()) => {
                first_line += pending.len() as i64;
                pending.clear();
                tries = 0;
            }
            Err(e) if is_code(&e, "stale_attempt") => {
                tracing::warn!("run {run_id}: lease lost; stopping the agent");
                stale = true;
                let _ = stop.send(true);
                break;
            }
            Err(e) if *shutdown.borrow() => {
                tracing::warn!(
                    "run {run_id}: cannot send log lines while shutting down: {e}; abandoning the rest of the log"
                );
                abandoned = true;
                break;
            }
            Err(e) => {
                tries += 1;
                let give_up = is_permanent(&e) || tries >= LOG_BATCH_TRIES;
                if give_up {
                    tracing::warn!(
                        "run {run_id}: dropping {} log line(s) after {tries} tries: {e}",
                        pending.len()
                    );
                    first_line += pending.len() as i64;
                    pending.clear();
                    tries = 0;
                } else {
                    tracing::warn!("run {run_id}: cannot send log lines, will retry: {e}");
                    let backoff = Duration::from_millis(250 * 2u64.pow(tries - 1));
                    tokio::select! {
                        _ = tokio::time::sleep(backoff.min(Duration::from_secs(8))) => {}
                        _ = shutdown.wait_for(|s| *s) => {}
                    }
                }
            }
        }
    }
    if stale || abandoned {
        // Keep draining so the agent's output pipe never blocks on a closed channel.
        while lines.recv().await.is_some() {}
    }
    stale
}

/// Runs one attempt and reports its outcome. `resume_in` is the worktree of an attempt this
/// runner held before it restarted: its agent carries on there in the attempt's session.
async fn run_attempt(
    ctx: &Context,
    claim: Claim,
    stop_tx: Arc<watch::Sender<bool>>,
    resume_in: Option<PathBuf>,
) {
    let start = Instant::now();
    let run_id = claim.run_id.to_string();
    let attempt = claim.attempt;
    // The branch is not checked yet, so it is logged escaped.
    tracing::info!(
        "{} {} for {} (run {run_id}, attempt {attempt}) on branch {:?}, model {:?}",
        if resume_in.is_some() {
            "resumed"
        } else {
            "claimed"
        },
        claim.issue.id,
        claim.agent,
        claim.branch,
        claim.model.as_deref().unwrap_or("the agent's default"),
    );

    let (lines_tx, lines_rx) = mpsc::channel(LOG_CHANNEL_LINES);
    let stop_rx = stop_tx.subscribe();
    let flusher = tokio::spawn(flush_logs(
        ctx.client.clone(),
        run_id.clone(),
        attempt,
        claim.next_line,
        lines_rx,
        stop_tx.clone(),
        ctx.shutdown.clone(),
        FINAL_REPORT_TIMEOUT,
    ));
    let forward = {
        let mut shutdown = ctx.shutdown.clone();
        let stop_tx = stop_tx.clone();
        tokio::spawn(async move {
            if shutdown.wait_for(|stopping| *stopping).await.is_ok() {
                let _ = stop_tx.send(true);
            }
        })
    };

    // The names the prompt prints are checked before git sees any of them.
    let prompt = build_prompt(
        &claim.issue,
        &claim.repository,
        &claim.branch,
        &claim.prompt,
    );
    let failed = |reason: String| {
        AgentExit::Finished(AgentResult {
            outcome: RunOutcome::Failed,
            reason: Some(reason),
            input_tokens: None,
            output_tokens: None,
            usage_limit: None,
            cost_usd: None,
        })
    };
    // The run acts on GitHub as the workspace's App when the server minted it a token; a
    // server without App credentials sends none, and the run uses the runner's own.
    let github = match (&claim.github_error, &claim.github) {
        (Some(error), _) => Err(format!("GitHub refused the run's token: {error}")),
        (None, Some(token)) => GithubAccess::create(&ctx.config.state_dir, &run_id, token)
            .map(|access| Some(Arc::new(access)))
            .map_err(|e| format!("cannot write the run's GitHub token: {e}")),
        (None, None) => Ok(None),
    };
    let refresher = match (&github, &claim.github) {
        (Ok(Some(access)), Some(token)) => Some(tokio::spawn(refresh_github_token(
            ctx.client.clone(),
            run_id.clone(),
            attempt,
            access.clone(),
            token.expires_at,
            lines_tx.clone(),
        ))),
        _ => None,
    };
    let mut env = match &github {
        Ok(Some(access)) => access.env(),
        _ => Vec::new(),
    };
    let attempt_id = format!("{run_id}-{attempt}");
    env.push((ATTEMPT_MARKER, Some(attempt_id.clone().into())));
    // Stopping the runner drops the preparation, which kills the git command it is running.
    let exit = match (resume_in, prompt) {
        _ if github.is_err() => {
            let reason = github.as_ref().err().cloned().unwrap_or_default();
            let _ = lines_tx.send(reason.clone()).await;
            failed(reason)
        }
        (Some(path), _) => {
            let resume = RESUME_PROMPT;
            run_agent(
                ctx,
                &claim,
                &env,
                &path,
                resume,
                true,
                lines_tx.clone(),
                stop_rx,
            )
            .await
        }
        (None, Err(reason)) => {
            let _ = lines_tx.send(reason.clone()).await;
            failed(reason)
        }
        (None, Ok(prompt)) => {
            let prepared = {
                let mut stop = stop_rx.clone();
                tokio::select! {
                    prepared = ctx.worktrees.prepare(&claim.repository, &claim.branch, &claim.merged_branches, &claim.issue.id, env.clone()) => Some(prepared),
                    _ = async { let _ = stop.wait_for(|stopped| *stopped).await; } => None,
                }
            };
            match prepared {
                None => AgentExit::Stopped,
                Some(Ok(path)) => {
                    tracing::info!("worktree ready: {}", path.display());
                    let _ = lines_tx
                        .send(format!(
                            "worktree {} on branch {}",
                            path.display(),
                            claim.branch
                        ))
                        .await;
                    run_agent(
                        ctx,
                        &claim,
                        &env,
                        &path,
                        &prompt,
                        false,
                        lines_tx.clone(),
                        stop_rx,
                    )
                    .await
                }
                Some(Err(reason)) => {
                    let reason = format!("worktree preparation failed: {reason}");
                    let _ = lines_tx.send(reason.clone()).await;
                    failed(reason)
                }
            }
        }
    };
    // Before the outcome, so the next run of the issue finds its ports free.
    let (processes, containers) = end_attempt(&attempt_id, ATTEMPT_GRACE).await;
    let line = format!(
        "attempt {attempt_id} ended: removed {processes} process(es) and {containers} container(s) it left"
    );
    tracing::info!("run {run_id}: {line}");
    let _ = lines_tx.send(line).await;
    if let Some(refresher) = refresher {
        // Its log sender has to go before the flusher can end.
        refresher.abort();
        let _ = refresher.await;
    }
    drop(github);
    drop(lines_tx);
    forward.abort();
    let stale = flusher.await.unwrap_or(false);

    // A stopped agent did not finish: the attempt is lost, and the run gets its next one.
    let (result, lost) = match exit {
        AgentExit::Finished(result) => (result, false),
        AgentExit::Stopped if stale => {
            tracing::warn!("run {run_id}: agent stopped, lease lost; not reporting an outcome");
            return;
        }
        AgentExit::Stopped => (
            AgentResult {
                outcome: RunOutcome::Failed,
                reason: Some("runner stopped".into()),
                input_tokens: None,
                output_tokens: None,
                usage_limit: None,
                cost_usd: None,
            },
            true,
        ),
    };
    tracing::info!(
        "agent exited for {} (run {run_id}): {}",
        claim.issue.id,
        result.reason.as_deref().unwrap_or("success")
    );
    if let Some(limit) = &result.usage_limit {
        let until =
            (limit.resets_at).map_or("15 minutes from now".to_string(), |at| at.to_string());
        tracing::warn!(
            "{} hit its usage limit: the server holds this runner's {} claims until {until}",
            claim.agent,
            claim.agent
        );
    }
    let request = FinishRun {
        attempt,
        outcome: result.outcome,
        reason: result.reason,
        duration_ms: start.elapsed().as_millis().min(i64::MAX as u128) as i64,
        input_tokens: result.input_tokens,
        output_tokens: result.output_tokens,
        cost_usd: result.cost_usd,
        lost,
        usage_limit: result.usage_limit,
    };
    finish_with_retry(
        &ctx.client,
        &run_id,
        &request,
        ctx.shutdown.clone(),
        FINAL_REPORT_TIMEOUT,
    )
    .await;
}

/// How long a failed token refresh waits before it asks again.
const GITHUB_TOKEN_RETRY: Duration = Duration::from_secs(30);

/// Keeps the run's GitHub token fresh: asks the server for the next one before the current
/// one expires (`refresh_at`) and writes it where the agent's `gh` and git read it, until
/// the attempt ends and this is aborted, or the attempt is no longer the runner's.
async fn refresh_github_token(
    client: Client,
    run_id: String,
    attempt: i32,
    access: Arc<GithubAccess>,
    mut expires_at: chrono::DateTime<chrono::Utc>,
    lines: mpsc::Sender<String>,
) {
    let mut received_at = chrono::Utc::now();
    loop {
        let wait = refresh_at(received_at, expires_at) - chrono::Utc::now();
        tokio::time::sleep(wait.to_std().unwrap_or_default()).await;
        let refreshed = match client.refresh_github_token(&run_id, attempt).await {
            Ok(token) => access
                .write(&token.token)
                .map(|()| token)
                .map_err(|e| e.to_string()),
            Err(e) if is_code(&e, "stale_attempt") => return,
            Err(e) => Err(e.to_string()),
        };
        match refreshed {
            Ok(token) => {
                let line = format!(
                    "refreshed the run's GitHub token; it expires at {}",
                    token.expires_at.to_rfc3339()
                );
                tracing::info!("run {run_id}: {line}");
                let _ = lines.send(line).await;
                received_at = chrono::Utc::now();
                expires_at = token.expires_at;
            }
            Err(e) => {
                let delay = GITHUB_TOKEN_RETRY.as_secs();
                tracing::warn!(
                    "run {run_id}: cannot refresh the GitHub token: {e}; retrying in {delay}s"
                );
                tokio::time::sleep(GITHUB_TOKEN_RETRY).await;
            }
        }
    }
}

/// Runs the agent the claim names in the worktree, with Punchlist's MCP server attached and
/// `env` over the runner's environment. `resume` carries on the attempt's Claude Code
/// session; only Claude Code is resumed.
#[allow(clippy::too_many_arguments)]
async fn run_agent(
    ctx: &Context,
    claim: &Claim,
    env: &[(&'static str, Option<std::ffi::OsString>)],
    worktree: &Path,
    prompt: &str,
    resume: bool,
    lines: mpsc::Sender<String>,
    stop: watch::Receiver<bool>,
) -> AgentExit {
    let config = &ctx.config;
    let failed = |reason: String| {
        AgentExit::Finished(AgentResult {
            outcome: RunOutcome::Failed,
            reason: Some(reason),
            input_tokens: None,
            output_tokens: None,
            usage_limit: None,
            cost_usd: None,
        })
    };
    let marker = &ctx.runner_id;
    match claim.agent.as_str() {
        "codex" => {
            // Outside the worktree, and removed when this returns or is dropped.
            let home = match CodexHome::create(&config.state_dir, &claim.run_id.to_string()) {
                Ok(home) => home,
                Err(e) => return failed(format!("cannot make the Codex home: {e}")),
            };
            // An older server sends neither: the runner's flags.
            let params = thread_params(
                claim.model.as_deref().or(config.codex_model.as_deref()),
                claim.effort.as_deref().unwrap_or(&config.codex_effort),
                mcp_servers(config, claim),
                worktree,
            );
            let command = &config.codex_command;
            run_codex(
                command,
                home.path(),
                env,
                params,
                worktree,
                prompt,
                marker,
                lines,
                stop,
            )
            .await
        }
        "claude-code" => {
            // Outside the worktree, so the agent cannot commit its token.
            let mcp_dir = config.state_dir.join("mcp");
            let run_id = claim.run_id.to_string();
            let mcp_config = match write_mcp_config(&mcp_dir, &run_id, config, claim) {
                Ok(mcp_config) => mcp_config,
                Err(e) => return failed(format!("cannot write the MCP config: {e}")),
            };
            let args = claude_args(
                claim.model.as_deref().or(config.model.as_deref()),
                Some(&mcp_config),
                claim.session_id,
                resume,
            );
            let exit = run_claude(
                &config.claude_command,
                &args,
                env,
                worktree,
                prompt,
                marker,
                lines,
                stop,
            )
            .await;
            let _ = std::fs::remove_file(&mcp_config);
            exit
        }
        other => failed(format!("this runner has no adapter for agent `{other}`")),
    }
}

/// Once shutdown starts, how long the outcome report may still take before the runner
/// gives up and leaves the attempt to its lease.
const FINAL_REPORT_TIMEOUT: Duration = Duration::from_secs(10);

/// Runs `fut` to the end, unless shutdown starts first: then it gets `final_timeout` more.
/// `None` when it ran out.
async fn until_shutdown_grace<T>(
    fut: impl std::future::Future<Output = T>,
    shutdown: &watch::Receiver<bool>,
    final_timeout: Duration,
) -> Option<T> {
    let mut shutdown = shutdown.clone();
    tokio::select! {
        out = fut => Some(out),
        _ = async {
            let _ = shutdown.wait_for(|s| *s).await;
            tokio::time::sleep(final_timeout).await;
        } => None,
    }
}

/// Reports the outcome, retrying with backoff. Once shutdown starts, a request gets
/// `final_timeout`, a backoff ends at once, and the first failure gives up: the lease
/// expires and the server fails the attempt.
async fn finish_with_retry(
    client: &Client,
    run_id: &str,
    request: &FinishRun,
    mut shutdown: watch::Receiver<bool>,
    final_timeout: Duration,
) {
    for failures in 1..=5u32 {
        let Some(done) =
            until_shutdown_grace(client.finish_run(run_id, request), &shutdown, final_timeout)
                .await
        else {
            tracing::error!(
                "run {run_id}: shutting down before the outcome was recorded; the lease will expire"
            );
            return;
        };
        match done {
            Ok(_) => {
                tracing::info!("finished run {run_id}: {}", request.outcome.as_str());
                return;
            }
            Err(e) if is_code(&e, "stale_attempt") => {
                tracing::warn!("run {run_id}: lease lost before the outcome was recorded");
                return;
            }
            Err(e) if *shutdown.borrow() => {
                tracing::error!(
                    "run {run_id}: cannot record the outcome while shutting down: {e}; the lease will expire"
                );
                return;
            }
            Err(e) => {
                let delay = retry_delay_ms(failures, MAX_RETRY_BACKOFF_MS) as u64;
                tracing::warn!(
                    "run {run_id}: cannot record the outcome: {e}; retrying in {delay} ms"
                );
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_millis(delay)) => {}
                    _ = shutdown.wait_for(|s| *s) => {}
                }
            }
        }
    }
    tracing::error!("run {run_id}: gave up recording the outcome; the lease will expire");
}

#[cfg(test)]
mod tests {
    use super::*;
    use punchlist_api::CreateIssue;
    use punchlist_server::{AppState, Bootstrap, bootstrap, router};

    fn finish_request() -> FinishRun {
        FinishRun {
            attempt: 1,
            outcome: RunOutcome::Succeeded,
            reason: None,
            duration_ms: 1,
            input_tokens: None,
            output_tokens: None,
            cost_usd: None,
            lost: false,
            usage_limit: None,
        }
    }

    /// Reports an outcome to `url` with a 200 ms final timeout, sends shutdown after
    /// `shutdown_after`, and returns how long the report took.
    async fn report_with_shutdown(url: &str, shutdown_after: Duration) -> Duration {
        let client = Client::new(url, "token").unwrap();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let started = Instant::now();
        let report = tokio::spawn(async move {
            finish_with_retry(
                &client,
                "run",
                &finish_request(),
                shutdown_rx,
                Duration::from_millis(200),
            )
            .await
        });
        tokio::time::sleep(shutdown_after).await;
        shutdown_tx.send(true).unwrap();
        report.await.unwrap();
        started.elapsed()
    }

    #[tokio::test]
    async fn shutdown_bounds_an_outcome_report_in_flight() {
        // Accepts connections and never answers, so the request hangs.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let took = report_with_shutdown(&url, Duration::from_millis(100)).await;
        assert!(took < Duration::from_secs(2), "took {took:?}");
        drop(listener);
    }

    #[tokio::test]
    async fn shutdown_skips_the_outcome_backoff() {
        // Nothing listens here, so the first request fails at once and a 10 s backoff starts.
        let url = {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            format!("http://{}", listener.local_addr().unwrap())
        };
        let took = report_with_shutdown(&url, Duration::from_millis(300)).await;
        assert!(took < Duration::from_secs(2), "took {took:?}");
    }

    #[tokio::test]
    async fn shutdown_ends_a_flusher_stuck_on_a_silent_server() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let client = Client::new(&url, "token").unwrap();
        let (tx, rx) = mpsc::channel(16);
        let (stop_tx, _stop_rx) = watch::channel(false);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let started = Instant::now();
        let flusher = tokio::spawn(flush_logs(
            client,
            "run".into(),
            1,
            1,
            rx,
            Arc::new(stop_tx),
            shutdown_rx,
            Duration::from_millis(200),
        ));
        tx.send("one".into()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(700)).await;
        shutdown_tx.send(true).unwrap();
        // The agent keeps writing after shutdown; the flusher must not block it.
        for n in 0..100 {
            tx.send(format!("late {n}")).await.unwrap();
        }
        drop(tx);
        assert!(!flusher.await.unwrap());
        assert!(started.elapsed() < Duration::from_secs(3));
        drop(listener);
    }

    #[test]
    fn clip_cuts_oversized_lines_on_a_char_boundary() {
        assert_eq!(clip("short".into()), "short");
        let long = "é".repeat(40 * 1024); // 80 KiB of two-byte characters
        let clipped = clip(long);
        assert!(clipped.ends_with("… [truncated]"));
        assert_eq!(clipped.len(), LOG_LINE_MAX_BYTES + "… [truncated]".len());
    }

    #[test]
    fn only_client_errors_are_permanent() {
        let api = |status: u16| ClientError::Api {
            status: reqwest::StatusCode::from_u16(status).unwrap(),
            code: "x".into(),
            message: "x".into(),
        };
        assert!(is_permanent(&api(422)));
        assert!(!is_permanent(&api(500)));
        assert!(!is_permanent(&ClientError::Refused(Box::new(
            punchlist_api::ErrorBody::new("stale_attempt", "x")
        ))));
    }

    /// The flusher against a real server: an oversized line is stored clipped, and the
    /// flusher ends once the agent's output closes.
    #[sqlx::test(migrator = "punchlist_server::MIGRATOR")]
    async fn the_flusher_stores_an_oversized_line_clipped_and_ends(pool: sqlx::PgPool) {
        let person = bootstrap(
            &pool,
            &Bootstrap {
                workspace_name: "Punchlist".into(),
                issue_prefix: "PL".into(),
                repository_owner: "gannonh".into(),
                repository_name: "punchlist-sandbox".into(),
                person_name: "Gannon".into(),
            },
        )
        .await
        .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let app = router(AppState::new(pool.clone()));
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let people = Client::new(&url, &person.token).unwrap();
        people
            .create_issue(&CreateIssue {
                title: "Log".into(),
                body: String::new(),
            })
            .await
            .unwrap();
        people.move_issue("PL-1", "todo").await.unwrap();
        people.move_issue("PL-1", "start").await.unwrap();
        let registered = people
            .register_runner(&RegisterRunner {
                name: "r1".into(),
                agents: vec!["claude-code".into()],
                models: Default::default(),
            })
            .await
            .unwrap();
        let runner = Client::new(&url, &registered.token).unwrap();
        let claim = runner.claim(0).await.unwrap().unwrap();

        let (tx, rx) = mpsc::channel(16);
        let (stop_tx, _stop_rx) = watch::channel(false);
        let run_id = claim.run_id.to_string();
        let flusher = tokio::spawn(flush_logs(
            runner.clone(),
            run_id.clone(),
            claim.attempt,
            claim.next_line,
            rx,
            Arc::new(stop_tx),
            watch::channel(false).1,
            Duration::from_secs(10),
        ));
        tx.send("short".into()).await.unwrap();
        // 40 lines clipped to 60 KiB each: 2.4 MiB, over the server's 2 MiB body limit if
        // they went in one request.
        let sender = tokio::spawn(async move {
            for _ in 0..40 {
                tx.send("x".repeat(70 * 1024)).await.unwrap();
            }
        });
        sender.await.unwrap();
        let stale = tokio::time::timeout(Duration::from_secs(10), flusher)
            .await
            .expect("the flusher ends")
            .unwrap();
        assert!(!stale);

        let log = runner.run_log(&run_id).await.unwrap();
        let lines: Vec<&str> = log.lines.iter().map(|l| l.line.as_str()).collect();
        assert_eq!(lines.len(), 41);
        assert_eq!(lines[0], "short");
        let clipped = format!("{}… [truncated]", "x".repeat(LOG_LINE_MAX_BYTES));
        assert!(lines[1..].iter().all(|line| *line == clipped));
    }

    /// Requests a fake server was sent: method, path and JSON body.
    type Seen = Arc<Mutex<Vec<(String, String, serde_json::Value)>>>;

    const RUNNER: Uuid = Uuid::from_u128(1);
    const RESUMED: Uuid = Uuid::from_u128(0xa);
    const MISSING: Uuid = Uuid::from_u128(0xb);
    const FRESH: Uuid = Uuid::from_u128(0xc);
    const RESUMED_SESSION: Uuid = Uuid::from_u128(0x5a);
    const FRESH_SESSION: Uuid = Uuid::from_u128(0x5c);

    fn fake_claim(run_id: Uuid, issue: &str, session_id: Uuid, next_line: i64) -> Claim {
        let now = chrono::Utc::now();
        Claim {
            run_id,
            attempt: 1,
            agent: "claude-code".into(),
            issue: punchlist_api::Issue {
                id: issue.into(),
                title: "Fix it".into(),
                body: String::new(),
                status: "in_progress".into(),
                status_name: "In Progress".into(),
                created_at: now,
                updated_at: now,
                assignee: None,
                labels: Vec::new(),
                parent: None,
                project: None,
                milestone: None,
                blocked_by: Vec::new(),
                blocks: Vec::new(),
                waiting_on_person: None,
                waiting_for_usage_limit: None,
                waiting_to_run: None,
                linear: None,
            },
            repository: crate::worktree::tests::repository(),
            branch: format!("feature/{}", issue.to_lowercase()),
            merged_branches: vec![],
            lease_expires_at: now,
            prompt: "Do the work.".into(),
            agent_token: "agent-token".into(),
            session_id,
            model: None,
            effort: None,
            next_line,
            github: Some(punchlist_api::GithubToken {
                token: "ghs_run".into(),
                expires_at: now + chrono::Duration::hours(1),
                bot_name: "app[bot]".into(),
                bot_email: "1+app[bot]@users.noreply.github.com".into(),
            }),
            github_error: None,
        }
    }

    /// A Punchlist server that holds two leases for runner `RUNNER`, resumes `RESUMED`,
    /// hands out `FRESH` on the first claim, and records every request. When `displaced`,
    /// it holds nothing and refuses the runner's heartbeats with 401, as after another
    /// registration of its name.
    async fn fake_server(seen: Seen, displaced: bool) -> String {
        use axum::http::{Method, StatusCode, Uri};
        use axum::response::IntoResponse;
        let claims = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let app = axum::Router::new().fallback(
            move |method: Method, uri: Uri, body: axum::body::Bytes| {
                let seen = seen.clone();
                let claims = claims.clone();
                async move {
                    let json = serde_json::from_slice(&body).unwrap_or_default();
                    let path = uri.path().to_string();
                    seen.lock()
                        .unwrap()
                        .push((method.to_string(), path.clone(), json));
                    let now = chrono::Utc::now();
                    let runner = punchlist_api::Runner {
                        id: RUNNER,
                        name: "sartre".into(),
                        agents: vec!["claude-code".into()],
                        registered_at: now,
                        last_heartbeat: now,
                        paused: Vec::new(),
                    };
                    let lease = |run_id, issue: &str| Lease {
                        run_id,
                        attempt: 1,
                        agent: "claude-code".into(),
                        issue_id: issue.into(),
                        branch: format!("feature/{}", issue.to_lowercase()),
                        repository: crate::worktree::tests::repository(),
                        lease_expires_at: now,
                    };
                    let run = punchlist_api::Run {
                        id: RESUMED,
                        issue_id: "PL-1".into(),
                        agent: "claude-code".into(),
                        agent_name: "Claude Code".into(),
                        branch: "feature/pl-1".into(),
                        outcome: RunOutcome::Succeeded,
                        started_at: now,
                        finished_at: Some(now),
                        attempt: 1,
                        model: None,
                        reported_model: None,
                        runner: punchlist_api::RunnerRef {
                            id: RUNNER,
                            name: "sartre".into(),
                        },
                        failure_reason: None,
                        duration_ms: None,
                        input_tokens: None,
                        output_tokens: None,
                        cost_usd: None,
                        log_tail: vec![],
                        attempts: vec![],
                    };
                    let resume = format!("/api/runs/{RESUMED}/resume");
                    match (method.as_str(), path.as_str()) {
                        ("POST", "/api/runners") => axum::Json(punchlist_api::RegisteredRunner {
                            runner,
                            token: "runner-token".into(),
                            lease_seconds: 30,
                        })
                        .into_response(),
                        ("GET", _) if displaced => axum::Json(Vec::<Lease>::new()).into_response(),
                        ("POST", p) if displaced && p.ends_with("/heartbeat") => {
                            StatusCode::UNAUTHORIZED.into_response()
                        }
                        ("POST", "/api/runs/claim") if displaced => {
                            tokio::time::sleep(Duration::from_millis(100)).await;
                            StatusCode::NO_CONTENT.into_response()
                        }
                        ("GET", p) if p == format!("/api/runners/{RUNNER}/leases") => {
                            axum::Json(vec![lease(RESUMED, "PL-1"), lease(MISSING, "PL-2")])
                                .into_response()
                        }
                        ("POST", p) if p == resume => {
                            axum::Json(fake_claim(RESUMED, "PL-1", RESUMED_SESSION, 4))
                                .into_response()
                        }
                        ("POST", "/api/runs/claim") => {
                            if claims.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                                // The dispatch rule's model, over the runner's (none here).
                                axum::Json(Claim {
                                    model: Some("haiku".into()),
                                    ..fake_claim(FRESH, "PL-3", FRESH_SESSION, 1)
                                })
                                .into_response()
                            } else {
                                tokio::time::sleep(Duration::from_millis(100)).await;
                                StatusCode::NO_CONTENT.into_response()
                            }
                        }
                        ("POST", p) if p.ends_with("/heartbeat") => {
                            axum::Json(punchlist_api::HeartbeatReply {
                                runner,
                                stale: vec![],
                            })
                            .into_response()
                        }
                        ("POST", p) if p.ends_with("/finish") => axum::Json(run).into_response(),
                        _ => StatusCode::NO_CONTENT.into_response(),
                    }
                }
            },
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        url
    }

    fn fake_config(server_url: String, state_dir: PathBuf, claude: PathBuf) -> RunnerConfig {
        RunnerConfig {
            server_url,
            token: "person-token".into(),
            workspace: None,
            name: "sartre".into(),
            agents: vec!["claude-code".into()],
            state_dir,
            claude_command: claude,
            mcp_command: "pl".into(),
            model: None,
            codex_command: "codex".into(),
            codex_model: None,
            codex_effort: "medium".into(),
            max_concurrent: 2,
            heartbeat_interval: Duration::from_secs(1),
            claim_wait: Duration::ZERO,
        }
    }

    #[tokio::test]
    async fn an_unknown_kind_and_a_kind_without_an_adapter_fail_before_registering() {
        let temp = tempfile::tempdir().unwrap();
        // Nothing listens there: a call to the server would fail with a different error.
        let refused = |agent: &str| {
            let mut config = fake_config(
                "http://127.0.0.1:9".into(),
                temp.path().into(),
                "claude".into(),
            );
            config.agents = vec!["claude-code".into(), agent.into()];
            run_with_worktrees(
                Worktrees::new(temp.path().into()),
                config,
                std::future::pending(),
            )
        };
        assert_eq!(
            refused("opncode").await.unwrap_err().to_string(),
            "unknown agent kind `opncode`; the kinds are claude-code, codex, opencode, pi, cursor"
        );
        assert_eq!(
            refused("pi").await.unwrap_err().to_string(),
            "unsupported agent `pi`; this runner can run claude-code and codex"
        );
    }

    #[tokio::test]
    async fn a_runner_whose_name_was_registered_again_shuts_down() {
        let temp = tempfile::tempdir().unwrap();
        let url = fake_server(Arc::default(), true).await;
        let config = fake_config(url, temp.path().into(), "claude".into());
        let ran = tokio::time::timeout(
            Duration::from_secs(10),
            run_with_worktrees(
                Worktrees::new(temp.path().into()),
                config,
                std::future::pending(),
            ),
        )
        .await
        .expect("the runner stops by itself");
        assert_eq!(
            ran.unwrap_err().to_string(),
            "another `pl runner start` registered runner `sartre`; this one stopped"
        );
        // It made its state directory's four parts before registering.
        let mut made: Vec<_> = std::fs::read_dir(temp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        made.sort();
        assert_eq!(made, ["codex-home", "mcp", "repos", "worktrees"]);
    }

    /// A runner restarts holding two Claude Code attempts: it resumes the one whose worktree
    /// is still on its branch in the attempt's session, reports the one with no worktree
    /// lost, and starts a fresh claim in a new session.
    #[tokio::test]
    async fn a_restarted_runner_resumes_an_intact_worktree_and_reports_a_missing_one_lost() {
        let temp = tempfile::tempdir().unwrap();
        let origin = temp.path().join("origin");
        crate::worktree::tests::make_origin(&origin);
        let root = temp.path().join("root");
        let remote = origin.to_string_lossy().to_string();
        let worktrees = Worktrees::with_remote_base(root.clone(), remote);
        let repository = crate::worktree::tests::repository();
        (worktrees
            .prepare(&repository, "feature/pl-1", &[], "PL-1", vec![])
            .await)
            .unwrap();
        let argv = temp.path().join("argv");
        let github = temp.path().join("github");
        let detached = temp.path().join("detached");
        let bin = temp.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        // Each agent also leaves a `sleep` outside its process group, as `setsid pnpm dev`
        // does, and records its pid and attempt.
        let claude = crate::agent::tests::fake_agent(
            &bin,
            &format!(
                r#"echo "$@" >> {}
test "$1" = --probe || echo "${{GH_TOKEN-unset}} $(cat "$PUNCHLIST_GITHUB_TOKEN_FILE") $GIT_AUTHOR_NAME" >> {}
setsid sleep 300 </dev/null >/dev/null 2>&1 &
echo "$! $PUNCHLIST_ATTEMPT" >> {}
cat > /dev/null
echo '{{"type":"result","subtype":"success","is_error":false}}'"#,
                argv.display(),
                github.display(),
                detached.display()
            ),
        );
        let seen: Seen = Arc::default();
        let url = fake_server(seen.clone(), false).await;
        let config = fake_config(url, root, claude);
        let finished = |seen: &Seen, run: Uuid| {
            let path = format!("/api/runs/{run}/finish");
            seen.lock().unwrap().iter().any(|(_, p, _)| *p == path)
        };
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let runner = tokio::spawn(run_with_worktrees(worktrees, config, async {
            let _ = stopped.await;
        }));
        let deadline = Instant::now() + Duration::from_secs(20);
        while !(finished(&seen, RESUMED) && finished(&seen, FRESH)) {
            assert!(Instant::now() < deadline, "{:?}", seen.lock().unwrap());
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        stop.send(()).unwrap();
        runner.await.unwrap().unwrap();

        let seen = seen.lock().unwrap().clone();
        let startup: Vec<(&str, &str, &serde_json::Value)> = (seen.iter())
            .filter(|(_, p, _)| !p.ends_with("/heartbeat") && !p.ends_with("/log"))
            .map(|(m, p, b)| (m.as_str(), p.as_str(), b))
            .take(4)
            .collect();
        let leases = format!("/api/runners/{RUNNER}/leases");
        let resume = format!("/api/runs/{RESUMED}/resume");
        let lost = format!("/api/runs/{MISSING}/finish");
        let register = serde_json::json!({"name": "sartre", "agents": ["claude-code"]});
        assert_eq!(
            startup,
            [
                ("POST", "/api/runners", &register),
                ("GET", leases.as_str(), &serde_json::Value::Null),
                ("POST", resume.as_str(), &serde_json::json!({"attempt": 1})),
                (
                    "POST",
                    lost.as_str(),
                    &serde_json::json!({"attempt": 1, "outcome": "failed",
                        "reason": "runner restarted", "lost": true, "duration_ms": 0,
                        "input_tokens": null, "output_tokens": null, "cost_usd": null})
                ),
            ]
        );
        // The resumed attempt's log goes on after the 3 lines the server has.
        let resumed_log = format!("/api/runs/{RESUMED}/log");
        let first_lines: Vec<i64> = (seen.iter())
            .filter(|(_, p, _)| *p == resumed_log)
            .map(|(_, _, b)| b["first_line"].as_i64().unwrap())
            .collect();
        assert_eq!(first_lines.first(), Some(&4));
        let fresh_log = format!("/api/runs/{FRESH}/log");
        let first = seen.iter().find(|(_, p, _)| *p == fresh_log).unwrap();
        assert_eq!(first.2["first_line"], 1);
        for run in [RESUMED, FRESH] {
            let path = format!("/api/runs/{run}/finish");
            let (_, _, body) = seen.iter().find(|(_, p, _)| *p == path).unwrap();
            assert_eq!(
                (body["outcome"].as_str(), body["lost"].as_bool()),
                (Some("succeeded"), Some(false))
            );
        }

        // Each attempt's `sleep` carried the attempt and was gone when its outcome was sent.
        let detached = std::fs::read_to_string(&detached).unwrap();
        let mut attempts: Vec<&str> = (detached.lines())
            .map(|line| line.split_once(' ').unwrap().1)
            .collect();
        attempts.sort();
        assert_eq!(attempts, [format!("{RESUMED}-1"), format!("{FRESH}-1")]);
        for line in detached.lines() {
            let pid = line.split_once(' ').unwrap().0;
            let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
            let state = stat.rsplit_once(") ").map(|(_, rest)| &rest[..1]);
            assert!(
                matches!(state, None | Some("Z")),
                "sleep {pid} still runs: {stat}"
            );
        }

        // Both agents, resumed and fresh, had the claim's GitHub token and no other.
        assert_eq!(
            std::fs::read_to_string(&github).unwrap(),
            "unset ghs_run app[bot]\nunset ghs_run app[bot]\n"
        );
        let argv = std::fs::read_to_string(&argv).unwrap();
        let mut sessions: Vec<&str> = argv
            .lines()
            .filter(|line| !line.starts_with("--probe"))
            .map(|line| {
                let at = line.find("--session-id").or(line.find("--resume")).unwrap();
                &line[at..]
            })
            .collect();
        sessions.sort();
        assert_eq!(
            sessions,
            [
                format!("--resume {RESUMED_SESSION}"),
                format!("--session-id {FRESH_SESSION}"),
            ]
        );
        let models: Vec<&str> = (argv.lines())
            .filter(|line| line.contains("--model"))
            .collect();
        assert_eq!(models.len(), 1, "{argv}");
        assert!(models[0].contains("--model haiku --mcp-config"), "{argv}");
        assert!(
            models[0].ends_with(&format!("--session-id {FRESH_SESSION}")),
            "{argv}"
        );
    }

    #[test]
    fn retry_delay_doubles_and_caps() {
        let delays: Vec<i64> = (1..=7)
            .map(|n| retry_delay_ms(n, MAX_RETRY_BACKOFF_MS))
            .collect();
        assert_eq!(
            delays,
            [10_000, 20_000, 40_000, 80_000, 160_000, 300_000, 300_000]
        );
        assert_eq!(retry_delay_ms(0, MAX_RETRY_BACKOFF_MS), 10_000);
    }
}
