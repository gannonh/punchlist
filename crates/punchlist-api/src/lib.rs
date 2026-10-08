//! Request and response types for the Punchlist API (ADR 0008).
//!
//! The server and the Rust client share these types, and utoipa derives their schemas for
//! the OpenAPI document. Core types that cross the API, such as `Role`, are re-exported
//! with their schemas from `punchlist-core`'s `openapi` feature.

use chrono::{DateTime, Utc};
pub use punchlist_core::Role;
use punchlist_core::display_name;
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
}

/// `POST /api/runs/{id}/log`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct AppendLog {
    pub attempt: i32,
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

/// The body of every error response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ErrorBody {
    /// A stable code, such as `role_not_allowed` or `not_found`.
    pub code: String,
    /// What went wrong. For a refused transition, the rule that refused it.
    pub message: String,
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
