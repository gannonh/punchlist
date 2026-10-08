//! The Punchlist runner (ADR 0005). It registers with the server, heartbeats, claims issues
//! in Start, prepares a worktree on the issue's branch, runs the agent named by the dispatch
//! rule, streams the agent's output to the server and reports the outcome. It keeps no
//! database: the server holds runs, attempts and leases.

use std::path::PathBuf;
use std::time::Duration;

/// Everything `pl runner start` passes to the runner.
#[derive(Debug, Clone)]
pub struct RunnerConfig {
    pub server_url: String,
    /// A person's token, used once to register. The runner gets its own token back.
    pub token: String,
    /// The name people read in `pl runner list`.
    pub name: String,
    /// Agent keys this runner can start. Only `claude-code` is supported today.
    pub agents: Vec<String>,
    /// Where repository clones and worktrees live.
    pub worktree_root: PathBuf,
    /// The `claude` executable.
    pub claude_command: PathBuf,
    /// Passed to `claude --model` when set, such as `haiku`.
    pub model: Option<String>,
    /// Issues this runner works on at once.
    pub max_concurrent: usize,
    pub heartbeat_interval: Duration,
    /// How long each claim request waits on the server for an issue to enter Start.
    pub claim_wait: Duration,
}

mod agent;
pub mod error;
mod orchestrator;
pub mod path_safety;
pub mod workspace;
mod worktree;

/// Registers, then heartbeats and claims until `shutdown` resolves.
pub async fn run(
    config: RunnerConfig,
    shutdown: impl std::future::Future<Output = ()> + Send,
) -> anyhow::Result<()> {
    orchestrator::run(config, shutdown).await
}
