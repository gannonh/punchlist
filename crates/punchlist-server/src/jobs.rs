//! The job queue (ADR 0007): a Postgres table claimed with `FOR UPDATE SKIP LOCKED` under
//! a lease. A job's writes and its completion commit in one transaction, so a crash runs
//! the job again and idempotency keeps the rerun from doubling its effects.

use std::time::Duration;

use anyhow::Context;

use crate::AppState;
use crate::db;

/// How long a claimed job is leased before another worker may take it.
const LEASE_SECONDS: i32 = 30;
/// A job that fails this many times is marked `failed` and not retried.
const MAX_ATTEMPTS: i32 = 10;
/// How many times one run of a `github_event` job reads GitHub before a dropped read fails it.
const MAX_READS: u32 = 3;
/// How long the worker sleeps when no job wakes it.
const POLL: Duration = Duration::from_secs(1);

/// A job failure a retry cannot fix, such as an import whose states no longer map: the job is
/// marked `failed` at once, with this message.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub(crate) struct Permanent(pub String);

/// Runs jobs for the life of the server: until none is ready, then until a job is enqueued
/// or a second passes (a retry's `run_after` or an expired lease needs no wake-up).
pub async fn work_jobs(state: AppState) {
    loop {
        if let Err(error) = run_jobs_until_idle(&state).await {
            tracing::error!(%error, "job worker");
        }
        let _ = tokio::time::timeout(POLL, state.job_wake.notified()).await;
    }
}

/// Runs ready jobs, oldest first, until none is ready.
pub async fn run_jobs_until_idle(state: &AppState) -> Result<(), sqlx::Error> {
    while run_next_job(state).await? {}
    Ok(())
}

/// Claims and runs the oldest ready job. Returns false when no job was ready. A job that
/// fails is rescheduled; only a database failure while claiming or rescheduling is an error.
pub async fn run_next_job(state: &AppState) -> Result<bool, sqlx::Error> {
    let mut tx = db::begin(&state.pool).await?;
    let Some(job) = sqlx::query!(
        "SELECT id, kind, payload FROM job
         WHERE (state = 'queued' AND run_after <= now())
            OR (state = 'running' AND lease_expires_at < now())
         ORDER BY id LIMIT 1 FOR UPDATE SKIP LOCKED"
    )
    .fetch_optional(&mut *tx)
    .await?
    else {
        return Ok(false);
    };
    let attempt = sqlx::query_scalar!(
        "UPDATE job SET state = 'running', attempts = attempts + 1,
                lease_expires_at = now() + make_interval(secs => $2)
         WHERE id = $1 RETURNING attempts",
        job.id,
        f64::from(LEASE_SECONDS),
    )
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;

    // The work and the completion commit together. The completion only matches while this
    // attempt still holds the job, so a worker whose lease expired rolls back.
    let result = async {
        let mut tx;
        let mut result = None;
        match job.kind.as_str() {
            "github_event" => {
                // GitHub is read before the transaction begins, so no lock or connection is
                // held across the network. A read that is dropped because another was
                // recorded first is made again here, so it does not spend one of the job's
                // attempts. Every read is done within half the lease from the first's start,
                // so a slow GitHub fails the job instead of outliving it.
                let deadline = tokio::time::Instant::now()
                    + Duration::from_secs_f64(f64::from(LEASE_SECONDS) / 2.0);
                let mut reads = 0;
                tx = loop {
                    let read = crate::github::read_current(state, &job.payload);
                    let (event, current) = tokio::time::timeout_at(deadline, read)
                        .await
                        .context("GitHub did not answer in time")??;
                    let mut tx = db::begin(&state.pool).await?;
                    if crate::github::process(&mut tx, &event, current).await? {
                        break tx;
                    }
                    tx.rollback().await?;
                    reads += 1;
                    anyhow::ensure!(
                        reads < MAX_READS,
                        "another read of the pull request was recorded first, {MAX_READS} times"
                    );
                    tracing::info!(
                        job = job.id,
                        "another read of the pull request was recorded first, reading again"
                    );
                };
            }
            // ponytail: the GitHub reads run inside the transaction, holding a connection
            // for their few seconds; fine at one load per push to `.punchlist/`.
            "workflow_load" => {
                tx = db::begin(&state.pool).await?;
                crate::workflow::load(state, &mut tx, job.payload).await?;
            }
            "linear_import" => {
                tx = db::begin(&state.pool).await?;
                result = Some(crate::linear_import::run(&mut tx, job.payload).await?);
            }
            other => anyhow::bail!("unknown job kind `{other}`"),
        }
        let done = sqlx::query!(
            "UPDATE job SET state = 'done', lease_expires_at = NULL, last_error = NULL,
                    finished_at = now(), result = $3
             WHERE id = $1 AND state = 'running' AND attempts = $2",
            job.id,
            attempt,
            result,
        )
        .execute(&mut *tx)
        .await?;
        anyhow::ensure!(done.rows_affected() == 1, "the job's lease was taken over");
        tx.commit().await?;
        anyhow::Ok(())
    }
    .await;

    if let Err(error) = result {
        tracing::warn!(job = job.id, attempt, %error, "job failed");
        let last = if error.is::<Permanent>() {
            attempt
        } else {
            MAX_ATTEMPTS
        };
        sqlx::query!(
            "UPDATE job SET
                state = CASE WHEN attempts >= $3 THEN 'failed' ELSE 'queued' END,
                lease_expires_at = NULL,
                run_after = now() + make_interval(secs => LEAST(power(2, attempts), 300)),
                last_error = $4,
                finished_at = CASE WHEN attempts >= $3 THEN now() END
             WHERE id = $1 AND state = 'running' AND attempts = $2",
            job.id,
            attempt,
            last,
            format!("{error:#}"),
        )
        .execute(&state.pool)
        .await?;
    }
    Ok(true)
}
