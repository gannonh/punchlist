//! `POST /api/github/webhook`: GitHub's deliveries, verified, parsed and queued as jobs,
//! and the job that records each one as evidence on pull requests and issues, or queues a
//! workflow load for a push that changes `.punchlist/` on the default branch.

use std::collections::{HashMap, HashSet};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use chrono::{DateTime, Utc};
use hmac::{Hmac, KeyInit, Mac};
use punchlist_api::{ErrorBody, ReviewComment, plain_review_thread_url};
use punchlist_core::{CheckRunEvidence, GITHUB_ACTIONS, Refusal, Role, Workflow, linked_issue_id};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use crate::GithubClient;
use crate::auth::Actor;
use crate::events::mark_pull_request_change;
use crate::github_api::{
    CommentBody, GithubThread, MAX_COMMENTS, ThreadCommentNode, ThreadComments, unless_gone,
};
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
    /// The GitHub App that created the run. GitHub's schema allows none, and a job queued by
    /// an older binary has none.
    #[serde(default)]
    pub app: Option<CheckApp>,
}

/// The GitHub Actions job behind a check run, which only a read of GitHub has
/// (`GithubClient::workflow_jobs`): a job that a runner took.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkflowJob {
    /// The job's name, conclusion and commit, as the jobs listing has them: the job counts
    /// for a check run only when they are the check run's (`is_behind`).
    pub name: String,
    pub conclusion: Option<String>,
    pub head_sha: String,
    /// Its run's workflow file, such as `.github/workflows/ci.yml`.
    pub path: String,
    /// The git blob sha of that file at the head commit; `None` when the commit has no file
    /// at the path.
    pub sha: Option<String>,
    /// The git blob sha of the same path on the default branch when the job was read; `None`
    /// when the default branch has no file there.
    pub default_sha: Option<String>,
    /// The bases other than the default branch that a pull request from its run's head branch
    /// has been into; `None` when no pull request from a branch of the repository is behind
    /// the run. `CheckRunEvidence::workflow_other_bases`.
    pub other_bases: Option<Vec<String>>,
}

impl WorkflowJob {
    /// Whether this job is the one behind `run`. Its id already is the run's; the rest is
    /// belt and braces against a check run edited after its job ran.
    fn is_behind(&self, run: &CheckRunData) -> bool {
        (&self.name, &self.conclusion, &self.head_sha)
            == (&run.name, &run.conclusion, &run.head_sha)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct CheckApp {
    /// Such as `github-actions`.
    #[serde(default)]
    pub slug: Option<String>,
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
    #[serde(default)]
    pub user: Option<Account>,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub html_url: Option<String>,
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
    threads: Option<Vec<GithubThread>>,
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
            let changed =
                record_check_run(tx, job.repository_id, &event.check_run, None, None).await?;
            recompute_checks(tx, job.repository_id, &event.check_run.head_sha, changed).await?;
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
                    let all = &event.thread.comments;
                    let newest = &all[all.len().saturating_sub(MAX_COMMENTS)..];
                    let comments = newest.iter().map(|comment| ThreadCommentNode {
                        author: comment.user.clone(),
                        body: CommentBody::cut(&comment.body),
                        url: comment.html_url.clone(),
                    });
                    let thread = GithubThread {
                        id: event.thread.node_id.clone(),
                        is_resolved: event.action == "resolved",
                        path: place.map(|comment| comment.path.clone()),
                        line: place.and_then(|comment| comment.line),
                        original_line: place.and_then(|comment| comment.original_line),
                        comments: ThreadComments {
                            total_count: all.len() as i32,
                            nodes: comments.collect(),
                        },
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
/// conclusion of a row last stamped at or before then (so a recorded `completed` can give way
/// to GitHub's `in_progress` for a restarted run). A row stamped after it, by a delivery or by
/// a newer read, is not overwritten: `record_read_check_runs` finds it, the pull request is
/// read again, and a second one refuses the request. A delivery carries no order from its
/// source, so its stamp cannot say which is current. `updated_at` is when the row's content
/// was current: a delivery stamps the write clock (`clock_timestamp()`, not its transaction's
/// start), a read its `read_started`, the time of its snapshot. `written_since` reads the
/// column too. Of two overlapping reads (snapshots t1 < t2): the older writes first, stamp t1,
/// and the newer sees t1 <= t2 and overwrites; the newer writes first, stamp t2, and the older
/// sees t2 <= t1 false and is skipped. A rerun gets a new id, in the same suite. A suite once
/// known is kept, and so is the app that owns the run. A delivery that changes nothing the
/// gates read (a redelivery, one of a burst as a matrix finishes) writes nothing, so it does
/// not look like a write to a request in flight; a read always stamps, which is what orders
/// it against the next read. A read also records the workflow job behind the run (`job`), or
/// that it found none; a delivery knows neither and leaves its `workflow_` columns alone.
///
/// True when the write changed what the gates read of the run (a first insert, or any of its
/// head commit, name, status, conclusion, suite, app and workflow job columns, the other
/// bases included), which a read that stamps an identical row does not: the caller marks the
/// commit's pull requests. The old row is the statement's snapshot, so a write that commits
/// between it and this one is compared against too; that write marks its own change.
// ponytail: one round trip per run, up to the page cap (1000); one `UNNEST` insert does it in
// one when a commit has that many.
async fn record_check_run(
    tx: &mut sqlx::PgConnection,
    repository_id: Uuid,
    run: &CheckRunData,
    read_started: Option<DateTime<Utc>>,
    job: Option<&WorkflowJob>,
) -> sqlx::Result<bool> {
    let app = run.app.as_ref().and_then(|app| app.slug.as_deref());
    sqlx::query_scalar!(
        r#"WITH old AS (
               SELECT head_sha, name, status, conclusion, check_suite_id, app,
                      workflow_path, workflow_sha, workflow_default_sha, workflow_other_bases
               FROM check_run WHERE id = $1),
           up AS (
        INSERT INTO check_run (id, repository_id, head_sha, name, status, conclusion,
                                check_suite_id, updated_at, app,
                                workflow_path, workflow_sha, workflow_default_sha,
                                workflow_other_bases)
         VALUES ($1, $2, $3, $4, $5, $6, $7, COALESCE($8, clock_timestamp()), $9, $10, $11, $12,
                 $13)
         ON CONFLICT (id) DO UPDATE SET head_sha = EXCLUDED.head_sha,
             name = EXCLUDED.name, status = EXCLUDED.status,
             conclusion = EXCLUDED.conclusion,
             check_suite_id = COALESCE(EXCLUDED.check_suite_id, check_run.check_suite_id),
             app = COALESCE(EXCLUDED.app, check_run.app),
             workflow_path = CASE WHEN $8::timestamptz IS NULL THEN check_run.workflow_path
                                  ELSE EXCLUDED.workflow_path END,
             workflow_sha = CASE WHEN $8::timestamptz IS NULL THEN check_run.workflow_sha
                                 ELSE EXCLUDED.workflow_sha END,
             workflow_default_sha = CASE WHEN $8::timestamptz IS NULL
                                         THEN check_run.workflow_default_sha
                                         ELSE EXCLUDED.workflow_default_sha END,
             workflow_other_bases = CASE WHEN $8::timestamptz IS NULL
                                         THEN check_run.workflow_other_bases
                                         ELSE EXCLUDED.workflow_other_bases END,
             updated_at = COALESCE($8, clock_timestamp())
         WHERE CASE WHEN $8::timestamptz IS NOT NULL THEN check_run.updated_at <= $8
               ELSE (CASE check_run.status WHEN 'completed' THEN 2 WHEN 'in_progress' THEN 1
                      ELSE 0 END)
                  <= (CASE EXCLUDED.status WHEN 'completed' THEN 2 WHEN 'in_progress' THEN 1
                      ELSE 0 END)
                  AND (check_run.head_sha, check_run.name, check_run.status,
                       check_run.conclusion, check_run.check_suite_id, check_run.app)
                      IS DISTINCT FROM
                      (EXCLUDED.head_sha, EXCLUDED.name, EXCLUDED.status, EXCLUDED.conclusion,
                       COALESCE(EXCLUDED.check_suite_id, check_run.check_suite_id),
                       COALESCE(EXCLUDED.app, check_run.app)) END
        RETURNING head_sha, name, status, conclusion, check_suite_id, app,
                  workflow_path, workflow_sha, workflow_default_sha, workflow_other_bases)
           SELECT EXISTS (SELECT 1 FROM up LEFT JOIN old ON true
                          WHERE old.name IS NULL
                             OR (old.head_sha, old.name, old.status, old.conclusion,
                                 old.check_suite_id, old.app, old.workflow_path,
                                 old.workflow_sha, old.workflow_default_sha,
                                 old.workflow_other_bases)
                                IS DISTINCT FROM
                                (up.head_sha, up.name, up.status, up.conclusion,
                                 up.check_suite_id, up.app, up.workflow_path,
                                 up.workflow_sha, up.workflow_default_sha,
                                 up.workflow_other_bases)) AS "changed!""#,
        run.id,
        repository_id,
        run.head_sha,
        run.name,
        run.status,
        run.conclusion,
        run.check_suite.as_ref().map(|suite| suite.id),
        read_started,
        app,
        job.map(|job| job.path.as_str()),
        job.and_then(|job| job.sha.as_deref()),
        job.and_then(|job| job.default_sha.as_deref()),
        job.and_then(|job| job.other_bases.as_deref()),
    )
    .fetch_one(&mut *tx)
    .await
}

/// Records the check runs GitHub lists on `head_sha`, in their own short transaction, before
/// the pull request's row is locked: a `check_run` delivery's job takes the run's row and then
/// the pull request's, and this takes them in the same order. Runs are keyed by id and need no
/// version ordering, so they are kept even if the pull request's write is then dropped.
///
/// True when a run on the commit was written since `read_started`, by a delivery that changed
/// it or by a newer read. `written_since` asks the same of the same rows later and refuses the
/// request; asked here, where GitHub can still be read, the caller reads again instead.
async fn record_read_check_runs(
    pool: &sqlx::PgPool,
    repository_id: Uuid,
    head_sha: &str,
    mut runs: Vec<CheckRunData>,
    jobs: &HashMap<i64, WorkflowJob>,
    read_started: DateTime<Utc>,
) -> sqlx::Result<bool> {
    // The head asked for is the head judged: a run listed for another is not this commit's.
    runs.retain(|run| run.head_sha == head_sha);
    runs.sort_by_key(|run| run.id);
    let mut tx = pool.begin().await?;
    let mut changed = false;
    for run in &runs {
        let job = jobs.get(&run.id).filter(|job| job.is_behind(run));
        changed |= record_check_run(&mut tx, repository_id, run, Some(read_started), job).await?;
    }
    recompute_checks(&mut tx, repository_id, head_sha, changed).await?;
    let conflict = sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM check_run WHERE repository_id = $1 AND head_sha = $2
                          AND updated_at > $3) AS "conflict!""#,
        repository_id,
        head_sha,
        read_started,
    )
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(conflict)
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
    threads: &[GithubThread],
) -> sqlx::Result<()> {
    let ids: Vec<String> = threads.iter().map(|thread| thread.id.clone()).collect();
    let forgotten = sqlx::query!(
        "DELETE FROM review_thread WHERE pull_request_id = $1 AND node_id <> ALL($2)",
        pull_request_id,
        &ids,
    )
    .execute(&mut *tx)
    .await?;
    // A row that is gone has no stamp of its own for `written_since`: the pull request's says so.
    if forgotten.rows_affected() > 0 {
        sqlx::query!(
            "UPDATE pull_request SET updated_at = clock_timestamp() WHERE id = $1",
            pull_request_id,
        )
        .execute(&mut *tx)
        .await?;
    }
    Ok(())
}

/// A thread's comments as `review_thread.comments` holds them. The text is from outside the
/// team: `get_issue` shows it to an agent, inside its fence.
fn recorded_comments(comments: &ThreadComments) -> Vec<ReviewComment> {
    let recorded = comments.nodes.iter().map(|comment| ReviewComment {
        author: comment
            .author
            .as_ref()
            .map_or("ghost", |author| author.login.as_str())
            .to_string(),
        body: comment.body.text.clone(),
        cut: comment.body.cut,
    });
    recorded.collect()
}

/// Records review threads on a pull request, recounts its open ones and marks its issue for a
/// `pull_request` event. The caller holds the pull request's row lock: the lock order is
/// workspace, pull request, issue, then threads.
/// `updated_at` is the write clock under that lock, not the transaction's start, so that it
/// orders against a `PullRequestRead` noted under the same lock. It moves only when something
/// the gates read changed (the pull request, `resolved`, `path`, `line`): every thread event and
/// every refresh rewrites all of a pull request's threads, mostly with what is recorded.
async fn record_review_threads(
    tx: &mut sqlx::PgConnection,
    pull_request_id: Uuid,
    threads: &[GithubThread],
) -> sqlx::Result<()> {
    let (ids, resolved): (Vec<String>, Vec<bool>) = threads
        .iter()
        .map(|thread| (thread.id.clone(), thread.is_resolved))
        .unzip();
    let paths: Vec<Option<String>> = threads.iter().map(|t| t.path.clone()).collect();
    let lines: Vec<Option<i64>> = threads.iter().map(|t| t.line.or(t.original_line)).collect();
    // An event can come without its comments: NULL then, which keeps what is recorded.
    let (comments, totals): (Vec<Option<serde_json::Value>>, Vec<Option<i32>>) = threads
        .iter()
        .map(|t| &t.comments)
        .map(|c| match c.nodes.is_empty() {
            true => (None, None),
            false => (
                Some(serde_json::json!(recorded_comments(c))),
                Some(c.total_count),
            ),
        })
        .unzip();
    let urls: Vec<Option<String>> = (threads.iter())
        .map(|t| t.comments.nodes.first()?.url.as_deref())
        .map(|url| url.and_then(plain_review_thread_url).map(str::to_string))
        .collect();
    sqlx::query!(
        "INSERT INTO review_thread
             (node_id, pull_request_id, resolved, path, line, comments, comments_total, url,
              updated_at)
         SELECT node_id, $1, resolved, path, line, comments, comments_total, url,
                clock_timestamp()
         FROM UNNEST($2::text[], $3::boolean[], $4::text[], $5::bigint[], $6::jsonb[], $7::int[],
                     $8::text[])
             AS t (node_id, resolved, path, line, comments, comments_total, url)
         ON CONFLICT (node_id) DO UPDATE SET pull_request_id = EXCLUDED.pull_request_id,
             resolved = EXCLUDED.resolved,
             updated_at = CASE WHEN (review_thread.pull_request_id, review_thread.resolved,
                                     review_thread.path, review_thread.line)
                                    IS DISTINCT FROM
                                    (EXCLUDED.pull_request_id, EXCLUDED.resolved,
                                     coalesce(EXCLUDED.path, review_thread.path),
                                     coalesce(EXCLUDED.line, review_thread.line))
                               THEN EXCLUDED.updated_at ELSE review_thread.updated_at END,
             -- An event without its comments does not forget where the thread is, or what
             -- it says. One with them replaces what was recorded.
             path = coalesce(EXCLUDED.path, review_thread.path),
             line = coalesce(EXCLUDED.line, review_thread.line),
             comments = coalesce(EXCLUDED.comments, review_thread.comments),
             comments_total = coalesce(EXCLUDED.comments_total, review_thread.comments_total),
             -- The link goes with the comments it came with, and is NULL when not plain.
             url = CASE WHEN EXCLUDED.comments IS NULL THEN review_thread.url
                        ELSE EXCLUDED.url END",
        pull_request_id,
        &ids,
        &resolved,
        &paths as &[Option<String>],
        &lines as &[Option<i64>],
        &comments as &[Option<serde_json::Value>],
        &totals as &[Option<i32>],
        &urls as &[Option<String>],
    )
    .execute(&mut *tx)
    .await?;
    // Only a changed count moves the row.
    let issue = sqlx::query_scalar!(
        "UPDATE pull_request SET open_threads = counted.open
         FROM (SELECT count(*)::int AS open FROM review_thread
               WHERE pull_request_id = $1 AND NOT resolved) counted
         WHERE id = $1 AND open_threads IS DISTINCT FROM counted.open
         RETURNING issue_id",
        pull_request_id,
    )
    .fetch_optional(&mut *tx)
    .await?;
    mark_pull_request_change(tx, issue.flatten().as_slice()).await
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
    let was = sqlx::query!(
        "SELECT state, issue_id, updated_at FROM pull_request
         WHERE repository_id = $1 AND number = $2 FOR UPDATE",
        repository_id,
        pr.number
    )
    .fetch_optional(&mut *tx)
    .await?;
    let applied = sqlx::query!(
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
             -- The write clock, under the row's lock, when something the gates read
             -- changed: see `PullRequestRead`.
             updated_at = CASE WHEN (pull_request.title, pull_request.branch,
                                     pull_request.head_sha, pull_request.state,
                                     pull_request.draft, pull_request.merge_state,
                                     pull_request.issue_id)
                                    IS DISTINCT FROM
                                    (EXCLUDED.title, EXCLUDED.branch, EXCLUDED.head_sha,
                                     EXCLUDED.state, EXCLUDED.draft, EXCLUDED.merge_state,
                                     EXCLUDED.issue_id)
                               THEN clock_timestamp() ELSE pull_request.updated_at END
         WHERE pull_request.github_updated_at <= EXCLUDED.github_updated_at
           AND pull_request.state <> 'merged'
         RETURNING id, updated_at",
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
    let Some(applied) = applied else {
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
    let id = applied.id;
    // `updated_at` moves only when something the row shows or the gates read changed.
    if was
        .as_ref()
        .is_none_or(|was| was.updated_at != applied.updated_at)
    {
        mark_pull_request_change(tx, linked.as_slice()).await?;
    }
    recompute_checks(tx, repository_id, &pr.head.sha, false).await?;
    // The issue it named before no longer shows it once it names another.
    let unlinked = was.as_ref().and_then(|was| was.issue_id.clone());
    if unlinked.is_some() && unlinked != linked {
        mark_pull_request_change(tx, unlinked.as_slice()).await?;
    }
    let closed = state != "open" && was.is_none_or(|was| was.state == "open");
    if let (true, Some(issue_id)) = (closed, linked) {
        let closed = Closed {
            number: pr.number,
            url: &pr.html_url,
            merged: pr.is_merged(),
        };
        close_issue_on_pull_request(tx, workspace_id, closed, closed_by, delivery_id, &issue_id)
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
/// A pull request whose write is dropped (`claim_read`), that a newer payload was recorded
/// for since GitHub answered, or with a check run written since the read began
/// (`record_read_check_runs`) is read once more, alone.
///
/// Returns each of the issue's pull requests as this left it, for `written_since`. One whose
/// second read is dropped too is not among them: what is recorded for it is another reader's,
/// so `written_since` refuses a request whose gates would read it.
pub(crate) async fn refresh_pull_requests(
    github: &GithubClient,
    pool: &sqlx::PgPool,
    workspace_id: Uuid,
    issue_id: &str,
) -> Result<Vec<PullRequestRead>, ApiError> {
    let mut read = Vec::new();
    let dropped = refresh_once(github, pool, workspace_id, issue_id, None, &mut read).await?;
    if !dropped.is_empty() {
        let only = Some(dropped.as_slice());
        refresh_once(github, pool, workspace_id, issue_id, only, &mut read).await?;
    }
    Ok(read)
}

/// A pull request of an issue as a gated request's refresh left it, noted under the row's
/// lock. Every writer of the row or of its review threads holds that lock and, when it changes
/// what the gates read, stamps `updated_at` from the write clock, so one that wrote before this
/// was noted stamped before `at`, and one that writes after stamps after it.
pub(crate) struct PullRequestRead {
    id: Uuid,
    at: DateTime<Utc>,
    head_sha: String,
    /// `read_started` of this request's read of the check runs on `head_sha`. None for a pull
    /// request whose runs were not read (closed, or gone from GitHub): they stand as of `at`.
    checks_read: Option<DateTime<Utc>>,
}

/// Notes a pull request for `written_since`, if it is the issue's. The caller holds the row's
/// lock, and has made its writes.
async fn note_read(
    tx: &mut sqlx::PgConnection,
    repository_id: Uuid,
    number: i64,
    issue_id: &str,
    checks_read: Option<DateTime<Utc>>,
) -> sqlx::Result<Option<PullRequestRead>> {
    let row = sqlx::query!(
        r#"SELECT id, head_sha, clock_timestamp() AS "at!" FROM pull_request
           WHERE repository_id = $1 AND number = $2 AND issue_id = $3"#,
        repository_id,
        number,
        issue_id,
    )
    .fetch_optional(&mut *tx)
    .await?;
    Ok(row.map(|row| PullRequestRead {
        id: row.id,
        at: row.at,
        head_sha: row.head_sha,
        checks_read,
    }))
}

/// Locks the pull request rows a gated request's gates read: the issue's, and those its
/// refresh noted. After the workspace's row and before the issue's, as the lock order has it,
/// and holding no check run's row: a `check_run` delivery takes the run's row and then these.
/// Until the request is judged no delivery or other read changes them, their review threads
/// (written under these locks) or their check-run summary.
///
/// In id order, as `recompute_checks` takes the pull requests of one head commit.
// ponytail: a payload's job that links a second pull request on a head commit to the issue
// holds that row and a key share on the issue, and waits in `recompute_checks` for a row held
// here, while this waits for the issue's row. Postgres ends one of the two: a job is tried
// again, and `move_issue` answers a request `evidence_changed`. Taking the issue's key share
// before the pull request's row in `record_pull_request` removes the cycle.
pub(crate) async fn lock_pull_requests(
    tx: &mut sqlx::PgConnection,
    issue_id: &str,
    read: &[PullRequestRead],
) -> sqlx::Result<()> {
    let ids: Vec<Uuid> = read.iter().map(|read| read.id).collect();
    sqlx::query!(
        "SELECT id FROM pull_request WHERE issue_id = $1 OR id = ANY($2) ORDER BY id FOR UPDATE",
        issue_id,
        &ids,
    )
    .fetch_all(&mut *tx)
    .await?;
    Ok(())
}

/// Whether anything a gated request's gates read changed since its refresh read it, which
/// refuses the request (`ApiError::EvidenceChanged`): a delivery, or another read, was recorded
/// between this request's read and its judgment, and the request is judged on what it read.
/// Asked after the gates have read their evidence, so a row they saw is a row this sees. True
/// for:
/// - a pull request of the issue the refresh did not note (linked since, or dropped on both
///   reads), or a noted one changed since, or with a review thread forgotten since;
/// - a review thread on an open one, changed or new since;
/// - a check run on an open one's head commit stamped after this request's read of that
///   commit began. `record_read_check_runs` asks the same earlier so that the request can read
///   again; this is for a run written after that.
///
/// The mirror of `evidence` in `issues.rs`, which decides what the gates read: the issue's
/// pull requests, and the threads and head-commit check runs of the open ones. Change them
/// together. This counts every run on the head, also one a rerun superseded.
///
/// Not flagged: a check run a delivery stamped before `read_started` and committed after the
/// read's transaction, which GitHub's list did not have (one it had, the read waited for and
/// replaced). Only GitHub's list lagging a just-created run does that, and the run is then
/// queued or in progress, which fails `ci_green`.
// ponytail: `pull_request.version` (reads recorded, for `claim_read`) and these stamps both
// say "written since I read it". One counter per pull request, raised by every change to what
// the gates read, would replace the stamps and the clock they depend on: a clock set back
// between a note and a write hides the write, and two reads of one commit begun in the same
// microsecond are not told apart. It cannot be `version`: a delivery raising that would drop
// the reads in flight, so it is a second column, raised in every writer.
pub(crate) async fn written_since(
    tx: &mut sqlx::PgConnection,
    issue_id: &str,
    read: &[PullRequestRead],
) -> sqlx::Result<bool> {
    let ids: Vec<Uuid> = read.iter().map(|read| read.id).collect();
    let noted: Vec<DateTime<Utc>> = read.iter().map(|read| read.at).collect();
    let heads: Vec<String> = read.iter().map(|read| read.head_sha.clone()).collect();
    let checks_read: Vec<Option<DateTime<Utc>>> =
        read.iter().map(|read| read.checks_read).collect();
    sqlx::query_scalar!(
        r#"SELECT EXISTS (
               SELECT 1 FROM pull_request p
               LEFT JOIN UNNEST($2::uuid[], $3::timestamptz[]) AS r (id, at) ON r.id = p.id
               WHERE (p.issue_id = $1 OR r.id IS NOT NULL)
                 AND (r.id IS NULL OR p.updated_at > r.at
                      OR (p.state = 'open' AND (
                          EXISTS (SELECT 1 FROM review_thread t
                                  WHERE t.pull_request_id = p.id AND t.updated_at > r.at)
                          -- Two of the issue's pull requests can have one head commit: the
                          -- last read of it is the one its runs are stamped with.
                          OR EXISTS (SELECT 1 FROM check_run c
                                     WHERE c.repository_id = p.repository_id
                                       AND c.head_sha = p.head_sha
                                       AND c.updated_at > coalesce(
                                           (SELECT max(h.checks_read)
                                            FROM UNNEST($4::text[], $5::timestamptz[])
                                                AS h (sha, checks_read)
                                            WHERE h.sha = p.head_sha),
                                           r.at)))))
           ) AS "written!""#,
        issue_id,
        &ids,
        &noted,
        &heads,
        &checks_read as &[Option<DateTime<Utc>>],
    )
    .fetch_one(&mut *tx)
    .await
}

/// One pass of `refresh_pull_requests`, over the pull requests numbered in `only` when it is
/// given. Returns the numbers of those whose write was dropped, and adds each pull request it
/// leaves recorded for the issue to `read`.
// ponytail: GitHub's pull request names who merged it but not who closed it, so a close read
// here credits nobody; read the issue's `closed_by` if the name matters.
async fn refresh_once(
    github: &GithubClient,
    pool: &sqlx::PgPool,
    workspace_id: Uuid,
    issue_id: &str,
    only: Option<&[i64]>,
    read: &mut Vec<PullRequestRead>,
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
    // The names of the GitHub Actions checks the active workflow requires. The gates read the
    // workflow again under the issue's lock: one that has come to require a check since has
    // no job recorded for it, and refuses.
    let (workflow, _) = active_workflow(&mut *pool.acquire().await?, workspace_id).await?;
    let required: HashSet<String> = (workflow.required_checks().iter())
        .filter(|check| check.app == GITHUB_ACTIONS)
        .map(|check| check.name.clone())
        .collect();
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
    let asked = async {
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
    let fresh = asked
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
        let mut checks_read = None;
        if pr.state == "open" {
            let read_started = sqlx::query_scalar!(r#"SELECT clock_timestamp() AS "at!""#)
                .fetch_one(pool)
                .await?;
            let runs = (github.check_runs(owner, name, &pr.head.sha).await)
                .map_err(|error| unreadable("check runs", error))?;
            // The workflow jobs behind them, which only a required GitHub Actions check is
            // judged on: read for the suites that hold a run of such a check's name, so a
            // workspace that requires none asks GitHub nothing more (and needs no Actions
            // permission). Read after the check runs: a job that has since moved on no
            // longer matches its check run, and does not count.
            let suites: HashSet<i64> = (runs.iter())
                .filter(|run| required.contains(&run.name))
                .filter_map(|run| run.check_suite.as_ref().map(|suite| suite.id))
                .collect();
            let jobs = match suites.is_empty() {
                true => HashMap::new(),
                false => (github
                    .workflow_jobs(owner, name, &pr.head.sha, &suites)
                    .await)
                    .map_err(|error| unreadable("workflow runs", error))?,
            };
            let conflict = record_read_check_runs(
                pool,
                repository.id,
                &pr.head.sha,
                runs,
                &jobs,
                read_started,
            )
            .await?;
            if conflict {
                // A run was written since the read began: read again, after it.
                dropped.push(pr.number);
                continue;
            }
            checks_read = Some(read_started);
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
        // A payload newer than GitHub's answer, recorded since, kept its row: that is not
        // what this request read (it may be ready where this read a draft, or open, with runs
        // never read, where this read it closed), so this is read again. Newer, not different:
        // a merged row takes no write, and stands. Also a push that kept its head, whatever its
        // time: the runs recorded are for another commit.
        let row = sqlx::query!(
            "SELECT head_sha, github_updated_at FROM pull_request WHERE id = $1",
            id
        )
        .fetch_one(&mut *tx)
        .await?;
        if row.github_updated_at > pr.updated_at
            || (pr.state == "open" && row.head_sha != pr.head.sha)
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
        read.extend(note_read(&mut tx, repository.id, pr.number, issue_id, checks_read).await?);
        tx.commit().await?;
    }
    // A pull request of the issue that GitHub no longer has stands as recorded: as it is now.
    for &number in only.unwrap_or(&known) {
        if fresh.iter().any(|pr| pr.number == number) {
            continue;
        }
        let mut tx = pool.begin().await?;
        sqlx::query!(
            "SELECT id FROM pull_request WHERE repository_id = $1 AND number = $2 FOR UPDATE",
            repository.id,
            number,
        )
        .fetch_optional(&mut *tx)
        .await?;
        read.extend(note_read(&mut tx, repository.id, number, issue_id, None).await?);
        tx.commit().await?;
    }
    Ok(dropped)
}

/// The check runs on a commit that count: a rerun is a new run with the same name in the same
/// suite, so the newest run of each name in each suite (two workflows may name a job alike).
/// A run with no suite (recorded before `check_suite_id`, or delivered without one) counts
/// only while no newer run of its name is on the commit: a rerun with a suite supersedes it.
/// Runs of one name from two apps are two checks, so only a run from the same app supersedes
/// it, or any run when either's app was not recorded (a row from before `app`).
///
/// Of the runs of one name in one suite, a completed one that no workflow job ran comes after
/// the others, whatever their ids: a check run created through the API with `checks: write`
/// joins the suite of the real run of its name with a higher id, and must not hide it. Only
/// a completed one: a rerun still waiting for a runner has no job either, and is the newest.
/// And only in a suite where a read recorded a workflow job: jobs are read for the suites of
/// the checks the workflow requires alone, so in any other suite (another app's, too) no run
/// has a job recorded, and the newest id counts, as it did before jobs were read.
pub(crate) async fn newest_check_runs(
    tx: &mut sqlx::PgConnection,
    repository_id: Uuid,
    head_sha: &str,
) -> sqlx::Result<Vec<CheckRunEvidence>> {
    sqlx::query_as!(
        CheckRunEvidence,
        "SELECT DISTINCT ON (c.check_suite_id, c.app, c.name)
                c.name, c.status, c.conclusion, c.app,
                c.workflow_path, c.workflow_sha, c.workflow_default_sha, c.workflow_other_bases
         FROM check_run c WHERE c.repository_id = $1 AND c.head_sha = $2
           -- Whatever `workflow_path` either has: a run with no suite has no job read for it.
           AND NOT (c.check_suite_id IS NULL AND EXISTS (
               SELECT 1 FROM check_run n WHERE n.repository_id = c.repository_id
                 AND n.head_sha = c.head_sha AND n.name = c.name AND n.id > c.id
                 AND (c.app IS NULL OR n.app IS NULL OR n.app = c.app)))
         ORDER BY c.check_suite_id, c.app, c.name,
                  (c.workflow_path IS NULL AND c.status = 'completed' AND EXISTS (
                      SELECT 1 FROM check_run w WHERE w.repository_id = c.repository_id
                        AND w.head_sha = c.head_sha AND w.check_suite_id = c.check_suite_id
                        AND w.workflow_path IS NOT NULL)),
                  c.id DESC",
        repository_id,
        head_sha,
    )
    .fetch_all(&mut *tx)
    .await
}

/// Summarizes the check runs on a commit onto every pull request whose head it is, and marks
/// their issues for a `pull_request` event when the summary changed, or when `evidence_changed`:
/// a run's row changed in a column a gate reads, as when a read adds the workflow job behind a
/// run, which the counts do not show.
async fn recompute_checks(
    tx: &mut sqlx::PgConnection,
    repository_id: Uuid,
    head_sha: &str,
    evidence_changed: bool,
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
    let issues = sqlx::query_scalar!(
        // In id order, as `lock_pull_requests` takes them.
        r#"WITH summarized AS (
               UPDATE pull_request SET checks = $3, checks_total = $4, checks_passed = $5
               WHERE id IN (SELECT id FROM pull_request WHERE repository_id = $1 AND head_sha = $2
                            ORDER BY id FOR UPDATE)
                 AND ($6 OR (checks, checks_total, checks_passed) IS DISTINCT FROM ($3, $4, $5))
               RETURNING issue_id)
           SELECT DISTINCT issue_id AS "issue_id!" FROM summarized WHERE issue_id IS NOT NULL"#,
        repository_id,
        head_sha,
        checks,
        runs.len() as i32,
        passed as i32,
        evidence_changed,
    )
    .fetch_all(&mut *tx)
    .await?;
    mark_pull_request_change(tx, &issues).await
}

/// The workspace's actor for a GitHub login, made on first use. With no login it is GitHub
/// itself, for what nobody is credited with; no GitHub login is empty, so that actor is never
/// a user's.
pub(crate) async fn github_actor(
    conn: &mut sqlx::PgConnection,
    workspace_id: Uuid,
    login: Option<&str>,
) -> sqlx::Result<Uuid> {
    let (name, login) = login.map_or(("GitHub", ""), |login| (login, login));
    sqlx::query_scalar!(
        "INSERT INTO actor (workspace_id, name, role, github_login)
         VALUES ($1, $2, 'github', $3)
         ON CONFLICT (workspace_id, github_login) WHERE github_login IS NOT NULL
         DO UPDATE SET name = EXCLUDED.name
         RETURNING id",
        workspace_id,
        name,
        login,
    )
    .fetch_one(&mut *conn)
    .await
}

/// The pull request a close moves an issue on.
struct Closed<'a> {
    number: i64,
    url: &'a str,
    merged: bool,
}

/// An issue that enters a status waiting on `pr_merged` after its pull request merged moves
/// on as the merge's webhook would have moved it: that close was applied once, while the
/// issue was elsewhere (`record_pull_request`), and does not come again. Only the pull
/// request the gates read counts: the issue's highest-numbered one, merged, with none of
/// its pull requests open. `pr_closed_unmerged` is not caught up: the next claim reuses a
/// closed pull request's branch. The caller holds the workspace's lock, so a merge being
/// recorded now either is seen here or finds the issue in the new status.
pub(crate) async fn catch_up_merge(
    tx: &mut sqlx::PgConnection,
    workspace_id: Uuid,
    workflow: &Workflow,
    issue_id: &str,
    status: &str,
) -> Result<(), ApiError> {
    let Some(transition) = workflow.transition_on(status, "pr_merged") else {
        return Ok(());
    };
    // A failed event is final for this status entry, including across restarts. Use
    // timeline sequence numbers: a catch-up refusal and the entry can share a transaction.
    let refused = sqlx::query_scalar!(
        "SELECT EXISTS (
            SELECT 1 FROM event e
            JOIN refusal r ON r.id = e.refusal_id
            JOIN actor a ON a.id = r.actor_id
            WHERE e.issue_id = $1 AND r.from_status = $2 AND r.to_status = $3
              AND a.role = 'github'
              AND e.seq > COALESCE((SELECT max(seq) FROM event
                                    WHERE issue_id = $1 AND kind = 'transition'), 0)
         )",
        issue_id,
        status,
        transition.to.as_str(),
    )
    .fetch_one(&mut *tx)
    .await?;
    if refused == Some(true) {
        return Ok(());
    }
    let merged = sqlx::query!(
        "SELECT number, url FROM pull_request p
         WHERE issue_id = $1 AND state = 'merged'
           AND NOT EXISTS (SELECT 1 FROM pull_request o
                           WHERE o.issue_id = $1 AND (o.state = 'open' OR o.number > p.number))",
        issue_id,
    )
    .fetch_optional(&mut *tx)
    .await?;
    let Some(pr) = merged else {
        return Ok(());
    };
    let closed = Closed {
        number: pr.number,
        url: &pr.url,
        merged: true,
    };
    close_issue_on_pull_request(tx, workspace_id, closed, None, None, issue_id).await
}

/// `catch_up_merge` for every issue, once per `serve`: an issue that entered such a status
/// before the catch-up existed, or whose merge was recorded while no server ran it, moves on
/// without re-entering the status. An issue it moved is no longer in that status, so a second
/// start writes nothing.
pub async fn catch_up_merges_at_start(pool: &sqlx::PgPool) -> Result<(), ApiError> {
    let workspaces = sqlx::query_scalar!("SELECT id FROM workspace ORDER BY created_at")
        .fetch_all(pool)
        .await?;
    for workspace_id in workspaces {
        if let Err(error) = catch_up_workspace_merges(pool, workspace_id).await {
            tracing::error!(%workspace_id, %error, "could not catch up merged pull requests at start");
        }
    }
    Ok(())
}

/// Isolate a workspace's catch-up transaction: an unreadable stored workflow must not
/// prevent other workspaces from catching up, or prevent the server from starting.
async fn catch_up_workspace_merges(
    pool: &sqlx::PgPool,
    workspace_id: Uuid,
) -> Result<(), ApiError> {
    let mut tx = pool.begin().await?;
    lock_workspace(&mut tx, workspace_id).await?;
    let (workflow, _) = active_workflow(&mut tx, workspace_id).await?;
    let issues = sqlx::query!(
        "SELECT id, status FROM issue WHERE workspace_id = $1 ORDER BY number",
        workspace_id
    )
    .fetch_all(&mut *tx)
    .await?;
    for issue in issues {
        catch_up_merge(&mut tx, workspace_id, &workflow, &issue.id, &issue.status).await?;
    }
    tx.commit().await?;
    Ok(())
}

/// A closed pull request moves its linked issue if the workflow has a transition on that
/// event from the issue's status and that transition's gates pass. With no such transition
/// the pull request's new state is all that is recorded. With a failed gate the issue stays
/// where it is and the refusal goes on its timeline: no status, transition or comment.
/// Runs once per close (see `record_pull_request`), and for a merge recorded before the issue
/// entered the status that waits on it (`catch_up_merge`). `closed_by` is the GitHub login
/// that closed it, when known.
async fn close_issue_on_pull_request(
    tx: &mut sqlx::PgConnection,
    workspace_id: Uuid,
    pr: Closed<'_>,
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
    let name = if pr.merged {
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
    let name = closed_by.unwrap_or("GitHub");
    let actor = Actor {
        id: github_actor(tx, workspace_id, closed_by).await?,
        workspace_id,
        name: name.to_string(),
        role: Role::Github,
        kind: None,
        token_hash: None,
    };
    let checked = check_gates(
        tx,
        &workflow,
        transition,
        name,
        issue_id,
        &status,
        Some(pr.number),
    );
    let gates = match checked.await {
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
    let how = if pr.merged {
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
        let body = format!("Pull request #{} was {how}{by}.\n\n{}", pr.number, pr.url);
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
        assert_eq!(
            e.thread.comments[0].html_url.as_deref(),
            Some("https://github.com/gannonh/punchlist/pull/12#discussion_r1700001")
        );
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
