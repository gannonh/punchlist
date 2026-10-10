//! Punchlist's workflow types and transition rules.
//!
//! This crate is pure: no I/O, so the server, the runner, `pl` and the web app (through
//! wasm32) apply the same rules.

mod agent;
mod gate;
mod link;
mod workflow;

pub use agent::{FenceError, agent_display_name, branch_name, fence};
pub use gate::{Evidence, Gate, GateResult, PullRequestEvidence, PullRequestState};
pub use link::linked_issue_id;
pub use workflow::{
    Dispatch, LoadError, Problem, Refusal, Role, SYSTEM_PROMPT, Status, Transition, Workflow,
    default_workflow, display_name, workflow_schema,
};
