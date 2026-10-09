//! Punchlist's workflow types and transition rules.
//!
//! This crate is pure: no I/O, so the server, the runner, `pl` and the web app (through
//! wasm32) apply the same rules.

mod agent;
mod link;
mod workflow;

pub use agent::{FenceError, agent_display_name, branch_name, fence};
pub use link::linked_issue_id;
pub use workflow::{
    Dispatch, LoadError, Refusal, Role, Status, Transition, Workflow, default_workflow,
    display_name,
};
