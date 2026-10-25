//! Request and response types for the Punchlist API (ADR 0008).
//!
//! The server and the Rust client share these types, and utoipa derives their schemas for
//! the OpenAPI document. Core types that cross the API, such as `Role`, are re-exported
//! with their schemas from `punchlist-core`'s `openapi` feature.

use chrono::{DateTime, Utc};
pub use punchlist_core::{
    CheckRunEvidence, Evidence, GateResult, ProofEvidence, ProofKind, PullRequestEvidence,
    PullRequestState, ReviewThreadEvidence, Role,
};
use punchlist_core::{display_name, is_plain_name};
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
    /// A person or an agent; `None` when nobody is assigned.
    #[serde(default)]
    pub assignee: Option<Actor>,
    /// In the order they were set.
    #[serde(default)]
    pub labels: Vec<String>,
    /// The parent issue's id, such as `PL-1`; `None` when the issue has none.
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub project: Option<ProjectRef>,
    /// One of `project`'s milestones.
    #[serde(default)]
    pub milestone: Option<MilestoneRef>,
    /// The issues that block this one, lowest number first.
    #[serde(default)]
    pub blocked_by: Vec<LinkedIssue>,
    /// The issues this one blocks, lowest number first: `blocked_by` in reverse.
    #[serde(default)]
    pub blocks: Vec<LinkedIssue>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Why the issue waits on a person, when it does: every run this entry into its status
    /// gets has ended and the issue has not moved. Only `GET /api/issues/{id}` fills this;
    /// every other response that carries an issue leaves it out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting_on_person: Option<String>,
    /// That the issue's next run waits for its agent's usage limit to reset, and when:
    /// `waiting for the Claude Code usage limit to reset at 2026-10-23 22:00:00 UTC`. Only
    /// `GET /api/issues/{id}` fills this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting_for_usage_limit: Option<String>,
    /// The Linear issue it was imported from; `None` for an issue made in Punchlist.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linear: Option<LinearLink>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct LinearLink {
    /// Such as `KAT-12`.
    pub identifier: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ProjectRef {
    pub id: Uuid,
    pub name: String,
}

/// An issue another one names, with its status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct LinkedIssue {
    pub id: String,
    pub status: String,
    pub status_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct MilestoneRef {
    pub id: Uuid,
    pub name: String,
}

/// An issue as the list and the board show it: the issue, its run, its newest pull request
/// and its next gated transition's gates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct IssueRow {
    pub issue: Issue,
    pub run: RunSummary,
    /// The issue's highest-numbered pull request; `None` when none is linked.
    pub pull_request: Option<PullRequestSummary>,
    /// `None` when no transition an actor can request from the issue's status has gates.
    pub gates: Option<GateSummary>,
}

/// What the issue's agent is doing in its status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RunSummary {
    /// No run is running or waiting, and the last one in this status did not fail.
    Idle,
    /// A dispatch waits for a runner that can run `agent`.
    Queued {
        agent: String,
    },
    Running {
        agent: String,
        since: DateTime<Utc>,
    },
    /// The latest run in the issue's status failed.
    Failed {
        agent: String,
        /// The last attempt's failure reason.
        reason: Option<String>,
        at: DateTime<Utc>,
    },
}

/// A pull request's state as people read it: `draft` is an open draft.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PullRequestStatus {
    Draft,
    Open,
    Merged,
    Closed,
}

impl PullRequestStatus {
    pub fn new(state: PullRequestState, draft: bool) -> PullRequestStatus {
        match state {
            PullRequestState::Open if draft => PullRequestStatus::Draft,
            PullRequestState::Open => PullRequestStatus::Open,
            PullRequestState::Merged => PullRequestStatus::Merged,
            PullRequestState::Closed => PullRequestStatus::Closed,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PullRequestSummary {
    pub number: i64,
    pub state: PullRequestStatus,
    pub checks: ChecksState,
    /// `None` unless it is a plain `https://github.com/<owner>/<name>/pull/<number>` URL.
    pub url: Option<String>,
}

/// The gates of the issue's next gated transition, over the evidence recorded now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct GateSummary {
    /// The status the gated transition moves to.
    pub to: String,
    pub to_name: String,
    pub passed: u32,
    pub total: u32,
    /// The first gate that fails, in the workflow's order.
    pub first_failed: Option<String>,
}

/// The answer to a write: the changed issue's row and the sequence number of the event it
/// recorded, or the workspace's latest when it recorded none (ADR 0006). The client drops
/// that event when the stream delivers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct IssueWritten {
    pub row: IssueRow,
    pub seq: i64,
}

/// `PATCH /api/issues/{id}`. An absent field is left as it is.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct UpdateIssue {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub title: Option<String>,
    /// Markdown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub body: Option<String>,
    /// Each trimmed, 1 to 50 characters; at most 20, duplicates dropped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub labels: Option<Vec<String>>,
    /// A person's or an agent's actor id, or `null` to unassign.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schema(value_type = Option<Uuid>, nullable = true)]
    pub assignee: Option<Option<Uuid>>,
    /// An issue id of the workspace, such as `PL-1`, or `null` for none. Neither the issue
    /// itself nor one of the issues under it.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schema(value_type = Option<String>, nullable = true)]
    pub parent: Option<Option<String>>,
    /// A project's id, or `null` for none. A change of project clears the milestone, unless
    /// the same request sets one.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schema(value_type = Option<Uuid>, nullable = true)]
    pub project: Option<Option<Uuid>>,
    /// A milestone of the project the issue has after the edit, or `null` for none.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schema(value_type = Option<Uuid>, nullable = true)]
    pub milestone: Option<Option<Uuid>>,
    /// The issue ids of the workspace that block the issue, replacing the ones it has; `[]`
    /// clears them. Not the issue itself, and none that the issue already blocks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub blocked_by: Option<Vec<String>>,
}

/// A field that is present, `null` or not, as `Some`; serde's default leaves an absent one
/// `None`.
fn present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

/// One message of `GET /api/workspaces/{id}/events`: an event and its issue's row as it is
/// now, which can be newer than the event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct StreamItem {
    pub event: Event,
    pub row: IssueRow,
}

/// `POST /api/session`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SignIn {
    /// A person's bearer token.
    pub token: String,
}

/// Who a request is signed in as: for a person, their account and its person actor in the
/// workspace the request names, else in the account's oldest workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Session {
    pub actor: Actor,
    pub workspace_id: Uuid,
    /// The account a person's token authenticates (ADR 0011). Agents and runners have none,
    /// and an older server sends none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<Account>,
}

/// The identity above a person's actors, one per workspace they belong to (ADR 0011).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Account {
    pub id: Uuid,
    pub name: String,
}

/// One of `GET /api/workspaces`: a workspace the account belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Workspace {
    pub id: Uuid,
    pub name: String,
    /// Its issues' ids are this and a number: `PL` gives PL-1.
    pub issue_prefix: String,
    /// The workspace's GitHub repository, as `owner/name`.
    pub repository: String,
}

/// A pull request's URL as GitHub sent it, when it is `https://github.com/<owner>/<name>/pull/<number>`
/// for this pull request, in the plain alphabet; otherwise `None`, since it would sit outside
/// the fence.
pub fn plain_pull_request_url(number: i64, url: &str) -> Option<&str> {
    let path = url.strip_prefix("https://github.com/")?;
    let mut parts = path.split('/');
    // `.` and `..` are plain names and not an owner or a repository.
    let plain =
        |part: Option<&str>| part.is_some_and(|p| is_plain_name(p) && p != "." && p != "..");
    (plain(parts.next())
        && plain(parts.next())
        && parts.next() == Some("pull")
        && parts.next() == Some(&number.to_string())
        && parts.next().is_none())
    .then_some(url)
}

/// A review comment's URL as GitHub sent it, when it is
/// `https://github.com/<owner>/<name>/pull/<number>#discussion_r<digits>`; otherwise `None`.
pub fn plain_review_thread_url(url: &str) -> Option<&str> {
    let (pull_request, comment) = url.split_once("#discussion_r")?;
    let number = pull_request.rsplit('/').next()?.parse().ok()?;
    plain_pull_request_url(number, pull_request)?;
    (!comment.is_empty() && comment.bytes().all(|b| b.is_ascii_digit())).then_some(url)
}

/// `GET /api/issues`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct IssueList {
    /// Oldest first.
    pub issues: Vec<IssueRow>,
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
    Created,
    Edited {
        /// Among `title`, `body`, `labels`, `assignee`, `parent`, `project` and `milestone`, in
        /// that order.
        fields: Vec<String>,
    },
    /// A run of the issue was claimed or ended, or lost its attempt. This kind, `pull_request`,
    /// `proof` and `workflow` say only that the issue's row changed: the event stream carries
    /// them, and an issue's timeline leaves them out.
    Run,
    /// GitHub evidence on the issue's pull requests was recorded: a pull request, the check
    /// runs on its head commit, or its review threads.
    PullRequest,
    /// Proof was attached to the issue.
    Proof,
    /// The workspace loaded a different workflow version, which can change the gates and status
    /// names every row shows.
    Workflow,
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

/// `POST /api/projects`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CreateProject {
    /// Trimmed, 1 to 100 characters, unique in the workspace.
    pub name: String,
}

/// `POST /api/projects/{id}/milestones`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CreateMilestone {
    /// Trimmed, 1 to 100 characters, unique in the project.
    pub name: String,
    /// What must be true for the milestone to pass.
    #[serde(default)]
    pub pass_when: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Project {
    pub id: Uuid,
    pub name: String,
    /// Oldest first.
    pub milestones: Vec<Milestone>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Milestone {
    pub id: Uuid,
    pub name: String,
    /// What must be true for the milestone to pass; empty when nobody wrote it.
    pub pass_when: String,
}

/// `GET /api/projects`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ProjectList {
    /// Oldest first.
    pub projects: Vec<Project>,
}

/// `POST /api/workspaces/{id}/imports/linear`: one Linear project as `pl import linear` read
/// it. Every id is Linear's, and a rerun finds what it imported before by them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct LinearImport {
    /// The team's key, such as `KAT`.
    pub team: String,
    pub project: LinearProject,
    pub issues: Vec<LinearIssue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct LinearProject {
    pub id: String,
    pub name: String,
    pub url: String,
    pub milestones: Vec<LinearMilestone>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct LinearMilestone {
    pub id: String,
    pub name: String,
    /// Imported as the milestone's PASS when text.
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct LinearIssue {
    pub id: String,
    /// Such as `KAT-12`.
    pub identifier: String,
    pub url: String,
    pub title: String,
    /// Markdown.
    #[serde(default)]
    pub description: Option<String>,
    pub state: LinearState,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub milestone_id: Option<String>,
    #[serde(default)]
    pub creator: Option<LinearUser>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub comments: Vec<LinearComment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct LinearState {
    pub name: String,
    /// Linear's state type: `triage`, `backlog`, `unstarted`, `started`, `completed`,
    /// `canceled` or `duplicate`.
    #[serde(rename = "type")]
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct LinearUser {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct LinearComment {
    pub id: String,
    pub body: String,
    /// `None` for a comment an integration wrote.
    #[serde(default)]
    pub user: Option<LinearUser>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImportState {
    Queued,
    Running,
    Done,
    Failed,
}

impl ImportState {
    pub fn as_str(self) -> &'static str {
        match self {
            ImportState::Queued => "queued",
            ImportState::Running => "running",
            ImportState::Done => "done",
            ImportState::Failed => "failed",
        }
    }

    pub fn parse(s: &str) -> Option<ImportState> {
        [
            ImportState::Queued,
            ImportState::Running,
            ImportState::Done,
            ImportState::Failed,
        ]
        .into_iter()
        .find(|state| state.as_str() == s)
    }
}

/// One import, as the server's job runs it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ImportRun {
    /// The job's id.
    pub id: i64,
    pub state: ImportState,
    /// Why the last attempt failed; a `queued` import with one is retried.
    pub error: Option<String>,
    /// Set once it is `done`.
    pub report: Option<ImportReport>,
    pub created_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

/// What one import wrote.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ImportReport {
    pub created: ImportCounts,
    pub updated: ImportCounts,
    /// Issues gone from Linear's project, parents outside it, empty comments skipped.
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ImportCounts {
    pub issues: i64,
    pub comments: i64,
    pub projects: i64,
    pub milestones: i64,
    pub people: i64,
}

/// `GET /api/workspaces/{id}/imports`: the workspace's rows imported from Linear, and the
/// latest import.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ImportStatus {
    pub totals: ImportCounts,
    pub last_run: Option<ImportRun>,
}

/// `POST /api/runners`, with a person's token. Registers a runner and creates its actor, or
/// re-adopts the runner this person registered under the same name, with a new token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RegisterRunner {
    /// A name people read, such as `sartre`.
    pub name: String,
    /// Agent keys the runner can start, such as `claude-code`.
    pub agents: Vec<String>,
    /// The model the runner passes each agent, by agent key, such as `claude-code` →
    /// `haiku`. An agent missing here runs its own default model.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub models: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Runner {
    pub id: Uuid,
    pub name: String,
    pub agents: Vec<String>,
    pub registered_at: DateTime<Utc>,
    pub last_heartbeat: DateTime<Utc>,
    /// The agent kinds the runner claims no work for until their usage limit resets.
    #[serde(default)]
    pub paused: Vec<AgentPause>,
}

/// An agent kind a runner claims nothing for until `until`: its last attempt hit the
/// agent's usage limit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct AgentPause {
    pub agent: String,
    pub until: DateTime<Utc>,
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
    /// What the timeline calls the agent, 1 to 100 characters. The kind's name by default,
    /// such as `Codex`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
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

/// `GET /api/search`: hits in the id or title first, then the rest, each group newest issue
/// first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SearchResults {
    pub hits: Vec<SearchHit>,
    /// More issues match than `limit`.
    pub more: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SearchHit {
    pub issue_id: String,
    pub title: String,
    pub status_name: String,
    /// The query matches the issue's id or title.
    pub in_title: bool,
    /// Where the query matches the body, or else the newest comment it matches; `None` when
    /// it matches neither.
    pub snippet: Option<Snippet>,
}

/// One line around the first match, with whitespace runs and control characters made one
/// space. `before` starts and `after` ends with `…` where the text was cut.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Snippet {
    pub field: SnippetField,
    pub before: String,
    /// The matched text as the document writes it. Empty when the server could not find the
    /// match again, and `before` is then the text's first 120 characters.
    pub hit: String,
    pub after: String,
    /// The `seq` of the comment's `comment` event; `None` for the body.
    pub comment_seq: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SnippetField {
    Body,
    Comment,
}

impl SnippetField {
    pub fn as_str(self) -> &'static str {
        match self {
            SnippetField::Body => "body",
            SnippetField::Comment => "comment",
        }
    }
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
    /// while the run is running, until the next claim or resume of the run replaces it.
    pub agent_token: String,
    /// The attempt's agent session: `claude --session-id` on a fresh attempt, `--resume`
    /// on a resumed one.
    pub session_id: Uuid,
    /// The number of the attempt's next log line: 1 on a fresh attempt, one past the last
    /// stored line on a resumed one.
    pub next_line: i64,
    /// The token the run acts on GitHub with, as the workspace's GitHub App. `None` when the
    /// server has no App credentials, or could not mint one (`github_error`).
    #[serde(default)]
    pub github: Option<GithubToken>,
    /// Why the server could not mint the run's token, such as GitHub's answer. The runner
    /// fails the attempt with it.
    #[serde(default)]
    pub github_error: Option<String>,
}

/// An installation token of the workspace's GitHub App, for its repository alone, and the
/// App's bot account, which a run's commits are authored by.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct GithubToken {
    pub token: String,
    /// The runner asks for a fresh one before then (`POST /api/runs/{id}/github-token`).
    pub expires_at: DateTime<Utc>,
    /// `<app slug>[bot]`.
    pub bot_name: String,
    /// `<bot user id>+<app slug>[bot]@users.noreply.github.com`.
    pub bot_email: String,
}

/// `GET /api/runners/{id}/leases`: a running attempt the runner holds whose lease has not
/// lapsed. A runner that restarts resumes or fails each one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Lease {
    pub run_id: Uuid,
    pub attempt: i32,
    pub agent: String,
    pub issue_id: String,
    pub branch: String,
    /// Where the issue's worktree is cloned from.
    pub repository: Repository,
    pub lease_expires_at: DateTime<Utc>,
}

/// `POST /api/runs/{id}/resume`: the attempt the runner still holds, to run again in the
/// same agent session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ResumeRun {
    pub attempt: i32,
}

/// `POST /api/runs/{id}/github-token`: the runner's running attempt, whose token to renew.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RefreshGithubToken {
    pub attempt: i32,
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
    /// The cost in US dollars the agent reported, as a decimal such as `0.0453`, every digit
    /// kept. `None` when it reported none.
    #[serde(default)]
    pub cost_usd: Option<String>,
    /// The runner lost the attempt before its agent finished, such as when it stopped or
    /// restarted. Only with `failed`. A lost attempt that is not the run's last allowed one
    /// keeps the run running for another attempt.
    #[serde(default)]
    pub lost: bool,
    /// The agent stopped on its account's usage limit. Only with `failed`. The run does not
    /// count toward the runs its entry gets, and the issue's next run, like the runner's
    /// claims for that agent, waits for the reset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_limit: Option<UsageLimit>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct UsageLimit {
    /// When the limit resets, as the agent reported it; 15 minutes from the finish when
    /// `None`.
    #[serde(default)]
    pub resets_at: Option<DateTime<Utc>>,
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
    /// The latest attempt's number, runner, failure reason, duration, tokens and cost.
    pub attempt: i32,
    /// The model the latest attempt's runner passed the agent; `None` for the agent's default.
    #[serde(default)]
    pub model: Option<String>,
    /// The model the latest attempt's agent said it runs on; `None` until it says.
    #[serde(default)]
    pub reported_model: Option<String>,
    pub runner: RunnerRef,
    pub failure_reason: Option<String>,
    pub duration_ms: Option<i64>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    #[serde(default)]
    pub cost_usd: Option<String>,
    /// The last 20 log lines, oldest first.
    pub log_tail: Vec<LogLine>,
    /// Every attempt, oldest first.
    #[serde(default)]
    pub attempts: Vec<AttemptInfo>,
}

/// One attempt of a run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct AttemptInfo {
    pub attempt: i32,
    pub runner: RunnerRef,
    /// The model the runner passed the agent; `None` for the agent's default.
    #[serde(default)]
    pub model: Option<String>,
    /// The model the agent said it runs on, such as `claude-haiku-5-5` for `haiku`; `None`
    /// until it says.
    #[serde(default)]
    pub reported_model: Option<String>,
    #[serde(default)]
    pub duration_ms: Option<i64>,
    #[serde(default)]
    pub input_tokens: Option<i64>,
    #[serde(default)]
    pub output_tokens: Option<i64>,
    /// The cost in US dollars the agent reported, as a decimal string; `None` when it
    /// reported none.
    #[serde(default)]
    pub cost_usd: Option<String>,
    pub state: RunOutcome,
    pub failure_reason: Option<String>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
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
    /// The newest check run of each name on the head commit, as `ci_green` reads them. Only
    /// an issue's pull requests carry them; their names are text from outside the team.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub check_runs: Vec<CheckRunEvidence>,
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

/// `GET /api/pull-requests`: an open or draft pull request of the workspace's repository, as
/// the Pull requests page shows it. Its title and branch are text from outside the team.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct OpenPullRequest {
    pub number: i64,
    pub title: String,
    pub branch: String,
    /// `None` unless it is a plain `https://github.com/<owner>/<name>/pull/<number>` URL.
    pub url: Option<String>,
    pub draft: bool,
    /// GitHub's mergeable_state, such as `clean`, `dirty` or `unknown`.
    pub merge_state: String,
    pub checks_passed: i32,
    pub checks_total: i32,
    pub open_threads: i32,
    pub updated_at: DateTime<Utc>,
    /// The issue it names; `None` when it names none.
    pub issue: Option<PullRequestIssue>,
    /// Open, not a draft, and its issue is in a status the workflow moves on `pr_merged`.
    pub waits_on_merge: bool,
}

/// The issue a pull request or an inbox item names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PullRequestIssue {
    pub id: String,
    pub title: String,
    pub status: String,
    pub status_name: String,
    /// As in `StatusInfo`: the status, as people read it, a merge moves the issue to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_merge_to: Option<String>,
}

/// `GET /api/inbox`: what waits on the caller, newest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct InboxList {
    pub items: Vec<InboxItem>,
    /// How many of `items` are unread.
    pub unread: i64,
}

/// An inbox item, derived from the issue's state: it leaves when its condition ends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InboxItem {
    /// The issue is in a status only a person moves it on from (`Workflow::person_statuses`).
    Waiting {
        issue: PullRequestIssue,
        /// When the event that put the issue in its status was written.
        since: DateTime<Utc>,
        /// That event's sequence number: the issue's newest `transition` or `created` event.
        since_seq: i64,
        /// The caller has not opened the item since `since_seq`.
        unread: bool,
    },
    /// The issue's latest run in its status ended and nothing dispatches it again: the run
    /// failed, or it was the entry's last (`limit_reached`). The caller owns the run's agent.
    Run {
        issue: PullRequestIssue,
        /// The run's agent kind, such as `claude-code`.
        agent: String,
        /// The run's number in this entry into the status.
        runs: i32,
        /// `failed` or `succeeded`.
        last_outcome: RunOutcome,
        /// The last attempt's failure reason, when the run failed. Text from outside the team.
        reason: Option<String>,
        /// The entry's runs are used up: `<runs> runs in <status> ended without moving it`.
        limit_reached: bool,
        /// When the run's end was recorded.
        since: DateTime<Utc>,
        /// That event's sequence number: the issue's newest `run` event.
        since_seq: i64,
        /// The caller has not opened the item since `since_seq`.
        unread: bool,
    },
}

/// The kinds of inbox item, as `POST /api/inbox/read` names them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum InboxKind {
    Waiting,
    Run,
}

impl InboxKind {
    pub fn as_str(self) -> &'static str {
        match self {
            InboxKind::Waiting => "waiting",
            InboxKind::Run => "run",
        }
    }
}

/// `POST /api/inbox/read`: the caller opened the item for `issue` up to `since_seq`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct MarkInboxRead {
    pub issue: String,
    pub kind: InboxKind,
    pub since_seq: i64,
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
    /// The oldest recorded comment on GitHub; `None` unless it is a plain
    /// `https://github.com/<owner>/<name>/pull/<number>#discussion_r<id>` URL.
    #[serde(default)]
    pub url: Option<String>,
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

/// `GET /api/issues/{id}/transitions`: the moves that can be requested from the issue's
/// status, each with the verdict the caller's request would get on the evidence recorded now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct TransitionList {
    /// The issue's status; `locked_to` are the roles it locks out.
    pub status: StatusInfo,
    /// The status locks the caller's role out: it can move, comment on and edit nothing here.
    pub viewer_locked: bool,
    /// Each transition from `status` that is not made only by an event, in the workflow's order.
    pub transitions: Vec<TransitionOption>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct TransitionOption {
    pub to: StatusInfo,
    /// The roles that make it.
    pub by: Vec<Role>,
    pub verdict: Verdict,
}

/// What `POST /api/issues/{id}/transitions` would answer the caller, without the read of
/// GitHub a gated request makes first, so a request can still answer differently.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Verdict {
    /// The issue would move.
    Allowed,
    /// The first gate that fails (`gate_failed`).
    Blocked(GateResult),
    /// The workflow refuses the caller: `code` is the refusal's, such as `locked` or
    /// `role_not_allowed`, and `message` names the rule.
    Refused { code: String, message: String },
}

/// `POST /api/issues/{id}/comments`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CreateComment {
    /// Markdown.
    pub body: String,
}

/// `POST /api/runs/{id}/waiting`: the run cannot go on without a person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct WaitForPerson {
    /// What the person must do, 1 to 500 characters.
    pub reason: String,
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
    /// In the workflow's order.
    pub statuses: Vec<StatusInfo>,
    /// The last push of `workflow.toml` that was refused; `None` once a later load succeeds.
    pub refused: Option<WorkflowRefusal>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct StatusInfo {
    /// The status key, such as `in_progress`.
    pub key: String,
    /// As people read it, such as `In Progress`.
    pub name: String,
    /// The roles that may not act on an issue in it, from the workflow's `[lock]`.
    pub locked_to: Vec<Role>,
    /// The agent a runner starts when an issue enters it, such as `claude-code`.
    pub dispatch: Option<String>,
    /// No transition leaves it, as with Done and Canceled. The list hides these by default.
    pub closed: bool,
    /// The status, as people read it (`Done`), that a merge of the issue's pull request
    /// moves an issue in it to; `None` when it does not wait on a merge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_merge_to: Option<String>,
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
    fn proof_types_are_found_by_extension_in_any_case_and_by_content_type() {
        assert_eq!(
            proof_type_by_extension("JPEG"),
            Some(("image/jpeg", ProofKind::Image))
        );
        assert_eq!(
            proof_type_by_extension("log"),
            Some(("text/plain", ProofKind::Report))
        );
        assert_eq!(proof_type_by_extension("html"), None);
        assert_eq!(proof_kind("video/quicktime"), Some(ProofKind::Video));
        for refused in ["text/html", "image/svg+xml", "application/json"] {
            assert_eq!(proof_kind(refused), None);
        }
    }

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
    fn a_pull_request_url_is_plain_only_as_the_github_path_of_that_number() {
        let url = "https://github.com/gannonh/punchlist-sandbox/pull/12";
        assert_eq!(plain_pull_request_url(12, url), Some(url));
        assert_eq!(plain_pull_request_url(13, url), None);
        assert_eq!(
            plain_pull_request_url(12, "https://github.com/../x/pull/12"),
            None
        );
        assert_eq!(
            plain_pull_request_url(12, "https://github.com/a/b/pull/12/files"),
            None
        );
    }

    #[test]
    fn a_review_thread_url_is_plain_only_as_a_github_discussion_of_a_pull_request() {
        let url = "https://github.com/gannonh/punchlist/pull/12#discussion_r1700001";
        assert_eq!(plain_review_thread_url(url), Some(url));
        for url in [
            "javascript:alert(1)//https://github.com/a/b/pull/12#discussion_r1",
            "https://github.com.evil.example/a/b/pull/12#discussion_r1",
            "https://github.com/a/b/pull/12#discussion_rabc",
            "https://github.com/a/b/pull/12#discussion_r",
            "https://github.com/a/b/pull/12",
            "https://github.com/a/b/pull/+12#discussion_r1",
        ] {
            assert_eq!(plain_review_thread_url(url), None, "{url}");
        }
    }

    #[test]
    fn update_issue_tells_an_absent_assignee_from_a_null_one() {
        let absent: UpdateIssue = serde_json::from_str(r#"{"title":"T"}"#).unwrap();
        assert_eq!(absent.assignee, None);
        let cleared: UpdateIssue = serde_json::from_str(r#"{"assignee":null}"#).unwrap();
        assert_eq!(cleared.assignee, Some(None));
        let id = Uuid::nil();
        let set: UpdateIssue = serde_json::from_str(&format!(r#"{{"assignee":"{id}"}}"#)).unwrap();
        assert_eq!(set.assignee, Some(Some(id)));
    }

    #[test]
    fn run_summary_is_tagged_by_state() {
        let failed = RunSummary::Failed {
            agent: "codex".into(),
            reason: None,
            at: "2026-10-10T12:00:00Z".parse().unwrap(),
        };
        assert_eq!(
            serde_json::to_string(&failed).unwrap(),
            r#"{"state":"failed","agent":"codex","reason":null,"at":"2026-10-10T12:00:00Z"}"#
        );
        assert_eq!(
            serde_json::to_string(&RunSummary::Idle).unwrap(),
            r#"{"state":"idle"}"#
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
