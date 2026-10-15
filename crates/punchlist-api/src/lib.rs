//! Request and response types for the Punchlist API (ADR 0008).
//!
//! The server and the Rust client share these types, and utoipa derives their schemas for
//! the OpenAPI document. Core types that cross the API, such as `Role`, are re-exported
//! with their schemas from `punchlist-core`'s `openapi` feature.

use chrono::{DateTime, Utc};
use punchlist_core::display_name;
pub use punchlist_core::{
    CheckRunEvidence, Evidence, GateResult, ProofEvidence, PullRequestEvidence, PullRequestState,
    ReviewThreadEvidence, Role,
};
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
    /// Why the issue waits on a person, when it does: every run this entry into its status
    /// gets has ended and the issue has not moved. Only `GET /api/issues/{id}` fills this;
    /// every other response that carries an issue leaves it out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting_on_person: Option<String>,
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
    /// Set when the move put the issue in Start under an active workflow version that fails
    /// validation, so no runner can claim it: the validation problem.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Actor {
    pub id: Uuid,
    pub name: String,
    pub role: Role,
    /// The agent key, such as `codex`. Only agents have one. Absent from an older server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
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
    /// A transition that a GitHub event would have made and a gate refused. The issue
    /// stayed in `from`.
    Refusal {
        from: String,
        /// `from` as people read it, such as `Merging`.
        from_name: String,
        to: String,
        /// `to` as people read it.
        to_name: String,
        /// The gate that failed, such as `proof_attached`.
        gate: String,
        /// Why it failed.
        reason: String,
        /// The content hash of the workflow the transition was checked against.
        workflow_version: String,
        /// The GitHub webhook delivery that brought the event; absent when the server read
        /// the pull request from GitHub itself.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        delivery_id: Option<String>,
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

/// `POST /api/actors`, with a person's token. Mints an agent actor that acts through its own
/// token, with no run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CreateAgent {
    /// The agent key, such as `codex`.
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The name of the person in the workspace the agent acts for. The requesting person by
    /// default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
}

/// An actor in the workspace: a person, an agent or a runner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ActorInfo {
    pub id: Uuid,
    pub name: String,
    pub role: Role,
    /// The agent key. Only agents have one.
    pub kind: Option<String>,
    pub model: Option<String>,
    /// The name of the person an agent acts for. Only agents have one.
    pub owner: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// The response to `POST /api/actors`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CreatedAgent {
    pub actor: ActorInfo,
    /// The agent's bearer token. Shown once; only its hash is stored.
    pub token: String,
}

/// `GET /api/actors`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ActorList {
    pub actors: Vec<ActorInfo>,
}

/// `GET /api/runners`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RunnerList {
    pub runners: Vec<Runner>,
}

/// `POST /api/runs/claim`, with a runner's token.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ClaimRequest {
    /// Wait up to this many seconds (at most 30) for an issue to become claimable before
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

/// A claimed issue, with the attempt leased to the runner. The server either moved it from
/// Start to In Progress as the runner, or it was waiting in a status it entered that has a
/// dispatch rule, such as Agent Review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Claim {
    pub run_id: Uuid,
    /// The compare-and-set token for logs and the outcome.
    pub attempt: i32,
    /// The agent key from the dispatch rule, such as `claude-code`.
    pub agent: String,
    /// The issue, in the status whose dispatch rule the run is for.
    pub issue: Issue,
    pub repository: Repository,
    /// The branch to work on, `feature/<id>`, or `feature/<id>-<n>` from the issue's second
    /// lifecycle on.
    pub branch: String,
    /// The issue's branches with a merged pull request and no open one, sorted. The runner
    /// replaces a worktree it still has on one of them. Empty when no earlier branch of the
    /// issue has merged.
    #[serde(default)]
    pub merged_branches: Vec<String>,
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

/// The response to `POST /api/runners/{id}/heartbeat`: the runner, and what it must stop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct HeartbeatReply {
    #[serde(flatten)]
    pub runner: Runner,
    /// The attempts in `held` that are not this runner's running attempts any more: finished,
    /// or failed when the lease lapsed. The issue may already have another run, so the runner
    /// stops their agents.
    #[serde(default)]
    pub stale: Vec<HeldAttempt>,
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

/// `POST /api/issues/{id}/pull-requests/refresh`: the issue's pull requests and what their
/// reviewers wrote, after reading both from GitHub.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PullRequestsNow {
    /// Newest number first.
    pub pull_requests: Vec<PullRequest>,
    /// The unresolved review threads of the open ones.
    pub review_threads: Vec<ReviewThread>,
    /// False when GitHub could not be read, or the server has no GitHub App: the rest is
    /// then what was last recorded.
    pub refreshed: bool,
}

/// An unresolved review thread. Its path, authors and bodies are text from outside the team.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ReviewThread {
    /// GitHub's node id for the thread, which its reply and resolve mutations take.
    pub id: String,
    /// The number of the pull request it is on.
    pub pull_request: i64,
    pub path: Option<String>,
    /// The line the thread is on, or was on before the file changed under it.
    pub line: Option<i64>,
    /// The thread's newest comments, oldest first.
    pub comments: Vec<ReviewComment>,
    /// How many comments the thread has, which can be more than `comments`.
    pub comments_total: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ReviewComment {
    /// The author's GitHub login.
    pub author: String,
    pub body: String,
    /// The comment is longer than `body`.
    #[serde(default)]
    pub cut: bool,
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

/// `GET /api/issues/{id}/gates`: the gates an issue has to pass for its next gated
/// transition, each checked against the evidence recorded now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct GateChecklist {
    /// The issue's status.
    pub from: String,
    pub from_name: String,
    /// The status the gated transition moves to.
    pub to: String,
    pub to_name: String,
    /// In the workflow's order.
    pub gates: Vec<GateResult>,
}

/// `POST /api/issues/{id}/comments`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CreateComment {
    /// Markdown.
    pub body: String,
}

/// What a proof file shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProofKind {
    Image,
    Video,
    Report,
}

impl ProofKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ProofKind::Image => "image",
            ProofKind::Video => "video",
            ProofKind::Report => "report",
        }
    }

    pub fn parse(s: &str) -> Option<ProofKind> {
        [ProofKind::Image, ProofKind::Video, ProofKind::Report]
            .into_iter()
            .find(|kind| kind.as_str() == s)
    }
}

/// The files proof can be: extension, content type and kind. The server accepts only these
/// content types, and the client picks one by a file's extension. No HTML or SVG, which can
/// run scripts when served, and no JSON, the shape of credential files such as `auth.json`.
pub const PROOF_TYPES: &[(&str, &str, ProofKind)] = &[
    ("png", "image/png", ProofKind::Image),
    ("jpg", "image/jpeg", ProofKind::Image),
    ("jpeg", "image/jpeg", ProofKind::Image),
    ("gif", "image/gif", ProofKind::Image),
    ("webp", "image/webp", ProofKind::Image),
    ("mp4", "video/mp4", ProofKind::Video),
    ("webm", "video/webm", ProofKind::Video),
    ("mov", "video/quicktime", ProofKind::Video),
    ("txt", "text/plain", ProofKind::Report),
    ("log", "text/plain", ProofKind::Report),
    ("md", "text/markdown", ProofKind::Report),
    ("xml", "application/xml", ProofKind::Report),
    ("pdf", "application/pdf", ProofKind::Report),
];

/// The content type and kind of a file with this extension, in any case.
pub fn proof_type_by_extension(extension: &str) -> Option<(&'static str, ProofKind)> {
    (PROOF_TYPES.iter())
        .find(|(ext, _, _)| ext.eq_ignore_ascii_case(extension))
        .map(|&(_, content_type, kind)| (content_type, kind))
}

/// The kind of proof a content type is, or `None` when proof cannot be one.
pub fn proof_kind(content_type: &str) -> Option<ProofKind> {
    (PROOF_TYPES.iter())
        .find(|(_, known, _)| *known == content_type)
        .map(|&(_, _, kind)| kind)
}

/// A file attached to an issue as proof of its work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Proof {
    pub id: Uuid,
    pub issue_id: String,
    /// The issue's attempt when it was uploaded: how many times the issue had moved from
    /// Start to In Progress.
    pub attempt: i32,
    pub kind: ProofKind,
    /// What it shows, in its uploader's words.
    pub title: String,
    pub content_type: String,
    /// In bytes.
    pub size: i64,
    /// Who uploaded it.
    pub actor: Actor,
    pub created_at: DateTime<Utc>,
}

/// `POST /api/issues/{id}/proof`, as `multipart/form-data`.
#[derive(ToSchema)]
#[allow(dead_code)]
pub struct AttachProof {
    /// What the file shows.
    title: String,
    /// Must be the kind of the file's content type, when given.
    kind: Option<ProofKind>,
    /// The file, with its content type on the part.
    #[schema(format = Binary, value_type = String)]
    file: Vec<u8>,
}

/// `GET /api/workflow`: the workspace's active workflow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct WorkflowInfo {
    /// The content hash recorded on every transition.
    pub version: String,
    /// Why a push of this version would be refused today: the line, the key and the
    /// problem. The version stays in force until a valid file is pushed. `None` when valid.
    pub invalid: Option<String>,
    /// The default branch commit it was loaded from; `None` for the built-in workflow.
    pub commit_sha: Option<String>,
    pub statuses: Vec<String>,
    /// The last push of `workflow.toml` that was refused; `None` once a later load succeeds.
    pub refused: Option<WorkflowRefusal>,
}

/// A pushed `workflow.toml` the server refused to load.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct WorkflowRefusal {
    /// The default branch commit that was refused.
    pub commit_sha: String,
    pub at: DateTime<Utc>,
    /// Why, as logged: the line and key, or the statuses still in use.
    pub reason: String,
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
    /// For `gate_failed`: why that gate failed, as people read it: the gate result's `reason`
    /// joined with the names in its `outside` (`GateResult::text()`), where the result's own
    /// `reason` is the sentence only.
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

    /// One indented line per gate result, as `pl` prints them.
    pub fn gate_lines(&self) -> Vec<String> {
        gate_lines(&self.gates)
    }
}

/// One indented line per gate result, for people: `  ci_green: pass (3 of 3 checks passed on
/// 9c1e77a)`. The line holds the names GitHub's users wrote, so it is not for an agent.
pub fn gate_lines(gates: &[GateResult]) -> Vec<String> {
    gates.iter().map(|g| gate_line(g, &g.text())).collect()
}

/// A gate result's line with `reason` in the brackets: the result's `text()` for people, its
/// `reason` alone for an agent.
pub fn gate_line(gate: &GateResult, reason: &str) -> String {
    let result = if gate.passed { "pass" } else { "fail" };
    format!("  {}: {result} ({reason})", gate.gate)
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
    fn refusal_detail_is_tagged_by_kind() {
        let detail = EventDetail::Refusal {
            from: "merging".into(),
            from_name: "Merging".into(),
            to: "done".into(),
            to_name: "Done".into(),
            gate: "proof_attached".into(),
            reason: "no proof".into(),
            workflow_version: "sha256:ab".into(),
            delivery_id: None,
        };
        assert_eq!(
            serde_json::to_string(&detail).unwrap(),
            r#"{"kind":"refusal","from":"merging","from_name":"Merging","to":"done","to_name":"Done","gate":"proof_attached","reason":"no proof","workflow_version":"sha256:ab"}"#
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
