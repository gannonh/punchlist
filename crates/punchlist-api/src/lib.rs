//! Request and response types for the Punchlist API (ADR 0008).
//!
//! The server and the Rust client share these types, and utoipa derives their schemas for
//! the OpenAPI document. Core types that cross the API, such as `Role`, are re-exported
//! with their schemas from `punchlist-core`'s `openapi` feature.

use chrono::{DateTime, Utc};
use punchlist_core::display_name;
pub use punchlist_core::{Evidence, GateResult, PullRequestEvidence, PullRequestState, Role};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

/// `POST /api/issues`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CreateIssue {
    pub title: String,
    /// Markdown.
    #[serde(default)]
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Issue {
    /// The identifier people use, such as `PL-1`.
    pub id: String,
    pub title: String,
    /// Markdown.
    pub body: String,
    /// The workflow's status key, such as `in_progress`.
    pub status: String,
    /// The status as people read it, such as `In Progress`.
    pub status_name: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// `GET /api/issues`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct IssueList {
    pub issues: Vec<Issue>,
    /// The sequence number of the workspace's latest event, read in the same snapshot as
    /// the list (ADR 0006).
    pub last_event_seq: i64,
}

/// `POST /api/issues/{id}/transitions`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct MoveIssue {
    /// The status key to move to, such as `todo`.
    pub to: String,
}

/// A transition that the server accepted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Moved {
    pub issue: Issue,
    pub event: Event,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Actor {
    pub id: Uuid,
    pub name: String,
    pub role: Role,
}

/// One entry in an issue's timeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Event {
    /// Increases per workspace (ADR 0006).
    pub seq: i64,
    pub issue_id: String,
    pub actor: Actor,
    pub created_at: DateTime<Utc>,
    pub detail: EventDetail,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventDetail {
    Transition {
        from: String,
        /// `from` as people read it, such as `In Progress`.
        from_name: String,
        to: String,
        /// `to` as people read it.
        to_name: String,
        /// The content hash of the workflow the transition was checked against.
        workflow_version: String,
        /// The GitHub webhook delivery that caused the move, for moves made on a GitHub
        /// event.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        delivery_id: Option<String>,
    },
    /// A comment on the issue, such as the one a pull request closed without merging
    /// requires.
    Comment {
        /// Markdown.
        body: String,
    },
}

impl EventDetail {
    /// A transition between two status keys, with their display names filled in.
    pub fn transition(from: String, to: String, workflow_version: String) -> EventDetail {
        EventDetail::Transition {
            from_name: display_name(&from),
            to_name: display_name(&to),
            from,
            to,
            workflow_version,
            delivery_id: None,
        }
    }
}

/// `POST /api/runners`, with a person's token. Registers a runner and creates its actor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RegisterRunner {
    /// A name people read, such as `sartre`.
    pub name: String,
    /// Agent keys the runner can start, such as `claude-code`.
    pub agents: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Runner {
    pub id: Uuid,
    pub name: String,
    pub agents: Vec<String>,
    pub registered_at: DateTime<Utc>,
    pub last_heartbeat: DateTime<Utc>,
}

/// The response to `POST /api/runners`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RegisteredRunner {
    pub runner: Runner,
    /// The runner actor's bearer token, for heartbeats, claims, logs and outcomes. Shown
    /// once; only its hash is stored.
    pub token: String,
    /// How long a lease lasts without a heartbeat.
    pub lease_seconds: u32,
}

/// `GET /api/runners`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RunnerList {
    pub runners: Vec<Runner>,
}

/// `POST /api/runs/claim`, with a runner's token.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ClaimRequest {
    /// Wait up to this many seconds (at most 30) for an issue to enter Start before
    /// answering 204.
    #[serde(default)]
    pub wait_seconds: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Repository {
    pub owner: String,
    pub name: String,
    pub default_branch: String,
}

/// A claimed issue: the server moved it from Start to In Progress as the runner and leased
/// the attempt to the runner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Claim {
    pub run_id: Uuid,
    /// The compare-and-set token for logs and the outcome.
    pub attempt: i32,
    /// The agent key from the dispatch rule, such as `claude-code`.
    pub agent: String,
    /// The issue after the move, in In Progress.
    pub issue: Issue,
    pub repository: Repository,
    /// The branch to work on, `feature/<id>-<slug>`.
    pub branch: String,
    pub lease_expires_at: DateTime<Utc>,
    /// The workflow's prompt for the status: `prompts/system.md`, then the status's own.
    pub prompt: String,
    /// The bearer token of the agent actor this run acts as, for the MCP server. Valid
    /// while the run is running.
    pub agent_token: String,
}

/// An attempt a runner is working on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
pub struct HeldAttempt {
    pub run_id: Uuid,
    pub attempt: i32,
}

/// `POST /api/runners/{id}/heartbeat`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Heartbeat {
    /// The attempts the runner is working on. Only these leases are renewed; any other
    /// running attempt of this runner expires and the sweep fails it.
    #[serde(default)]
    pub held: Vec<HeldAttempt>,
}

/// `POST /api/runs/{id}/log`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct AppendLog {
    pub attempt: i32,
    /// The 1-based number, within this attempt, of the first line in `lines`. The lines are
    /// consecutive numbers from here. A resent batch keeps its `first_line`, so the server
    /// stores each numbered line once. Gaps between batches are fine.
    pub first_line: i64,
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RunOutcome {
    Running,
    Succeeded,
    Failed,
}

impl RunOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            RunOutcome::Running => "running",
            RunOutcome::Succeeded => "succeeded",
            RunOutcome::Failed => "failed",
        }
    }

    pub fn parse(s: &str) -> Option<RunOutcome> {
        [
            RunOutcome::Running,
            RunOutcome::Succeeded,
            RunOutcome::Failed,
        ]
        .into_iter()
        .find(|outcome| outcome.as_str() == s)
    }
}

/// `POST /api/runs/{id}/finish`. The outcome is `succeeded` or `failed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct FinishRun {
    pub attempt: i32,
    pub outcome: RunOutcome,
    /// Why the attempt failed. Required when the outcome is `failed`.
    #[serde(default)]
    pub reason: Option<String>,
    pub duration_ms: i64,
    #[serde(default)]
    pub input_tokens: Option<i64>,
    #[serde(default)]
    pub output_tokens: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RunnerRef {
    pub id: Uuid,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct LogLine {
    /// Increases per run, in the order the server received the lines.
    pub seq: i64,
    pub attempt: i32,
    pub line: String,
    pub created_at: DateTime<Utc>,
}

/// A run and its latest attempt. `GET /api/issues/{id}/runs` returns these, newest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Run {
    pub id: Uuid,
    pub issue_id: String,
    pub agent: String,
    /// The agent as people read it, such as `Claude Code`.
    pub agent_name: String,
    pub branch: String,
    pub outcome: RunOutcome,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    /// The latest attempt's number, runner, failure reason, duration and tokens.
    pub attempt: i32,
    pub runner: RunnerRef,
    pub failure_reason: Option<String>,
    pub duration_ms: Option<i64>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    /// The last 20 log lines, oldest first.
    pub log_tail: Vec<LogLine>,
}

/// `GET /api/runs/{id}/log`: every line, oldest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RunLog {
    pub run_id: Uuid,
    pub lines: Vec<LogLine>,
}

/// The check runs on a pull request's head commit, taken together.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChecksState {
    /// No check runs reported yet.
    None,
    /// At least one is still queued or in progress, and none has failed.
    Pending,
    /// Every one completed with success, neutral or skipped.
    Passing,
    /// At least one completed with another conclusion, such as failure or timed_out.
    Failing,
}

impl ChecksState {
    pub fn as_str(self) -> &'static str {
        match self {
            ChecksState::None => "none",
            ChecksState::Pending => "pending",
            ChecksState::Passing => "passing",
            ChecksState::Failing => "failing",
        }
    }

    pub fn parse(s: &str) -> Option<ChecksState> {
        [
            ChecksState::None,
            ChecksState::Pending,
            ChecksState::Passing,
            ChecksState::Failing,
        ]
        .into_iter()
        .find(|state| state.as_str() == s)
    }
}

/// A pull request on the workspace's repository, as recorded from GitHub's events.
/// `GET /api/issues/{id}/pull-requests` and `GET /api/pull-requests/unlinked` return these.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PullRequest {
    pub number: i64,
    pub title: String,
    pub branch: String,
    pub url: String,
    pub author_login: String,
    pub state: PullRequestState,
    /// Meaningful while the state is `open`.
    pub draft: bool,
    /// GitHub's mergeable_state, such as `clean`, `dirty` or `unknown`.
    pub merge_state: String,
    pub checks: ChecksState,
    /// Check runs on the head commit, and how many of them passed.
    pub checks_total: i32,
    pub checks_passed: i32,
    /// Review threads not yet resolved.
    pub open_threads: i32,
    /// The issue the branch or title names; `None` when it is unlinked.
    pub issue_id: Option<String>,
    pub updated_at: DateTime<Utc>,
}

impl PullRequest {
    /// The word people read for the state: `draft` for an open draft, otherwise the state.
    pub fn state_word(&self) -> &'static str {
        match self.state {
            PullRequestState::Open if self.draft => "draft",
            state => state.as_str(),
        }
    }
}

/// `POST /api/issues/{id}/comments`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CreateComment {
    /// Markdown.
    pub body: String,
}

/// `GET /api/workflow`: the workspace's active workflow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct WorkflowInfo {
    /// The content hash recorded on every transition.
    pub version: String,
    /// The default branch commit it was loaded from; `None` for the built-in workflow.
    pub commit_sha: Option<String>,
    pub statuses: Vec<String>,
}

/// The body of every error response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ErrorBody {
    /// A stable code, such as `role_not_allowed`, `gate_failed` or `not_found`.
    pub code: String,
    /// What went wrong. For a refused transition, the rule that refused it.
    pub message: String,
    /// For `gate_failed`: the first gate that failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate: Option<String>,
    /// For `gate_failed`: why that gate failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// For `gate_failed`: what the gates read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<Evidence>,
    /// For `gate_failed`: every gate's result.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gates: Vec<GateResult>,
}

impl ErrorBody {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> ErrorBody {
        ErrorBody {
            code: code.into(),
            message: message.into(),
            gate: None,
            reason: None,
            evidence: None,
            gates: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_detail_is_tagged_by_kind() {
        let detail = EventDetail::transition(
            "in_progress".into(),
            "agent_review".into(),
            "sha256:ab".into(),
        );
        assert_eq!(
            serde_json::to_string(&detail).unwrap(),
            r#"{"kind":"transition","from":"in_progress","from_name":"In Progress","to":"agent_review","to_name":"Agent Review","workflow_version":"sha256:ab"}"#
        );
    }

    #[test]
    fn comment_detail_is_tagged_by_kind() {
        let detail = EventDetail::Comment {
            body: "Closed".into(),
        };
        assert_eq!(
            serde_json::to_string(&detail).unwrap(),
            r#"{"kind":"comment","body":"Closed"}"#
        );
    }

    #[test]
    fn create_issue_body_defaults_to_empty() {
        let request: CreateIssue = serde_json::from_str(r#"{"title":"First"}"#).unwrap();
        assert_eq!(
            request,
            CreateIssue {
                title: "First".into(),
                body: String::new()
            }
        );
    }
}
