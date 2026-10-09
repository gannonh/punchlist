//! `POST /api/github/webhook`: GitHub's deliveries, verified, parsed and queued as jobs,
//! and the job that records each one as evidence on pull requests and issues.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use chrono::{DateTime, Utc};
use hmac::{Hmac, KeyInit, Mac};
use punchlist_api::ErrorBody;
use punchlist_core::{Role, default_workflow, linked_issue_id};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use crate::auth::Actor;
use crate::issues::write_transition;
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
const CHECK_SUITE_ACTIONS: [&str; 2] = ["completed", "requested"];
const REVIEW_THREAD_ACTIONS: [&str; 2] = ["resolved", "unresolved"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Repository {
    pub name: String,
    pub owner: Account,
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
    #[serde(default)]
    pub mergeable_state: Option<String>,
    pub head: Head,
    pub updated_at: DateTime<Utc>,
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
pub(crate) struct CheckSuiteData {
    pub head_sha: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct CheckSuiteEvent {
    pub action: String,
    pub check_suite: CheckSuiteData,
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

/// A delivery that passed validation, as queued.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "event", content = "payload", rename_all = "snake_case")]
pub(crate) enum GithubEvent {
    PullRequest(PullRequestEvent),
    CheckRun(CheckRunEvent),
    CheckSuite(CheckSuiteEvent),
    PullRequestReviewThread(ReviewThreadEvent),
}

impl GithubEvent {
    fn repository(&self) -> &Repository {
        match self {
            GithubEvent::PullRequest(e) => &e.repository,
            GithubEvent::CheckRun(e) => &e.repository,
            GithubEvent::CheckSuite(e) => &e.repository,
            GithubEvent::PullRequestReviewThread(e) => &e.repository,
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
    let handled: &[&str] = match event {
        "pull_request" => &PULL_REQUEST_ACTIONS,
        "check_run" => &CHECK_RUN_ACTIONS,
        "check_suite" => &CHECK_SUITE_ACTIONS,
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
        "check_suite" => GithubEvent::CheckSuite(typed(body)?),
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
         ON CONFLICT (idempotency_key) DO NOTHING",
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
            let (_, applied, linked) = upsert_pull_request(tx, &job, &event.pull_request).await?;
            if let (true, true, Some(issue_id)) = (applied, event.action == "closed", linked) {
                close_issue_on_pull_request(tx, &job, event, &issue_id).await?;
            }
        }
        GithubEvent::CheckRun(event) => {
            let run = &event.check_run;
            sqlx::query!(
                "INSERT INTO check_run (id, repository_id, head_sha, name, status, conclusion)
                 VALUES ($1, $2, $3, $4, $5, $6)
                 ON CONFLICT (id) DO UPDATE SET head_sha = EXCLUDED.head_sha,
                     name = EXCLUDED.name, status = EXCLUDED.status,
                     conclusion = EXCLUDED.conclusion, updated_at = now()",
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
        GithubEvent::CheckSuite(event) => {
            recompute_checks(tx, job.repository_id, &event.check_suite.head_sha).await?;
        }
        GithubEvent::PullRequestReviewThread(event) => {
            let (pull_request_id, _, _) =
                upsert_pull_request(tx, &job, &event.pull_request).await?;
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

/// Records a pull request from a payload, unless a newer payload was already applied.
/// Returns its row id, whether this payload was applied, and the issue it is linked to.
async fn upsert_pull_request(
    tx: &mut sqlx::PgConnection,
    job: &GithubJob,
    pr: &PullRequestData,
) -> anyhow::Result<(Uuid, bool, Option<String>)> {
    let prefix = sqlx::query_scalar!(
        "SELECT issue_prefix FROM workspace WHERE id = $1",
        job.workspace_id
    )
    .fetch_one(&mut *tx)
    .await?;
    // A name that matches no issue in the workspace leaves the pull request unlinked.
    let linked = match linked_issue_id(&prefix, &pr.head.branch, &pr.title) {
        Some(id) => {
            sqlx::query_scalar!(
                "SELECT id FROM issue WHERE id = $1 AND workspace_id = $2",
                id,
                job.workspace_id
            )
            .fetch_optional(&mut *tx)
            .await?
        }
        None => None,
    };
    let state = if pr.merged {
        "merged"
    } else if pr.state == "closed" {
        "closed"
    } else {
        "open"
    };
    let merge_state = pr.mergeable_state.as_deref().unwrap_or("unknown");
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
         RETURNING id",
        job.repository_id,
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
        // A payload older than the stored state changes nothing.
        let id = sqlx::query_scalar!(
            "SELECT id FROM pull_request WHERE repository_id = $1 AND number = $2",
            job.repository_id,
            pr.number
        )
        .fetch_one(&mut *tx)
        .await?;
        return Ok((id, false, linked));
    };
    recompute_checks(tx, job.repository_id, &pr.head.sha).await?;
    Ok((id, true, linked))
}

/// Summarizes the check runs on a commit onto every pull request whose head it is.
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
               FROM check_run WHERE repository_id = $1 AND head_sha = $2) c
         WHERE p.repository_id = $1 AND p.head_sha = $2",
        repository_id,
        head_sha,
    )
    .execute(&mut *tx)
    .await?;
    Ok(())
}

/// A closed pull request moves its linked issue if the workflow has a transition on that
/// event from the issue's status. Otherwise the event is only evidence. Redelivery finds
/// the issue already moved and changes nothing.
async fn close_issue_on_pull_request(
    tx: &mut sqlx::PgConnection,
    job: &GithubJob,
    event: &PullRequestEvent,
    issue_id: &str,
) -> anyhow::Result<()> {
    let pr = &event.pull_request;
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
    let workflow = default_workflow();
    let Some(transition) = workflow.transition_on(&status, name) else {
        tracing::info!(
            issue_id,
            status,
            event = name,
            "no transition on this event"
        );
        return Ok(());
    };
    let login = &event.sender.login;
    let actor_id = sqlx::query_scalar!(
        "INSERT INTO actor (workspace_id, name, role, github_login)
         VALUES ($1, $2, 'github', $2)
         ON CONFLICT (workspace_id, github_login) WHERE github_login IS NOT NULL
         DO UPDATE SET name = EXCLUDED.name
         RETURNING id",
        job.workspace_id,
        login,
    )
    .fetch_one(&mut *tx)
    .await?;
    let actor = Actor {
        id: actor_id,
        workspace_id: job.workspace_id,
        name: login.clone(),
        role: Role::Github,
    };
    let (_, transition_id) = write_transition(
        tx,
        &actor,
        issue_id,
        status,
        transition.to.as_str(),
        Some(&job.delivery_id),
    )
    .await?;
    if transition.require.iter().any(|r| r == "comment") {
        let body = format!(
            "Pull request #{} was closed without merging by @{login}.\n\n{}",
            pr.number, pr.html_url
        );
        let comment_id = sqlx::query_scalar!(
            "INSERT INTO comment (issue_id, actor_id, body, transition_id)
             VALUES ($1, $2, $3, $4) RETURNING id",
            issue_id,
            actor_id,
            body,
            transition_id,
        )
        .fetch_one(&mut *tx)
        .await?;
        let seq = sqlx::query_scalar!(
            "UPDATE workspace SET last_event_seq = last_event_seq + 1 WHERE id = $1
             RETURNING last_event_seq",
            job.workspace_id,
        )
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query!(
            "INSERT INTO event (workspace_id, seq, issue_id, kind, actor_id, comment_id)
             VALUES ($1, $2, $3, 'comment', $4, $5)",
            job.workspace_id,
            seq,
            issue_id,
            actor_id,
            comment_id,
        )
        .execute(&mut *tx)
        .await?;
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
    fn check_suite_completed_parses() {
        let GithubEvent::CheckSuite(e) = fixture(
            "check_suite",
            include_str!("../tests/fixtures/github/check_suite_completed.json"),
        ) else {
            panic!("not a check_suite event");
        };
        assert_eq!(e.check_suite.head_sha.len(), 40);
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
