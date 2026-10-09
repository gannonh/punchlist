//! The job queue (ADR 0007): a Postgres table claimed with `FOR UPDATE SKIP LOCKED` under
//! a lease. A job's writes and its completion commit in one transaction, so a crash runs
//! the job again and idempotency keeps the rerun from doubling its effects.

use std::time::Duration;

use crate::AppState;

/// How long a claimed job is leased before another worker may take it.
const LEASE_SECONDS: i32 = 30;
/// A job that fails this many times is marked `failed` and not retried.
const MAX_ATTEMPTS: i32 = 10;
/// How long the worker sleeps when no job wakes it.
const POLL: Duration = Duration::from_secs(1);

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
    let mut tx = state.pool.begin().await?;
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
        let mut tx = state.pool.begin().await?;
        match job.kind.as_str() {
            "github_event" => crate::github::process(&mut tx, job.payload).await?,
            other => anyhow::bail!("unknown job kind `{other}`"),
        }
        let done = sqlx::query!(
            "UPDATE job SET state = 'done', lease_expires_at = NULL, last_error = NULL,
                    finished_at = now()
             WHERE id = $1 AND state = 'running' AND attempts = $2",
            job.id,
            attempt,
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
            MAX_ATTEMPTS,
            format!("{error:#}"),
        )
        .execute(&state.pool)
        .await?;
    }
    Ok(true)
}
