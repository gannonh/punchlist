// Copied from kata-symphony apps/symphony/src/orchestrator.rs at e7c160f5; trimmed: only the failure-retry backoff (`FAILURE_RETRY_BASE_MS`, `retry_delay_ms`) is copied. The claim loop, concurrency limit, heartbeat and attempt lifecycle below are written for Punchlist against the server API (the original polls a tracker); tracker polling, supervisor, triage, docker/ssh workers, shared context, notifications and the workflow store are dropped.
//! The claim loop: register, heartbeat, claim up to `max_concurrent` issues and run each
//! attempt, backing off on server errors.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use punchlist_api::{
    AppendLog, Claim, FinishRun, Heartbeat, HeldAttempt, RegisterRunner, RunOutcome,
};
use punchlist_client::{Client, ClientError};
use tokio::sync::{Semaphore, mpsc, watch};
use tokio::task::JoinSet;
use uuid::Uuid;

use crate::RunnerConfig;
use crate::agent::{AgentExit, AgentResult, build_prompt, run_claude};
use crate::worktree::Worktrees;

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
    matches!(error, ClientError::Refused { code, .. } if code == wanted)
}

/// The attempts this runner is working on; the heartbeat renews exactly these leases.
type Held = Arc<Mutex<HashSet<(Uuid, i32)>>>;

/// Removes its attempt from the held set when dropped, however the attempt ends.
struct HeldGuard {
    held: Held,
    key: (Uuid, i32),
}

impl HeldGuard {
    fn new(held: Held, key: (Uuid, i32)) -> HeldGuard {
        held.lock().expect("held set").insert(key);
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
    client: Client,
    worktrees: Worktrees,
    shutdown: watch::Receiver<bool>,
}

pub async fn run(
    config: RunnerConfig,
    shutdown: impl std::future::Future<Output = ()> + Send,
) -> anyhow::Result<()> {
    run_with_worktrees(
        Worktrees::new(config.worktree_root.clone()),
        config,
        shutdown,
    )
    .await
}

pub(crate) async fn run_with_worktrees(
    worktrees: Worktrees,
    config: RunnerConfig,
    shutdown: impl std::future::Future<Output = ()> + Send,
) -> anyhow::Result<()> {
    if let Some(unsupported) = config.agents.iter().find(|a| a.as_str() != "claude-code") {
        anyhow::bail!("unsupported agent `{unsupported}`; only claude-code is supported");
    }
    let registered = Client::new(&config.server_url, &config.token)?
        .register_runner(&RegisterRunner {
            name: config.name.clone(),
            agents: config.agents.clone(),
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
    let held: Held = Arc::default();
    let heartbeat = {
        let client = client.clone();
        let held = held.clone();
        let interval = config.heartbeat_interval;
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                let snapshot = Heartbeat {
                    held: held
                        .lock()
                        .expect("held set")
                        .iter()
                        .map(|&(run_id, attempt)| HeldAttempt { run_id, attempt })
                        .collect(),
                };
                if let Err(e) = client.heartbeat(&runner_id, &snapshot).await {
                    tracing::warn!("heartbeat failed: {e}");
                }
            }
        })
    };

    let max_concurrent = config.max_concurrent.max(1);
    let claim_wait = config.claim_wait.as_secs().min(30) as u32;
    let ctx = Arc::new(Context {
        config,
        client,
        worktrees,
        shutdown: shutdown_rx,
    });
    let permits = Arc::new(Semaphore::new(max_concurrent));
    let mut attempts = JoinSet::new();
    let mut failures: u32 = 0;
    tokio::pin!(shutdown);

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
                let guard = HeldGuard::new(held.clone(), (claim.run_id, claim.attempt));
                attempts.spawn(async move {
                    run_attempt(&ctx, claim).await;
                    drop(guard);
                    drop(permit);
                });
            }
            Ok(None) => failures = 0,
            Err(e) if is_code(&e, "claim_taken") => {
                failures = 0;
                if let ClientError::Refused { message, .. } = &e {
                    tracing::info!("claim taken: {message}");
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
    Ok(())
}

/// Batches log lines: a flush after 500 ms or 50 lines. A batch the server refuses as
/// invalid is dropped; one that keeps failing is retried with backoff, then dropped after
/// `LOG_BATCH_TRIES`. Returns true when the server said the attempt is stale, after
/// signalling `stop`.
async fn flush_logs(
    client: Client,
    run_id: String,
    attempt: i32,
    mut lines: mpsc::Receiver<String>,
    stop: Arc<watch::Sender<bool>>,
) -> bool {
    let mut pending: Vec<String> = Vec::new();
    // The number of the first pending line; the lines of an attempt are numbered 1, 2, 3...
    let mut first_line: i64 = 1;
    let mut stale = false;
    let mut closed = false;
    let mut tries: u32 = 0;
    // After the agent's output closes, keep going until the last batch is sent or dropped.
    while !closed || !pending.is_empty() {
        if pending.is_empty() {
            match lines.recv().await {
                Some(line) => pending.push(clip(line)),
                None => break,
            }
        }
        let deadline = tokio::time::Instant::now() + LOG_FLUSH_INTERVAL;
        while !closed && pending.len() < LOG_BATCH_LINES {
            match tokio::time::timeout_at(deadline, lines.recv()).await {
                Ok(Some(line)) => pending.push(clip(line)),
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
        match client.append_log(&run_id, &request).await {
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
                    tokio::time::sleep(backoff.min(Duration::from_secs(8))).await;
                }
            }
        }
    }
    if stale {
        // Keep draining so the agent's output pipe never blocks on a closed channel.
        while lines.recv().await.is_some() {}
    }
    stale
}

async fn run_attempt(ctx: &Context, claim: Claim) {
    let start = Instant::now();
    let run_id = claim.run_id.to_string();
    let attempt = claim.attempt;
    tracing::info!(
        "claimed {} (run {run_id}, attempt {attempt}) on branch {}",
        claim.issue.id,
        claim.branch
    );

    let (lines_tx, lines_rx) = mpsc::channel(LOG_CHANNEL_LINES);
    let (stop_tx, stop_rx) = watch::channel(false);
    let stop_tx = Arc::new(stop_tx);
    let flusher = tokio::spawn(flush_logs(
        ctx.client.clone(),
        run_id.clone(),
        attempt,
        lines_rx,
        stop_tx.clone(),
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

    // Stopping the runner drops the preparation, which kills the git command it is running.
    let prepared = {
        let mut stop = stop_rx.clone();
        tokio::select! {
            prepared = ctx.worktrees.prepare(&claim.repository, &claim.branch, &claim.issue.id) => Some(prepared),
            _ = async { let _ = stop.wait_for(|stopped| *stopped).await; } => None,
        }
    };
    let exit = match prepared {
        None => AgentExit::Stopped,
        Some(prepared) => match prepared {
            Ok(path) => {
                tracing::info!("worktree ready: {}", path.display());
                let _ = lines_tx
                    .send(format!(
                        "worktree {} on branch {}",
                        path.display(),
                        claim.branch
                    ))
                    .await;
                let prompt = build_prompt(&claim.issue, &claim.repository, &claim.branch);
                run_claude(
                    &ctx.config.claude_command,
                    ctx.config.model.as_deref(),
                    &path,
                    &prompt,
                    lines_tx.clone(),
                    stop_rx,
                )
                .await
            }
            Err(reason) => {
                let _ = lines_tx
                    .send(format!("worktree preparation failed: {reason}"))
                    .await;
                AgentExit::Finished(AgentResult {
                    outcome: RunOutcome::Failed,
                    reason: Some(format!("worktree preparation failed: {reason}")),
                    input_tokens: None,
                    output_tokens: None,
                })
            }
        },
    };
    drop(lines_tx);
    forward.abort();
    let stale = flusher.await.unwrap_or(false);

    let result = match exit {
        AgentExit::Finished(result) => result,
        AgentExit::Stopped if stale => {
            tracing::warn!("run {run_id}: agent stopped, lease lost; not reporting an outcome");
            return;
        }
        AgentExit::Stopped => AgentResult {
            outcome: RunOutcome::Failed,
            reason: Some("runner stopped".into()),
            input_tokens: None,
            output_tokens: None,
        },
    };
    tracing::info!(
        "agent exited for {} (run {run_id}): {}",
        claim.issue.id,
        result.reason.as_deref().unwrap_or("success")
    );
    let request = FinishRun {
        attempt,
        outcome: result.outcome,
        reason: result.reason,
        duration_ms: start.elapsed().as_millis().min(i64::MAX as u128) as i64,
        input_tokens: result.input_tokens,
        output_tokens: result.output_tokens,
    };
    finish_with_retry(&ctx.client, &run_id, &request).await;
}

async fn finish_with_retry(client: &Client, run_id: &str, request: &FinishRun) {
    for failures in 1..=5u32 {
        match client.finish_run(run_id, request).await {
            Ok(_) => {
                tracing::info!("finished run {run_id}: {}", request.outcome.as_str());
                return;
            }
            Err(e) if is_code(&e, "stale_attempt") => {
                tracing::warn!("run {run_id}: lease lost before the outcome was recorded");
                return;
            }
            Err(e) => {
                let delay = retry_delay_ms(failures, MAX_RETRY_BACKOFF_MS) as u64;
                tracing::warn!(
                    "run {run_id}: cannot record the outcome: {e}; retrying in {delay} ms"
                );
                tokio::time::sleep(Duration::from_millis(delay)).await;
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
        assert!(!is_permanent(&ClientError::Refused {
            code: "stale_attempt".into(),
            message: "x".into(),
        }));
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
            rx,
            Arc::new(stop_tx),
        ));
        tx.send("short".into()).await.unwrap();
        tx.send("x".repeat(70 * 1024)).await.unwrap();
        drop(tx);
        let stale = tokio::time::timeout(Duration::from_secs(10), flusher)
            .await
            .expect("the flusher ends")
            .unwrap();
        assert!(!stale);

        let log = runner.run_log(&run_id).await.unwrap();
        let lines: Vec<&str> = log.lines.iter().map(|l| l.line.as_str()).collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "short");
        assert_eq!(
            lines[1],
            format!("{}… [truncated]", "x".repeat(LOG_LINE_MAX_BYTES))
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
