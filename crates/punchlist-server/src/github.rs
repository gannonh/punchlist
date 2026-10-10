//! `POST /api/github/webhook`: GitHub's deliveries, verified, parsed and queued as jobs,
//! and the job that records each one as evidence on pull requests and issues, or queues a
//! workflow load for a push that changes `.punchlist/` on the default branch.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use chrono::{DateTime, Utc};
use hmac::{Hmac, KeyInit, Mac};
use punchlist_api::ErrorBody;
use punchlist_core::{Role, linked_issue_id};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use crate::GithubClient;
use crate::auth::Actor;
use crate::issues::{record_comment, write_transition};
use crate::workflow::{active_workflow, queue_workflow_load};
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

/// Records one delivery. Runs inside the job's transaction, and is safe to run again.
pub(crate) async fn process(
    tx: &mut sqlx::PgConnection,
    payload: serde_json::Value,
) -> anyhow::Result<()> {
    let job: GithubJob = serde_json::from_value(payload)?;
    match &job.event {
        GithubEvent::PullRequest(event) => {
            // Only a `closed` event's sender closed it; another event may still be the first
            // to carry the closed state.
            let closed_by = (event.action == "closed").then_some(event.sender.login.as_str());
            record_pull_request(
                tx,
                job.workspace_id,
                job.repository_id,
                &event.pull_request,
                closed_by,
                Some(&job.delivery_id),
            )
            .await?;
        }
        GithubEvent::CheckRun(event) => {
            let run = &event.check_run;
            sqlx::query!(
                "INSERT INTO check_run (id, repository_id, head_sha, name, status, conclusion)
                 VALUES ($1, $2, $3, $4, $5, $6)
                 ON CONFLICT (id) DO UPDATE SET head_sha = EXCLUDED.head_sha,
                     name = EXCLUDED.name, status = EXCLUDED.status,
                     conclusion = EXCLUDED.conclusion, updated_at = now()
                 -- A run only moves forward (queued, in_progress, completed), so a delivery
                 -- arriving late does not undo a newer one. A rerun gets a new id.
                 WHERE (CASE check_run.status WHEN 'completed' THEN 2 WHEN 'in_progress' THEN 1
                        ELSE 0 END)
                    <= (CASE EXCLUDED.status WHEN 'completed' THEN 2 WHEN 'in_progress' THEN 1
                        ELSE 0 END)",
                run.id,
                job.repository_id,
                run.head_sha,
                run.name,
                run.status,
                run.conclusion,
            )
            .execute(&mut *tx)
            .await?;
            recompute_checks(tx, job.repository_id, &run.head_sha).await?;
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
            let known = sqlx::query_scalar!(
                "SELECT id FROM pull_request WHERE repository_id = $1 AND number = $2",
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
                        &event.pull_request,
                        None,
                        Some(&job.delivery_id),
                    )
                    .await?
                }
            };
            sqlx::query!(
                "INSERT INTO review_thread (node_id, pull_request_id, resolved)
                 VALUES ($1, $2, $3)
                 ON CONFLICT (node_id) DO UPDATE SET pull_request_id = EXCLUDED.pull_request_id,
                     resolved = EXCLUDED.resolved, updated_at = now()",
                event.thread.node_id,
                pull_request_id,
                event.action == "resolved",
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
        }
    }
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
    // ponytail: two deliveries that are both the first to record a pull request, closed and
    // unmerged, in jobs running at once, both read no row and both count as closing it; the
    // issue having left the status the event moves it from is then the only guard. Lock the
    // repository row if that bites.
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
        // actions in one second still apply in delivery order.
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

/// Brings the pull requests an issue's gates read up to date with GitHub. A webhook reaches
/// the server seconds after GitHub, and an agent asks for its transition right after it
/// opens or readies a pull request; a gate must not read the state from before. When GitHub
/// cannot be reached the recorded pull requests stand, so an outage does not stop moves.
///
/// A recorded pull request that GitHub now reports closed or merged is recorded so, and
/// moves the issue as its webhook would have: recording a merge stops that webhook from
/// applying. No delivery caused such a move, so its transition carries none.
///
/// Each pull request is recorded in its own transaction, as its webhook's job would, and
/// before the caller locks the issue: a job holds the pull request's row and waits for the
/// issue's, so taking the issue's lock first deadlocks.
///
/// A failure to record one fails the request, unlike a failure to read GitHub. What GitHub
/// said is then known and could not be stored, so letting the gates read the older rows
/// could pass a request on a pull request known to be merged or closed. The requester asks
/// again; the pull requests already recorded stay recorded.
// ponytail: GitHub's pull request names who merged it but not who closed it, so a close read
// here credits nobody; read the issue's `closed_by` if the name matters.
pub(crate) async fn refresh_pull_requests(
    github: &GithubClient,
    pool: &sqlx::PgPool,
    workspace_id: Uuid,
    issue_id: &str,
) -> Result<(), ApiError> {
    let Some(repository) = sqlx::query!(
        "SELECT r.id, r.owner, r.name, w.issue_prefix FROM repository r
         JOIN workspace w ON w.id = r.workspace_id WHERE r.workspace_id = $1",
        workspace_id,
    )
    .fetch_optional(pool)
    .await?
    else {
        return Ok(());
    };
    let known = sqlx::query_scalar!(
        "SELECT number FROM pull_request WHERE repository_id = $1 AND issue_id = $2",
        repository.id,
        issue_id,
    )
    .fetch_all(pool)
    .await?;
    let names_issue = |pr: &PullRequestData| {
        linked_issue_id(&repository.issue_prefix, &pr.head.branch, &pr.title).as_deref()
            == Some(issue_id)
    };
    let fresh = match github
        .pull_requests(&repository.owner, &repository.name, &known, names_issue)
        .await
    {
        Ok(fresh) => fresh,
        Err(error) => {
            tracing::warn!(
                issue_id,
                "cannot read pull requests from GitHub, the gates read the recorded ones: {error:#}"
            );
            return Ok(());
        }
    };
    for pr in &fresh {
        // A closed or merged pull request that was never recorded for this issue is history,
        // such as one closed while the server was down. It is not what the agent is working
        // on now, so it is left to its webhook and moves nothing here.
        if pr.state != "open" && !known.contains(&pr.number) {
            continue;
        }
        let merged_by = pr.merged_by.as_ref().map(|account| account.login.as_str());
        let mut tx = pool.begin().await?;
        record_pull_request(&mut tx, workspace_id, repository.id, pr, merged_by, None).await?;
        tx.commit().await?;
    }
    Ok(())
}

/// Summarizes the check runs on a commit onto every pull request whose head it is. A rerun
/// is a new run with the same name, so only the newest run of each name counts.
async fn recompute_checks(
    tx: &mut sqlx::PgConnection,
    repository_id: Uuid,
    head_sha: &str,
) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE pull_request p SET
             checks = CASE WHEN c.total = 0 THEN 'none'
                           WHEN c.failed > 0 THEN 'failing'
                           WHEN c.pending > 0 THEN 'pending'
                           ELSE 'passing' END,
             checks_total = c.total, checks_passed = c.passed
         FROM (SELECT count(*)::int AS total,
                      (count(*) FILTER (WHERE status = 'completed'
                           AND conclusion IN ('success', 'neutral', 'skipped')))::int AS passed,
                      (count(*) FILTER (WHERE status = 'completed'
                           AND coalesce(conclusion, '') NOT IN ('success', 'neutral', 'skipped')))::int AS failed,
                      (count(*) FILTER (WHERE status <> 'completed'))::int AS pending
               FROM (SELECT DISTINCT ON (name) status, conclusion FROM check_run
                     WHERE repository_id = $1 AND head_sha = $2
                     ORDER BY name, id DESC) latest) c
         WHERE p.repository_id = $1 AND p.head_sha = $2",
        repository_id,
        head_sha,
    )
    .execute(&mut *tx)
    .await?;
    Ok(())
}

/// A closed pull request moves its linked issue if the workflow has a transition on that
/// event from the issue's status. Otherwise the event is only evidence. Runs once per close:
/// see `record_pull_request`. `closed_by` is the GitHub login that closed it, when known.
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
    let (_, transition_id) = write_transition(
        tx,
        &actor,
        issue_id,
        status,
        transition.to.as_str(),
        delivery_id,
        workflow.version(),
        &[],
    )
    .await?;
    // The agent working this issue has nothing left to do: its pull request is gone, and the
    // issue is no longer in the status that started the run. Ending the run frees the issue
    // to be claimed again.
    let reason = format!(
        "the issue moved to {} because pull request #{} was {}",
        transition.to,
        pr.number,
        if pr.is_merged() {
            "merged"
        } else {
            "closed without merging"
        }
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
            "Pull request #{} was closed without merging{by}.\n\n{}",
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
