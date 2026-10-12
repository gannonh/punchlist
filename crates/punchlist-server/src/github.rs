//! `POST /api/github/webhook`: GitHub's deliveries, verified, parsed and queued as jobs,
//! and the job that records each one as evidence on pull requests and issues, or queues a
//! workflow load for a push that changes `.punchlist/` on the default branch.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use chrono::{DateTime, Utc};
use hmac::{Hmac, KeyInit, Mac};
use punchlist_api::ErrorBody;
use punchlist_core::{CheckRunEvidence, Refusal, Role, linked_issue_id};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use crate::GithubClient;
use crate::auth::Actor;
use crate::github_api::{ReviewThread, unless_gone};
use crate::issues::{check_gates, record_comment, record_refusal, write_transition};
use crate::workflow::{active_workflow, lock_workspace, queue_workflow_load};
use crate::{ApiError, AppState};

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(webhook))
}

/// `pull_request` actions that change what Punchlist records.
const PULL_REQUEST_ACTIONS: [&str; 7] = [
    "opened",
    "reopened",
    "edited",
    "synchronize",
    "ready_for_review",
    "converted_to_draft",
    "closed",
];
/// `check_run` actions: every one carries the run's current status.
const CHECK_RUN_ACTIONS: [&str; 4] = ["created", "completed", "rerequested", "requested_action"];
const REVIEW_THREAD_ACTIONS: [&str; 2] = ["resolved", "unresolved"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Repository {
    pub name: String,
    pub owner: Account,
    /// In `push` events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_branch: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Account {
    pub login: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Head {
    #[serde(rename = "ref")]
    pub branch: String,
    pub sha: String,
}

/// The `pull_request` object in `pull_request` and `pull_request_review_thread` events.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct PullRequestData {
    pub number: i64,
    pub id: i64,
    pub title: String,
    pub html_url: String,
    pub user: Account,
    /// `open` or `closed`.
    pub state: String,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub merged: bool,
    /// Set once merged. Present in every event that carries a pull request, unlike `merged`,
    /// which the short pull request in a review thread event lacks.
    #[serde(default)]
    pub merged_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub merged_by: Option<Account>,
    #[serde(default)]
    pub closed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub mergeable_state: Option<String>,
    pub head: Head,
    pub updated_at: DateTime<Utc>,
}

impl PullRequestData {
    fn is_merged(&self) -> bool {
        self.merged || self.merged_at.is_some()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct PullRequestEvent {
    pub action: String,
    pub pull_request: PullRequestData,
    pub repository: Repository,
    pub sender: Account,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct CheckRunData {
    pub id: i64,
    pub name: String,
    pub status: String,
    #[serde(default)]
    pub conclusion: Option<String>,
    pub head_sha: String,
    /// GitHub's schema allows none, and a job queued by an older binary has none.
    #[serde(default)]
    pub check_suite: Option<CheckSuite>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct CheckSuite {
    pub id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct CheckRunEvent {
    pub action: String,
    pub check_run: CheckRunData,
    pub repository: Repository,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ThreadData {
    pub node_id: String,
    /// The first comment says where the thread is.
    #[serde(default)]
    pub comments: Vec<ThreadComment>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ThreadComment {
    pub path: String,
    #[serde(default)]
    pub line: Option<i64>,
    #[serde(default)]
    pub original_line: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ReviewThreadEvent {
    pub action: String,
    pub thread: ThreadData,
    pub pull_request: PullRequestData,
    pub repository: Repository,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct PushCommit {
    #[serde(default)]
    pub added: Vec<String>,
    #[serde(default)]
    pub removed: Vec<String>,
    #[serde(default)]
    pub modified: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct PushEvent {
    /// Such as `refs/heads/main`.
    #[serde(rename = "ref")]
    pub git_ref: String,
    #[serde(default)]
    pub deleted: bool,
    #[serde(default)]
    pub forced: bool,
    #[serde(default)]
    pub commits: Vec<PushCommit>,
    pub repository: Repository,
}

impl PushEvent {
    /// A push to the default branch that changes `.punchlist/`. A forced push may drop
    /// commits that did, so it counts too.
    fn changes_workflow(&self) -> bool {
        let Some(branch) = &self.repository.default_branch else {
            return false;
        };
        !self.deleted
            && self.git_ref == format!("refs/heads/{branch}")
            && (self.forced
                || self.commits.iter().any(|c| {
                    [&c.added, &c.removed, &c.modified]
                        .into_iter()
                        .flatten()
                        .any(|path| path.starts_with(".punchlist/"))
                }))
    }
}

/// A delivery that passed validation, as queued.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "event", content = "payload", rename_all = "snake_case")]
pub(crate) enum GithubEvent {
    PullRequest(PullRequestEvent),
    CheckRun(CheckRunEvent),
    PullRequestReviewThread(ReviewThreadEvent),
    Push(PushEvent),
}

impl GithubEvent {
    fn repository(&self) -> &Repository {
        match self {
            GithubEvent::PullRequest(e) => &e.repository,
            GithubEvent::CheckRun(e) => &e.repository,
            GithubEvent::PullRequestReviewThread(e) => &e.repository,
            GithubEvent::Push(e) => &e.repository,
        }
    }
}

/// The payload of a `github_event` job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct GithubJob {
    pub delivery_id: String,
    pub workspace_id: Uuid,
    pub repository_id: Uuid,
    #[serde(flatten)]
    pub event: GithubEvent,
}

#[derive(Deserialize)]
struct ActionOnly {
    action: String,
}

/// Parses a delivery of a handled event type. `Ok(None)` for an event or action that
/// Punchlist does not record; an error for a handled one whose payload is malformed.
fn parse_event(event: &str, body: &[u8]) -> Result<Option<GithubEvent>, ApiError> {
    fn typed<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, ApiError> {
        serde_json::from_slice(body)
            .map_err(|error| ApiError::Invalid(format!("malformed payload: {error}")))
    }
    if event == "push" {
        return Ok(Some(GithubEvent::Push(typed(body)?)));
    }
    let handled: &[&str] = match event {
        "pull_request" => &PULL_REQUEST_ACTIONS,
        "check_run" => &CHECK_RUN_ACTIONS,
        "pull_request_review_thread" => &REVIEW_THREAD_ACTIONS,
        _ => return Ok(None),
    };
    let ActionOnly { action } = typed(body)?;
    if !handled.contains(&action.as_str()) {
        return Ok(None);
    }
    Ok(Some(match event {
        "pull_request" => GithubEvent::PullRequest(typed(body)?),
        "check_run" => GithubEvent::CheckRun(typed(body)?),
        _ => GithubEvent::PullRequestReviewThread(typed(body)?),
    }))
}

/// Checks `X-Hub-Signature-256` (`sha256=<hex>`) against the body, in constant time.
fn verify_signature(secret: &str, headers: &HeaderMap, body: &[u8]) -> Result<(), ApiError> {
    let signature = headers
        .get("x-hub-signature-256")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("sha256="))
        .and_then(decode_hex)
        .ok_or(ApiError::Unauthorized)?;
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).map_err(|_| ApiError::Unauthorized)?;
    mac.update(body);
    mac.verify_slice(&signature)
        .map_err(|_| ApiError::Unauthorized)
}

fn decode_hex(text: &str) -> Option<Vec<u8>> {
    if !text.is_ascii() || !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
        .collect()
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Result<&'a str, ApiError> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::Invalid(format!("missing {name} header")))
}

/// Receive a GitHub webhook delivery. The signature is checked, the payload parsed, and a
/// job queued to record it; the answer does not wait for the job. Unauthenticated: GitHub
/// signs the body instead of sending a bearer token.
#[utoipa::path(
    post,
    path = "/api/github/webhook",
    request_body(content = String, content_type = "application/json"),
    responses(
        (status = 202, description = "Accepted: queued, or ignored because Punchlist does not record this event."),
        (status = 401, body = ErrorBody, description = "The signature is missing or wrong."),
        (status = 422, body = ErrorBody, description = "A required header is missing or the payload is malformed."),
        (status = 503, body = ErrorBody, description = "No webhook secret is configured."),
    ),
    security(())
)]
async fn webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, ApiError> {
    let secret = state
        .github_webhook_secret
        .as_deref()
        .filter(|secret| !secret.is_empty())
        .ok_or(ApiError::GithubNotConfigured)?;
    verify_signature(secret, &headers, &body)?;
    let event_name = header(&headers, "x-github-event")?;
    let delivery_id = header(&headers, "x-github-delivery")?;
    let Some(event) = parse_event(event_name, &body)? else {
        return Ok(StatusCode::ACCEPTED);
    };

    let repository = event.repository();
    let Some(repo) = sqlx::query!(
        "SELECT id, workspace_id FROM repository
         WHERE lower(owner) = lower($1) AND lower(name) = lower($2)",
        repository.owner.login,
        repository.name,
    )
    .fetch_optional(&state.pool)
    .await?
    else {
        tracing::info!(
            repository = %format!("{}/{}", repository.owner.login, repository.name),
            delivery_id,
            "webhook for a repository no workspace uses"
        );
        return Ok(StatusCode::ACCEPTED);
    };
    let job = GithubJob {
        delivery_id: delivery_id.to_string(),
        workspace_id: repo.workspace_id,
        repository_id: repo.id,
        event,
    };
    let payload = serde_json::to_value(&job)
        .map_err(|error| ApiError::Invalid(format!("unserializable payload: {error}")))?;
    sqlx::query!(
        "INSERT INTO job (kind, idempotency_key, payload) VALUES ('github_event', $1, $2)
         ON CONFLICT (idempotency_key) DO UPDATE SET state = 'queued', attempts = 0,
             run_after = now(), last_error = NULL, finished_at = NULL
             -- Only a job that gave up: GitHub's redelivery reuses the delivery id, and is
             -- the retry. A queued, running or done job already has the delivery.
             WHERE job.state = 'failed'",
        delivery_id,
        payload,
    )
    .execute(&state.pool)
    .await?;
    state.job_wake.notify_one();
    Ok(StatusCode::ACCEPTED)
}

/// What GitHub says now, where a payload cannot be ordered against the others. `None` when
/// it was not read, or GitHub says the pull request is gone: the payload is then recorded.
#[derive(Default)]
pub(crate) struct Current {
    /// For a payload that would close the pull request: `updated_at` is in whole seconds.
    pull_request: Option<PullRequestData>,
    /// For a review thread event: it carries no time, and none is sent for a new thread.
    threads: Option<Vec<ReviewThread>>,
    /// The pull request row's version before either read, for `claim_read`: 0 with no row.
    /// `None` when GitHub gave no answer, so the payload is recorded as one without a read.
    version: Option<i64>,
}

/// Parses a `github_event` job and reads `Current` for it, before the job's transaction. A
/// failed read fails the job; without a GitHub App nothing is read.
pub(crate) async fn read_current(
    state: &AppState,
    payload: &serde_json::Value,
) -> anyhow::Result<(GithubJob, Current)> {
    let job = GithubJob::deserialize(payload)?;
    let mut current = Current::default();
    let (pr, thread_event) = match &job.event {
        GithubEvent::PullRequest(event) => (&event.pull_request, false),
        GithubEvent::PullRequestReviewThread(event) => (&event.pull_request, true),
        GithubEvent::CheckRun(_) | GithubEvent::Push(_) => return Ok((job, current)),
    };
    let Some(github) = &state.github else {
        return Ok((job, current));
    };
    let repository = job.event.repository();
    let (owner, name) = (&repository.owner.login, &repository.name);
    let recorded = sqlx::query!(
        "SELECT state, version FROM pull_request WHERE repository_id = $1 AND number = $2",
        job.repository_id,
        pr.number,
    )
    .fetch_optional(&state.pool)
    .await?;
    let version = recorded.as_ref().map_or(0, |row| row.version);
    // The payload closes it only if it is recorded open, or not recorded; a thread event
    // records the pull request only in the second case.
    let closes = pr.state != "open"
        && !pr.is_merged()
        && recorded.is_none_or(|row| row.state == "open" && !thread_event);
    if closes {
        current.pull_request = unless_gone(github.pull_request(owner, name, pr.number).await)?;
    }
    if thread_event {
        current.threads = unless_gone(github.review_threads(owner, name, pr.number).await)?;
    }
    if current.pull_request.is_some() || current.threads.is_some() {
        current.version = Some(version);
    }
    Ok((job, current))
}

/// Records one delivery. Runs inside the job's transaction, and is safe to run again. False
/// when what `current` read is dropped (`claim_read`) and nothing was written: the caller
/// rolls back and reads again.
pub(crate) async fn process(
    tx: &mut sqlx::PgConnection,
    job: &GithubJob,
    current: Current,
) -> anyhow::Result<bool> {
    match &job.event {
        GithubEvent::PullRequest(event) => {
            // Only a `closed` event's sender closed it; another event may still be the first
            // to carry the closed state. If GitHub says it has been merged since, whoever
            // merged it did; if it was closed again since, GitHub does not say who did.
            let closed_by = match &current.pull_request {
                Some(now) if now.is_merged() => now.merged_by.as_ref(),
                Some(now) if now.closed_at != event.pull_request.closed_at => None,
                _ => (event.action == "closed").then_some(&event.sender),
            };
            let pr = current.pull_request.as_ref().unwrap_or(&event.pull_request);
            if let Some(version) = current.version
                && !claim_read(tx, job.workspace_id, job.repository_id, pr, version).await?
            {
                return Ok(false);
            }
            let id = record_pull_request(
                tx,
                job.workspace_id,
                job.repository_id,
                pr,
                closed_by.map(|account| account.login.as_str()),
                Some(&job.delivery_id),
            )
            .await?;
            if current.version.is_some() {
                record_read(tx, id).await?;
            }
        }
        GithubEvent::CheckRun(event) => {
            record_check_run(tx, job.repository_id, &event.check_run, None).await?;
            recompute_checks(tx, job.repository_id, &event.check_run.head_sha).await?;
        }
        GithubEvent::Push(event) => {
            if event.changes_workflow() {
                let key = format!("workflow_load:{}", job.delivery_id);
                queue_workflow_load(tx, job.workspace_id, &key).await?;
            }
        }
        GithubEvent::PullRequestReviewThread(event) => {
            // This event carries GitHub's short pull request, without `merged` or
            // `mergeable_state`, so it records the pull request only if no `pull_request`
            // event has yet; otherwise it would turn a merged one back into closed. `merged_at`
            // still tells it whether the pull request was merged.
            // Locked before the thread rows are written, as `record_pull_request` locks it:
            // the gated request's refresh holds this lock while it writes the same rows.
            let pr = current.pull_request.as_ref().unwrap_or(&event.pull_request);
            if let Some(version) = current.version
                && !claim_read(tx, job.workspace_id, job.repository_id, pr, version).await?
            {
                return Ok(false);
            }
            let known = sqlx::query_scalar!(
                "SELECT id FROM pull_request WHERE repository_id = $1 AND number = $2 FOR UPDATE",
                job.repository_id,
                event.pull_request.number,
            )
            .fetch_optional(&mut *tx)
            .await?;
            let pull_request_id = match known {
                Some(id) => id,
                None => {
                    record_pull_request(
                        tx,
                        job.workspace_id,
                        job.repository_id,
                        pr,
                        None,
                        Some(&job.delivery_id),
                    )
                    .await?
                }
            };
            if current.version.is_some() {
                record_read(tx, pull_request_id).await?;
            }
            match &current.threads {
                Some(threads) => {
                    forget_other_review_threads(tx, pull_request_id, threads).await?;
                    record_review_threads(tx, pull_request_id, threads).await?;
                }
                // Without GitHub's list the event is all there is: its one thread, as the
                // event left it, in the order it arrived.
                None => {
                    let place = event.thread.comments.first();
                    let thread = ReviewThread {
                        id: event.thread.node_id.clone(),
                        is_resolved: event.action == "resolved",
                        path: place.map(|comment| comment.path.clone()),
                        line: place.and_then(|comment| comment.line),
                        original_line: place.and_then(|comment| comment.original_line),
                    };
                    record_review_threads(tx, pull_request_id, &[thread]).await?;
                }
            }
        }
    }
    Ok(true)
}

/// Records one check run. A delivery only moves a run forward (queued, in_progress,
/// completed), so one arriving late does not undo a newer one. A read is GitHub's list as of
/// `read_started`, the database's clock before GitHub was asked: it overwrites the status and
/// conclusion of any row last written at or before then (so a recorded `completed` can give
/// way to GitHub's `in_progress` for a restarted run), and leaves a row a delivery wrote since,
/// which is newer than the list. Every write sets `updated_at` from `clock_timestamp()`, the
/// moment of the write, not of its transaction's start. A rerun gets a new id, in the same
/// suite. A suite once known is kept.
// ponytail: one round trip per run, up to the page cap (1000); one `UNNEST` insert does it in
// one when a commit has that many.
async fn record_check_run(
    tx: &mut sqlx::PgConnection,
    repository_id: Uuid,
    run: &CheckRunData,
    read_started: Option<DateTime<Utc>>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "INSERT INTO check_run (id, repository_id, head_sha, name, status, conclusion,
                                check_suite_id, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, clock_timestamp())
         ON CONFLICT (id) DO UPDATE SET head_sha = EXCLUDED.head_sha,
             name = EXCLUDED.name, status = EXCLUDED.status,
             conclusion = EXCLUDED.conclusion,
             check_suite_id = COALESCE(EXCLUDED.check_suite_id, check_run.check_suite_id),
             updated_at = clock_timestamp()
         WHERE CASE WHEN $8::timestamptz IS NOT NULL THEN check_run.updated_at <= $8
               ELSE (CASE check_run.status WHEN 'completed' THEN 2 WHEN 'in_progress' THEN 1
                      ELSE 0 END)
                  <= (CASE EXCLUDED.status WHEN 'completed' THEN 2 WHEN 'in_progress' THEN 1
                      ELSE 0 END) END",
        run.id,
        repository_id,
        run.head_sha,
        run.name,
        run.status,
        run.conclusion,
        run.check_suite.as_ref().map(|suite| suite.id),
        read_started,
    )
    .execute(&mut *tx)
    .await?;
    Ok(())
}

/// Records the check runs GitHub lists on `head_sha`, in their own short transaction, before
/// the pull request's row is locked: a `check_run` delivery's job takes the run's row and then
/// the pull request's, and this takes them in the same order. Runs are keyed by id and need no
/// version ordering, so they are kept even if the pull request's write is then dropped.
async fn record_read_check_runs(
    pool: &sqlx::PgPool,
    repository_id: Uuid,
    head_sha: &str,
    mut runs: Vec<CheckRunData>,
    read_started: DateTime<Utc>,
) -> sqlx::Result<()> {
    // The head asked for is the head judged: a run listed for another is not this commit's.
    runs.retain(|run| run.head_sha == head_sha);
    runs.sort_by_key(|run| run.id);
    let mut tx = pool.begin().await?;
    for run in &runs {
        record_check_run(&mut tx, repository_id, run, Some(read_started)).await?;
    }
    recompute_checks(&mut tx, repository_id, head_sha).await?;
    tx.commit().await
}

/// Locks a pull request's row for a write of what GitHub answered a reader that noted the
/// row at `version`, 0 for no row. False if a read has been recorded since: this one may be
/// the older, so the caller writes nothing and reads again. A read that confirms what is
/// recorded claims too: it is still newer than an older read in flight. A payload recorded
/// with no answer from GitHub does not claim, and `github_updated_at` orders it, as before.
///
/// The workspace's lock comes first, as the lock order has it, when `pr` may close the pull
/// request, and when there is no row to lock, so that two first reads do not both insert it.
// ponytail: that time is in whole seconds, so a read and a payload recorded without one, both
// from the same second, stand in the order they were written, as two such payloads do. Read
// GitHub for every pull request payload if that bites.
async fn claim_read(
    tx: &mut sqlx::PgConnection,
    workspace_id: Uuid,
    repository_id: Uuid,
    pr: &PullRequestData,
    version: i64,
) -> sqlx::Result<bool> {
    let mut workspace_locked = pr.state != "open";
    if workspace_locked {
        lock_workspace(tx, workspace_id).await?;
    }
    loop {
        let now = sqlx::query_scalar!(
            "SELECT version FROM pull_request WHERE repository_id = $1 AND number = $2
             FOR UPDATE",
            repository_id,
            pr.number,
        )
        .fetch_optional(&mut *tx)
        .await?;
        if now.is_some() || workspace_locked {
            return Ok(now.unwrap_or(0) == version);
        }
        lock_workspace(tx, workspace_id).await?;
        workspace_locked = true;
    }
}

/// Counts the read `claim_read` claimed, once its pull request is recorded: the row's lock is
/// still held, and a row there was none of now exists.
async fn record_read(tx: &mut sqlx::PgConnection, pull_request_id: Uuid) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE pull_request SET version = version + 1 WHERE id = $1",
        pull_request_id,
    )
    .execute(&mut *tx)
    .await?;
    Ok(())
}

/// Forgets the recorded review threads of a pull request that are not among `threads`, which
/// are all GitHub has for it. `record_review_threads` follows, and recounts.
async fn forget_other_review_threads(
    tx: &mut sqlx::PgConnection,
    pull_request_id: Uuid,
    threads: &[ReviewThread],
) -> sqlx::Result<()> {
    let ids: Vec<String> = threads.iter().map(|thread| thread.id.clone()).collect();
    sqlx::query!(
        "DELETE FROM review_thread WHERE pull_request_id = $1 AND node_id <> ALL($2)",
        pull_request_id,
        &ids,
    )
    .execute(&mut *tx)
    .await?;
    Ok(())
}

/// Records review threads on a pull request and recounts its open ones. The caller holds the
/// pull request's row lock: the lock order is workspace, pull request, issue, then threads.
async fn record_review_threads(
    tx: &mut sqlx::PgConnection,
    pull_request_id: Uuid,
    threads: &[ReviewThread],
) -> sqlx::Result<()> {
    let (ids, resolved): (Vec<String>, Vec<bool>) = threads
        .iter()
        .map(|thread| (thread.id.clone(), thread.is_resolved))
        .unzip();
    let paths: Vec<Option<String>> = threads.iter().map(|t| t.path.clone()).collect();
    let lines: Vec<Option<i64>> = threads.iter().map(|t| t.line.or(t.original_line)).collect();
    sqlx::query!(
        "INSERT INTO review_thread (node_id, pull_request_id, resolved, path, line)
         SELECT node_id, $1, resolved, path, line
         FROM UNNEST($2::text[], $3::boolean[], $4::text[], $5::bigint[])
             AS t (node_id, resolved, path, line)
         ON CONFLICT (node_id) DO UPDATE SET pull_request_id = EXCLUDED.pull_request_id,
             resolved = EXCLUDED.resolved, updated_at = now(),
             -- An event without its comments does not forget where the thread is.
             path = coalesce(EXCLUDED.path, review_thread.path),
             line = coalesce(EXCLUDED.line, review_thread.line)",
        pull_request_id,
        &ids,
        &resolved,
        &paths as &[Option<String>],
        &lines as &[Option<i64>],
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        "UPDATE pull_request SET open_threads =
            (SELECT count(*) FROM review_thread
             WHERE pull_request_id = $1 AND NOT resolved)::int
         WHERE id = $1",
        pull_request_id,
    )
    .execute(&mut *tx)
    .await?;
    Ok(())
}

/// Records a pull request from a payload, unless a newer payload was already applied, and
/// returns its row id. `closed_by` is the GitHub login that closed it, when the payload says;
/// `delivery_id` is the webhook delivery, when one brought it.
///
/// A payload closes a pull request when it records it closed or merged and the row was open,
/// or absent, before. That happens once per close, whichever of a webhook delivery and a read
/// from GitHub comes first and however often either repeats, so `close_issue_on_pull_request`
/// runs on it, here, for every caller.
async fn record_pull_request(
    tx: &mut sqlx::PgConnection,
    workspace_id: Uuid,
    repository_id: Uuid,
    pr: &PullRequestData,
    closed_by: Option<&str>,
    delivery_id: Option<&str>,
) -> Result<Uuid, ApiError> {
    // A close can move the issue. Before the pull request row's foreign key touches the
    // issue row.
    if pr.state != "open" {
        lock_workspace(tx, workspace_id).await?;
    }
    let prefix = sqlx::query_scalar!(
        "SELECT issue_prefix FROM workspace WHERE id = $1",
        workspace_id
    )
    .fetch_one(&mut *tx)
    .await?;
    // A name that matches no issue in the workspace leaves the pull request unlinked.
    let linked = match linked_issue_id(&prefix, &pr.head.branch, &pr.title) {
        Some(id) => {
            sqlx::query_scalar!(
                "SELECT id FROM issue WHERE id = $1 AND workspace_id = $2",
                id,
                workspace_id
            )
            .fetch_optional(&mut *tx)
            .await?
        }
        None => None,
    };
    let state = if pr.is_merged() {
        "merged"
    } else if pr.state == "closed" {
        "closed"
    } else {
        "open"
    };
    let merge_state = pr.mergeable_state.as_deref().unwrap_or("unknown");
    // Locks the row: a second payload closing it waits here, then reads the closed state.
    // Two payloads that are both the first to record it closed wait on the workspace lock
    // above instead, so the second still reads the first's row.
    let was = sqlx::query_scalar!(
        "SELECT state FROM pull_request WHERE repository_id = $1 AND number = $2 FOR UPDATE",
        repository_id,
        pr.number
    )
    .fetch_optional(&mut *tx)
    .await?;
    let applied = sqlx::query_scalar!(
        "INSERT INTO pull_request
             (repository_id, number, github_id, title, branch, head_sha, url, author_login,
              state, draft, merge_state, issue_id, github_updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
         ON CONFLICT (repository_id, number) DO UPDATE SET
             github_id = EXCLUDED.github_id, title = EXCLUDED.title, branch = EXCLUDED.branch,
             head_sha = EXCLUDED.head_sha, url = EXCLUDED.url,
             author_login = EXCLUDED.author_login, state = EXCLUDED.state,
             draft = EXCLUDED.draft, merge_state = EXCLUDED.merge_state,
             issue_id = EXCLUDED.issue_id, github_updated_at = EXCLUDED.github_updated_at,
             updated_at = now()
         WHERE pull_request.github_updated_at <= EXCLUDED.github_updated_at
           AND pull_request.state <> 'merged'
         RETURNING id",
        repository_id,
        pr.number,
        pr.id,
        pr.title,
        pr.head.branch,
        pr.head.sha,
        pr.html_url,
        pr.user.login,
        state,
        pr.draft,
        merge_state,
        linked,
        pr.updated_at,
    )
    .fetch_optional(&mut *tx)
    .await?;
    let Some(id) = applied else {
        // A payload older than the stored state changes nothing, and nothing changes a merged
        // pull request: it cannot reopen. GitHub's `updated_at` is in whole seconds, so two
        // actions in one second still apply in delivery order; `read_current` covers the one
        // that would close it.
        let id = sqlx::query_scalar!(
            "SELECT id FROM pull_request WHERE repository_id = $1 AND number = $2",
            repository_id,
            pr.number
        )
        .fetch_one(&mut *tx)
        .await?;
        return Ok(id);
    };
    recompute_checks(tx, repository_id, &pr.head.sha).await?;
    let closed = state != "open" && was.is_none_or(|was| was == "open");
    if let (true, Some(issue_id)) = (closed, linked) {
        close_issue_on_pull_request(tx, workspace_id, pr, closed_by, delivery_id, &issue_id)
            .await?;
    }
    Ok(id)
}

/// Brings the pull requests an issue's gates read, and their review threads and check runs, up
/// to date with GitHub. A webhook reaches the server seconds after GitHub, and an agent asks for its
/// transition right after it opens or readies a pull request; a gate must not read the state
/// from before. So a read that fails refuses the request (`ApiError::GithubUnreadable`)
/// rather than let the gates pass on rows from before a push: the list, any pull request it
/// reads, and an open pull request's review threads and the check runs on its head commit,
/// whichever gates the transition has (the workflow that decides them is read later, under the
/// issue's lock). A closed pull request's threads and check runs are not read when they could
/// not be: no gate reads them. A pull request GitHub
/// answers 404 or 410 for is gone, and its recorded row stands.
///
/// A recorded pull request that GitHub now reports closed or merged is recorded so, and
/// moves the issue as its webhook would have: recording a merge stops that webhook from
/// applying. No delivery caused such a move, so its transition carries none.
///
/// Each pull request is recorded in its own transaction, as its webhook's job would, and
/// before the caller locks the issue: a job holds the pull request's row and waits for the
/// issue's, so taking the issue's lock first deadlocks.
///
/// A failure to record one fails the request too. What GitHub said is then known and could
/// not be stored, so letting the gates read the older rows could pass a request on a pull
/// request known to be merged or closed. The requester asks again; the pull requests already
/// recorded stay recorded.
///
/// A pull request whose write is dropped (`claim_read`) is read once more, alone. If that
/// write is dropped too the request goes on, and the gates read what is recorded.
// ponytail: the read recorded then began after this request did, but it may be half of one: a
// thread event's job reads only the threads and a close's only the pull request, and both
// raise the one version. So `draft` or `head_sha`, or the threads, can be from before the
// request. A version for each thing read, or reading both every time, closes that. The check
// runs are not under the version at all: they are keyed by run id, recorded before the pull
// request is locked, and kept when its write is dropped.
pub(crate) async fn refresh_pull_requests(
    github: &GithubClient,
    pool: &sqlx::PgPool,
    workspace_id: Uuid,
    issue_id: &str,
) -> Result<(), ApiError> {
    let dropped = refresh_once(github, pool, workspace_id, issue_id, None).await?;
    if !dropped.is_empty() {
        refresh_once(github, pool, workspace_id, issue_id, Some(&dropped)).await?;
    }
    Ok(())
}

/// One pass of `refresh_pull_requests`, over the pull requests numbered in `only` when it is
/// given. Returns the numbers of those whose write was dropped.
// ponytail: GitHub's pull request names who merged it but not who closed it, so a close read
// here credits nobody; read the issue's `closed_by` if the name matters.
async fn refresh_once(
    github: &GithubClient,
    pool: &sqlx::PgPool,
    workspace_id: Uuid,
    issue_id: &str,
    only: Option<&[i64]>,
) -> Result<Vec<i64>, ApiError> {
    let Some(repository) = sqlx::query!(
        "SELECT r.id, r.owner, r.name, w.issue_prefix FROM repository r
         JOIN workspace w ON w.id = r.workspace_id WHERE r.workspace_id = $1",
        workspace_id,
    )
    .fetch_optional(pool)
    .await?
    else {
        return Ok(Vec::new());
    };
    // Each version is noted here, before GitHub is read, for `claim_read`.
    // ponytail: every pull request of the repository, since GitHub's list can name one that
    // is recorded for another issue. Note only the 30 most recently updated and the issue's
    // own if a repository has enough pull requests for this to cost.
    let recorded = sqlx::query!(
        "SELECT number, version, issue_id FROM pull_request
         WHERE repository_id = $1 AND ($2::bigint[] IS NULL OR number = ANY($2))",
        repository.id,
        only,
    )
    .fetch_all(pool)
    .await?;
    let known: Vec<i64> = recorded
        .iter()
        .filter(|row| row.issue_id.as_deref() == Some(issue_id))
        .map(|row| row.number)
        .collect();
    // Of the pull requests not recorded for the issue, only an open one is read: a closed
    // one is history (below), and reading it would be one more read that can fail.
    let names_issue = |pr: &PullRequestData| {
        pr.state == "open"
            && linked_issue_id(&repository.issue_prefix, &pr.head.branch, &pr.title).as_deref()
                == Some(issue_id)
    };
    let (owner, name) = (&repository.owner, &repository.name);
    let read = async {
        let Some(numbers) = only else {
            return github.pull_requests(owner, name, &known, names_issue).await;
        };
        let mut fresh = Vec::new();
        for &number in numbers {
            fresh.extend(unless_gone(github.pull_request(owner, name, number).await)?);
        }
        Ok(fresh)
    };
    let unreadable = |what: &str, error: anyhow::Error| {
        tracing::warn!(issue_id, "cannot read {what} from GitHub: {error:#}");
        ApiError::GithubUnreadable(issue_id.to_string())
    };
    let fresh = read
        .await
        .map_err(|error| unreadable("pull requests", error))?;
    let mut dropped = Vec::new();
    for pr in &fresh {
        // A closed or merged pull request that was never recorded for this issue is history,
        // such as one closed while the server was down. It is not what the agent is working
        // on now, so it is left to its webhook and moves nothing here.
        if pr.state != "open" && !known.contains(&pr.number) {
            continue;
        }
        // An open pull request's threads are what `no_open_threads` reads, so they have to
        // be GitHub's. A closed one's stand as recorded when they cannot be read.
        let threads = match github.review_threads(owner, name, pr.number).await {
            Ok(threads) => Some(threads),
            Err(error) if pr.state == "open" => {
                return Err(unreadable("review threads", error));
            }
            Err(error) => {
                tracing::warn!(
                    issue_id,
                    number = pr.number,
                    "cannot read review threads from GitHub, the recorded ones stand: {error:#}"
                );
                None
            }
        };
        // The check runs `ci_green` reads are an open pull request's alone, and have to be
        // GitHub's too.
        if pr.state == "open" {
            let read_started = sqlx::query_scalar!(r#"SELECT clock_timestamp() AS "at!""#)
                .fetch_one(pool)
                .await?;
            let runs = (github.check_runs(owner, name, &pr.head.sha).await)
                .map_err(|error| unreadable("check runs", error))?;
            record_read_check_runs(pool, repository.id, &pr.head.sha, runs, read_started).await?;
        }
        let merged_by = pr.merged_by.as_ref().map(|account| account.login.as_str());
        let version = recorded
            .iter()
            .find(|row| row.number == pr.number)
            .map_or(0, |row| row.version);
        let mut tx = pool.begin().await?;
        if !claim_read(&mut tx, workspace_id, repository.id, pr, version).await? {
            tracing::info!(
                issue_id,
                number = pr.number,
                "another read of the pull request was recorded first, this one is dropped"
            );
            tx.rollback().await?;
            dropped.push(pr.number);
            continue;
        }
        let id =
            record_pull_request(&mut tx, workspace_id, repository.id, pr, merged_by, None).await?;
        // A push recorded since GitHub answered keeps its head: the runs recorded are for
        // another commit, so this is read again.
        if pr.state == "open"
            && sqlx::query_scalar!("SELECT head_sha FROM pull_request WHERE id = $1", id)
                .fetch_one(&mut *tx)
                .await?
                != pr.head.sha
        {
            tx.rollback().await?;
            dropped.push(pr.number);
            continue;
        }
        record_read(&mut tx, id).await?;
        if let Some(threads) = &threads {
            forget_other_review_threads(&mut tx, id, threads).await?;
            record_review_threads(&mut tx, id, threads).await?;
        }
        tx.commit().await?;
    }
    Ok(dropped)
}

/// The check runs on a commit that count: a rerun is a new run with the same name in the same
/// suite, so the newest run of each name in each suite (two workflows may name a job alike).
/// A run with no suite (recorded before `check_suite_id`, or delivered without one) counts
/// only while no newer run of its name is on the commit: a rerun with a suite supersedes it.
pub(crate) async fn newest_check_runs(
    tx: &mut sqlx::PgConnection,
    repository_id: Uuid,
    head_sha: &str,
) -> sqlx::Result<Vec<CheckRunEvidence>> {
    sqlx::query_as!(
        CheckRunEvidence,
        "SELECT DISTINCT ON (c.check_suite_id, c.name) c.name, c.status, c.conclusion
         FROM check_run c WHERE c.repository_id = $1 AND c.head_sha = $2
           AND NOT (c.check_suite_id IS NULL AND EXISTS (
               SELECT 1 FROM check_run n WHERE n.repository_id = c.repository_id
                 AND n.head_sha = c.head_sha AND n.name = c.name AND n.id > c.id))
         ORDER BY c.check_suite_id, c.name, c.id DESC",
        repository_id,
        head_sha,
    )
    .fetch_all(&mut *tx)
    .await
}

/// Summarizes the check runs on a commit onto every pull request whose head it is.
async fn recompute_checks(
    tx: &mut sqlx::PgConnection,
    repository_id: Uuid,
    head_sha: &str,
) -> sqlx::Result<()> {
    let runs = newest_check_runs(tx, repository_id, head_sha).await?;
    let count = |state: Option<bool>| runs.iter().filter(|run| run.passed() == state).count();
    let (passed, failed, pending) = (count(Some(true)), count(Some(false)), count(None));
    let checks = match (runs.len(), failed, pending) {
        (0, _, _) => "none",
        (_, 1.., _) => "failing",
        (_, _, 1..) => "pending",
        _ => "passing",
    };
    sqlx::query!(
        "UPDATE pull_request SET checks = $3, checks_total = $4, checks_passed = $5
         WHERE repository_id = $1 AND head_sha = $2",
        repository_id,
        head_sha,
        checks,
        runs.len() as i32,
        passed as i32,
    )
    .execute(&mut *tx)
    .await?;
    Ok(())
}

/// A closed pull request moves its linked issue if the workflow has a transition on that
/// event from the issue's status and that transition's gates pass. With no such transition
/// the pull request's new state is all that is recorded. With a failed gate the issue stays
/// where it is and the refusal goes on its timeline: no status, transition or comment.
/// Runs once per close: see `record_pull_request`. `closed_by` is the GitHub login that
/// closed it, when known.
async fn close_issue_on_pull_request(
    tx: &mut sqlx::PgConnection,
    workspace_id: Uuid,
    pr: &PullRequestData,
    closed_by: Option<&str>,
    delivery_id: Option<&str>,
    issue_id: &str,
) -> Result<(), ApiError> {
    let status = sqlx::query_scalar!(
        "SELECT status FROM issue WHERE id = $1 FOR UPDATE",
        issue_id
    )
    .fetch_one(&mut *tx)
    .await?;
    let name = if pr.is_merged() {
        "pr_merged"
    } else {
        "pr_closed_unmerged"
    };
    let (workflow, _) = active_workflow(tx, workspace_id).await?;
    let Some(transition) = workflow.transition_on(&status, name) else {
        tracing::info!(
            issue_id,
            status,
            event = name,
            "no transition on this event"
        );
        return Ok(());
    };
    // With nobody to credit the actor is GitHub itself. No GitHub login is empty, so that
    // actor is never a user's.
    let (name, login) = closed_by.map_or(("GitHub", ""), |login| (login, login));
    let actor_id = sqlx::query_scalar!(
        "INSERT INTO actor (workspace_id, name, role, github_login)
         VALUES ($1, $2, 'github', $3)
         ON CONFLICT (workspace_id, github_login) WHERE github_login IS NOT NULL
         DO UPDATE SET name = EXCLUDED.name
         RETURNING id",
        workspace_id,
        name,
        login,
    )
    .fetch_one(&mut *tx)
    .await?;
    let actor = Actor {
        id: actor_id,
        workspace_id,
        name: name.to_string(),
        role: Role::Github,
    };
    let gates = match check_gates(tx, transition, name, issue_id, &status, Some(pr.number)).await {
        Ok(gates) => gates,
        // `check_gates` logged it too. The event is not tried again, so the timeline is
        // where a person learns why the issue did not move.
        Err(ApiError::GateFailed(failure)) => {
            if let Refusal::GateFailed {
                to, gate, reason, ..
            } = &failure.refusal
            {
                record_refusal(
                    tx,
                    &actor,
                    issue_id,
                    &status,
                    to,
                    gate,
                    reason,
                    delivery_id,
                    workflow.version(),
                )
                .await?;
            }
            return Ok(());
        }
        Err(error) => return Err(error),
    };
    let (_, transition_id, _) = write_transition(
        tx,
        &actor,
        issue_id,
        status,
        transition.to.as_str(),
        delivery_id,
        &workflow,
        &gates,
    )
    .await?;
    // The agent working this issue has nothing left to do: its pull request is gone, and the
    // issue is no longer in the status that started the run. Ending the run frees the issue
    // to be claimed again.
    let how = if pr.is_merged() {
        "merged"
    } else {
        "closed without merging"
    };
    let reason = format!(
        "the issue moved to {} because pull request #{} was {how}",
        transition.to, pr.number
    );
    sqlx::query!(
        "UPDATE attempt SET state = 'failed', finished_at = now(), failure_reason = $2
         WHERE issue_id = $1 AND state = 'running'",
        issue_id,
        reason,
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        "UPDATE run SET outcome = 'failed', finished_at = now()
         WHERE issue_id = $1 AND outcome = 'running'",
        issue_id,
    )
    .execute(&mut *tx)
    .await?;
    if transition.require.iter().any(|r| r == "comment") {
        let by = closed_by.map_or_else(String::new, |login| format!(" by @{login}"));
        let body = format!(
            "Pull request #{} was {how}{by}.\n\n{}",
            pr.number, pr.html_url
        );
        record_comment(tx, &actor, issue_id, &body, Some(transition_id)).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(event: &str, text: &str) -> GithubEvent {
        parse_event(event, text.as_bytes())
            .unwrap_or_else(|e| panic!("{event} fixture: {e}"))
            .unwrap_or_else(|| panic!("{event} fixture is not handled"))
    }

    #[test]
    fn pull_request_opened_parses() {
        let GithubEvent::PullRequest(e) = fixture(
            "pull_request",
            include_str!("../tests/fixtures/github/pull_request_opened.json"),
        ) else {
            panic!("not a pull_request event");
        };
        assert_eq!(e.action, "opened");
        assert_eq!(e.pull_request.number, 12);
        assert_eq!(e.pull_request.head.branch, "feature/pl-1-fix-the-thing");
        assert!(e.pull_request.draft);
        assert!(!e.pull_request.merged);
        assert_eq!(e.repository.owner.login, "gannonh");
    }

    #[test]
    fn pull_request_ready_for_review_parses() {
        let GithubEvent::PullRequest(e) = fixture(
            "pull_request",
            include_str!("../tests/fixtures/github/pull_request_ready_for_review.json"),
        ) else {
            panic!("not a pull_request event");
        };
        assert_eq!(e.action, "ready_for_review");
        assert!(!e.pull_request.draft);
    }

    #[test]
    fn pull_request_closed_merged_parses() {
        let GithubEvent::PullRequest(e) = fixture(
            "pull_request",
            include_str!("../tests/fixtures/github/pull_request_closed_merged.json"),
        ) else {
            panic!("not a pull_request event");
        };
        assert_eq!(e.action, "closed");
        assert!(e.pull_request.merged);
        assert_eq!(e.sender.login, "octocat");
    }

    #[test]
    fn pull_request_closed_unmerged_parses() {
        let GithubEvent::PullRequest(e) = fixture(
            "pull_request",
            include_str!("../tests/fixtures/github/pull_request_closed_unmerged.json"),
        ) else {
            panic!("not a pull_request event");
        };
        assert_eq!(e.action, "closed");
        assert!(!e.pull_request.merged);
        assert_eq!(e.pull_request.state, "closed");
    }

    #[test]
    fn check_run_completed_parses() {
        let GithubEvent::CheckRun(e) = fixture(
            "check_run",
            include_str!("../tests/fixtures/github/check_run_completed.json"),
        ) else {
            panic!("not a check_run event");
        };
        assert_eq!(e.check_run.conclusion.as_deref(), Some("success"));
        assert_eq!(e.check_run.status, "completed");
    }

    #[test]
    fn review_thread_resolved_parses() {
        let GithubEvent::PullRequestReviewThread(e) = fixture(
            "pull_request_review_thread",
            include_str!("../tests/fixtures/github/pull_request_review_thread_resolved.json"),
        ) else {
            panic!("not a pull_request_review_thread event");
        };
        assert_eq!(e.action, "resolved");
        assert_eq!(e.thread.node_id, "PRRT_kwDOA1b2c84AbCdE");
    }

    #[test]
    fn other_events_and_actions_are_ignored() {
        assert!(
            parse_event("ping", br#"{"zen":"Keep it logically awesome."}"#)
                .unwrap()
                .is_none()
        );
        assert!(
            parse_event("pull_request", br#"{"action":"labeled"}"#)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn a_malformed_handled_payload_is_invalid() {
        assert!(matches!(
            parse_event("pull_request", br#"{"action":"opened"}"#),
            Err(ApiError::Invalid(_))
        ));
    }

    #[test]
    fn hex_decodes() {
        assert_eq!(decode_hex("00ff10"), Some(vec![0, 255, 16]));
        assert_eq!(decode_hex("0"), None);
        assert_eq!(decode_hex("zz"), None);
    }
}
