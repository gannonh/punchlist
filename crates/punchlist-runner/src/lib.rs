//! The Punchlist runner (ADR 0005). It registers with the server, heartbeats, claims issues
//! in Start and issues waiting in a status with a dispatch rule, prepares a worktree on the
//! issue's branch, runs the agent named by the dispatch rule (Claude Code or Codex), streams
//! the agent's output to the server and reports the outcome. It keeps no
//! database: the server holds runs, attempts and leases. A runner started again under the
//! same name gets the same runner back and carries on the attempts it still holds.

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
    /// Agent keys this runner can start, reported when it registers: `claude-code`, `codex`.
    pub agents: Vec<String>,
    /// Where repository clones and worktrees live.
    pub worktree_root: PathBuf,
    /// The `claude` executable.
    pub claude_command: PathBuf,
    /// The `pl` executable, which serves Punchlist's MCP tools to each run as `pl mcp`.
    pub mcp_command: PathBuf,
    /// Passed to `claude --model` when set, such as `haiku`.
    pub model: Option<String>,
    /// The `codex` executable, run as `codex app-server`.
    pub codex_command: PathBuf,
    /// The model a Codex thread starts with when set, such as `gpt-6-luna`; else Codex's
    /// built-in default, not the person's.
    pub codex_model: Option<String>,
    /// The reasoning effort a Codex thread starts with, such as `medium`.
    pub codex_effort: String,
    /// Issues this runner works on at once.
    pub max_concurrent: usize,
    pub heartbeat_interval: Duration,
    /// How long each claim request waits on the server for an issue to become claimable.
    pub claim_wait: Duration,
}

mod agent;
mod codex;
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
