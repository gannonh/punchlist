//! Punchlist's workflow types and transition rules.
//!
//! This crate is pure: no I/O, so the server, the runner, `pl` and the web app (through
//! wasm32) apply the same rules.

mod workflow;

pub use workflow::{
    LoadError, Refusal, Role, Status, Transition, Workflow, default_workflow, display_name,
};
