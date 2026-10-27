//! Punchlist's workflow types and transition rules.
//!
//! This crate is pure: no I/O, so the server, the runner, `pl` and the web app (through
//! wasm32) apply the same rules.

mod agent;
mod gate;
mod link;
mod workflow;

pub use agent::{
    AGENT_KINDS, FenceError, agent_display_name, branch_name, fence, fenced, is_plain_name, quoted,
    unknown_agent_kind,
};
pub use gate::{
    CheckRunEvidence, Evidence, Gate, GateResult, GateSettings, ProofEvidence, ProofKind,
    PullRequestEvidence, PullRequestState, RequiredCheck, ReviewThreadEvidence,
};
pub use link::linked_issue_id;
pub use workflow::{
    CANCELED, DEFAULT_RUNS_PER_ENTRY, DONE, Dispatch, GITHUB_ACTIONS, IN_PROGRESS, LoadError,
    Problem, Refusal, Role, START, SYSTEM_PROMPT, Status, Transition, Workflow, default_workflow,
    display_name, workflow_schema,
};
