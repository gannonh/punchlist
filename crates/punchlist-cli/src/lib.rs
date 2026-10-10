//! `pl`, the Punchlist command line.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context;
use chrono::{DateTime, Utc};
use clap::{Parser, Subcommand};
use punchlist_api::{
    ChecksState, CreateIssue, Event, EventDetail, Issue, PullRequest, Run, RunOutcome, Runner,
    WorkflowInfo,
};
use punchlist_client::{Client, ClientError};
use serde::Deserialize;

#[derive(Debug, Parser)]
#[command(name = "pl", version, about = "Drive Punchlist from a terminal")]
pub struct Cli {
    /// Config file with `server_url` and `token`.
    #[arg(long, env = "PUNCHLIST_CONFIG", global = true)]
    pub config: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Work with issues.
    #[command(subcommand)]
    Issue(IssueCommand),
    /// Work with runners.
    #[command(subcommand)]
    Runner(RunnerCommand),
    /// Work with runs.
    #[command(subcommand)]
    Run(RunCommand),
    /// Work with pull requests.
    #[command(name = "pr", subcommand)]
    PullRequest(PullRequestCommand),
    /// Read the workspace's workflow.
    #[command(subcommand)]
    Workflow(WorkflowCommand),
    /// Serve Punchlist's MCP tools on stdio for one issue. The runner starts this for each
    /// agent run.
    Mcp(McpArgs),
}

#[derive(Debug, Subcommand)]
pub enum WorkflowCommand {
    /// Print the active workflow's version, source commit and statuses.
    Show,
}

#[derive(Debug, clap::Args)]
pub struct McpArgs {
    #[arg(long, env = "PUNCHLIST_SERVER_URL")]
    pub server_url: String,
    /// The agent token the run acts with.
    #[arg(long, env = "PUNCHLIST_TOKEN", hide_env_values = true)]
    pub token: String,
    /// The issue the tools act on.
    #[arg(long, env = "PUNCHLIST_ISSUE")]
    pub issue: String,
}

#[derive(Debug, Subcommand)]
pub enum PullRequestCommand {
    /// List pull requests whose branch and title name no issue.
    Unlinked,
}

#[derive(Debug, Subcommand)]
pub enum RunnerCommand {
    /// Register this machine as a runner and work on issues in Start until stopped.
    Start(RunnerStart),
    /// List registered runners and their last heartbeat.
    List,
}

/// Flags override the `[runner]` table in the config file.
#[derive(Debug, Default, clap::Args)]
pub struct RunnerStart {
    /// The name `pl runner list` shows. Default: the machine's hostname.
    #[arg(long)]
    pub name: Option<String>,
    /// Where repository clones and worktrees live.
    #[arg(long)]
    pub worktree_root: Option<String>,
    /// Passed to `claude --model`, such as `haiku`.
    #[arg(long)]
    pub model: Option<String>,
    /// The `claude` executable.
    #[arg(long)]
    pub claude_command: Option<String>,
    /// Issues to work on at once.
    #[arg(long)]
    pub max_concurrent: Option<usize>,
}

#[derive(Debug, Subcommand)]
pub enum RunCommand {
    /// Print every log line of a run.
    Log { run_id: String },
}

#[derive(Debug, Subcommand)]
pub enum IssueCommand {
    /// Create an issue and print its id.
    Create {
        #[arg(long)]
        title: String,
        /// Markdown.
        #[arg(long, default_value = "")]
        body: String,
    },
    /// Show an issue and its timeline.
    Show { id: String },
    /// List the workspace's issues.
    List,
    /// Request a transition to another status, such as `todo` or `agent_review`. A refusal
    /// names the rule or gate.
    #[command(visible_alias = "transition")]
    Move { id: String, status: String },
    /// Comment on an issue.
    Comment {
        id: String,
        /// Markdown.
        #[arg(long)]
        body: String,
    },
}

/// `~/.config/punchlist/config.toml`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub server_url: String,
    pub token: String,
    #[serde(default)]
    pub runner: RunnerSection,
}

/// The optional `[runner]` table.
#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RunnerSection {
    pub name: Option<String>,
    pub agents: Option<Vec<String>>,
    pub worktree_root: Option<String>,
    pub claude_command: Option<String>,
    pub model: Option<String>,
    pub max_concurrent: Option<usize>,
}

impl Config {
    pub fn default_path() -> anyhow::Result<PathBuf> {
        let home = std::env::var_os("HOME").context("HOME is not set; pass --config")?;
        Ok(PathBuf::from(home).join(".config/punchlist/config.toml"))
    }

    pub fn load(path: &Path) -> anyhow::Result<Config> {
        let text = std::fs::read_to_string(path).with_context(|| {
            format!(
                "cannot read {}; it needs `server_url` and `token` (punchlist-server bootstrap prints both)",
                path.display()
            )
        })?;
        toml::from_str(&text).with_context(|| format!("invalid config in {}", path.display()))
    }
}

pub async fn run(cli: Cli, out: &mut dyn Write) -> anyhow::Result<()> {
    // The MCP server takes its server and token from the run, not from a config file.
    if let Command::Mcp(args) = cli.command {
        // stderr only: stdout carries the protocol.
        init_logging();
        let client = Client::new(&args.server_url, &args.token)?;
        return punchlist_mcp::serve_stdio(client, args.issue).await;
    }
    let path = match cli.config {
        Some(path) => path,
        None => Config::default_path()?,
    };
    let config = Config::load(&path)?;
    let client = Client::new(&config.server_url, &config.token)?;
    match cli.command {
        Command::Issue(command) => issue(&client, command, out).await,
        Command::Runner(RunnerCommand::List) => {
            let list = client.list_runners().await?;
            write!(out, "{}", render_runners(&list.runners, Utc::now()))?;
            Ok(())
        }
        Command::Runner(RunnerCommand::Start(flags)) => {
            init_logging();
            let home = std::env::var("HOME").ok();
            let runner = runner_config(&config, flags, home.as_deref(), &hostname());
            punchlist_runner::run(runner, shutdown_signal()).await
        }
        Command::PullRequest(PullRequestCommand::Unlinked) => {
            let pull_requests = client.unlinked_pull_requests().await?;
            write!(out, "{}", render_unlinked(&pull_requests))?;
            Ok(())
        }
        Command::Workflow(WorkflowCommand::Show) => {
            write!(out, "{}", render_workflow(&client.workflow().await?))?;
            Ok(())
        }
        Command::Mcp(_) => unreachable!("handled above"),
        Command::Run(RunCommand::Log { run_id }) => {
            let log = client.run_log(&run_id).await?;
            for line in &log.lines {
                writeln!(out, "{}", line.line)?;
            }
            Ok(())
        }
    }
}

fn init_logging() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        // Plain text when the log goes to a file.
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()))
        .try_init();
}

/// Resolves on Ctrl-C or SIGTERM.
async fn shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    let Ok(mut term) = signal(SignalKind::terminate()) else {
        let _ = tokio::signal::ctrl_c().await;
        return;
    };
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
}

fn hostname() -> String {
    let from_file = std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|h| h.trim().to_string());
    from_file
        .filter(|h| !h.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok().filter(|h| !h.is_empty()))
        .unwrap_or_else(|| "runner".to_string())
}

/// Expands a leading `~` to `home`.
fn expand_home(path: &str, home: Option<&str>) -> PathBuf {
    match (path.strip_prefix("~"), home) {
        (Some(""), Some(home)) => PathBuf::from(home),
        (Some(rest), Some(home)) if rest.starts_with('/') => {
            PathBuf::from(home).join(rest.trim_start_matches('/'))
        }
        _ => PathBuf::from(path),
    }
}

/// Flags, then the `[runner]` table, then defaults.
pub fn runner_config(
    config: &Config,
    flags: RunnerStart,
    home: Option<&str>,
    hostname: &str,
) -> punchlist_runner::RunnerConfig {
    let table = &config.runner;
    let worktree_root = flags
        .worktree_root
        .or_else(|| table.worktree_root.clone())
        .unwrap_or_else(|| "~/.local/share/punchlist/worktrees".to_string());
    let claude_command = flags
        .claude_command
        .or_else(|| table.claude_command.clone())
        .unwrap_or_else(|| "claude".to_string());
    punchlist_runner::RunnerConfig {
        server_url: config.server_url.clone(),
        token: config.token.clone(),
        name: flags
            .name
            .or_else(|| table.name.clone())
            .unwrap_or_else(|| hostname.to_string()),
        agents: table
            .agents
            .clone()
            .unwrap_or_else(|| vec!["claude-code".to_string()]),
        worktree_root: expand_home(&worktree_root, home),
        claude_command: expand_home(&claude_command, home),
        mcp_command: std::env::current_exe().unwrap_or_else(|_| PathBuf::from("pl")),
        model: flags.model.or_else(|| table.model.clone()),
        max_concurrent: flags.max_concurrent.or(table.max_concurrent).unwrap_or(1),
        heartbeat_interval: Duration::from_secs(10),
        claim_wait: Duration::from_secs(20),
    }
}

fn render_runners(runners: &[Runner], now: DateTime<Utc>) -> String {
    if runners.is_empty() {
        return "No runners.\n".to_string();
    }
    let name_width = runners.iter().map(|r| r.name.len()).max().unwrap_or(0);
    let agents: Vec<String> = runners.iter().map(|r| r.agents.join(",")).collect();
    let agents_width = agents.iter().map(String::len).max().unwrap_or(0);
    runners
        .iter()
        .zip(&agents)
        .map(|(r, agents)| {
            format!(
                "{:name_width$}  {:agents_width$}  heartbeat {} ago\n",
                r.name,
                agents,
                format_age((now - r.last_heartbeat).num_seconds()),
            )
        })
        .collect()
}

/// `4s`, `3m` or `2h`.
fn format_age(seconds: i64) -> String {
    let seconds = seconds.max(0);
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m", seconds / 60)
    } else {
        format!("{}h", seconds / 3600)
    }
}

/// `1m 23s`, `4.2s` or `—`.
fn format_duration(ms: Option<i64>) -> String {
    let Some(ms) = ms else {
        return "—".to_string();
    };
    let ms = ms.max(0);
    if ms >= 60_000 {
        format!("{}m {}s", ms / 60_000, (ms % 60_000) / 1000)
    } else {
        format!("{}.{}s", ms / 1000, (ms % 1000) / 100)
    }
}

fn truncate_line(line: &str) -> String {
    if line.chars().count() <= 200 {
        line.to_string()
    } else {
        let mut cut: String = line.chars().take(200).collect();
        cut.push('…');
        cut
    }
}

fn render_runs(runs: &[Run]) -> String {
    let mut text = String::from("\nRuns\n");
    if runs.is_empty() {
        text.push_str("  No runs yet.\n");
    }
    for run in runs {
        text.push_str(&format!(
            "  {}  {}  runner {}  started {}  duration {}  {}",
            run.id,
            run.agent_name,
            run.runner.name,
            run.started_at.format("%Y-%m-%d %H:%M:%S UTC"),
            format_duration(run.duration_ms),
            run.outcome.as_str(),
        ));
        if let (Some(input), Some(output)) = (run.input_tokens, run.output_tokens) {
            text.push_str(&format!("  tokens {input} in / {output} out"));
        }
        if run.outcome == RunOutcome::Failed
            && let Some(reason) = &run.failure_reason
        {
            text.push_str(&format!("  ({reason})"));
        }
        text.push('\n');
        for line in &run.log_tail {
            text.push_str(&format!("    {}\n", truncate_line(&line.line)));
        }
    }
    text
}

async fn issue(client: &Client, command: IssueCommand, out: &mut dyn Write) -> anyhow::Result<()> {
    match command {
        IssueCommand::Create { title, body } => {
            let issue = client.create_issue(&CreateIssue { title, body }).await?;
            writeln!(out, "{}", issue.id)?;
        }
        IssueCommand::Show { id } => {
            let (issue, events) = issue_with_timeline(client, &id).await?;
            let pull_requests = client.issue_pull_requests(&id).await?;
            let runs = client.issue_runs(&id).await?;
            write!(
                out,
                "{}{}",
                render_show(&issue, &pull_requests, &events),
                render_runs(&runs)
            )?;
        }
        IssueCommand::List => {
            let list = client.list_issues().await?;
            write!(out, "{}", render_list(&list.issues))?;
        }
        IssueCommand::Move { id, status } => {
            let moved = client.move_issue(&id, &status).await.map_err(with_gates)?;
            let EventDetail::Transition { from_name, .. } = &moved.event.detail else {
                anyhow::bail!("the server answered a move with an event that is not a transition");
            };
            writeln!(
                out,
                "{}  {} → {}",
                moved.issue.id, from_name, moved.issue.status_name
            )?;
        }
        IssueCommand::Comment { id, body } => {
            client.comment(&id, &body).await?;
            writeln!(out, "Commented on {id}.")?;
        }
    }
    Ok(())
}

/// A refused move whose gates ran lists every gate's result under the message.
fn with_gates(error: ClientError) -> anyhow::Error {
    let ClientError::Refused(body) = &error else {
        return error.into();
    };
    if body.gates.is_empty() {
        return error.into();
    }
    let gates: Vec<String> = body
        .gates
        .iter()
        .map(|g| {
            let result = if g.passed { "pass" } else { "fail" };
            format!("  {}: {result} ({})", g.gate, g.reason)
        })
        .collect();
    anyhow::anyhow!("{error}\n{}", gates.join("\n"))
}

/// `pl workflow show`.
fn render_workflow(workflow: &WorkflowInfo) -> String {
    format!(
        "Version   {}\nCommit    {}\nStatuses  {}\n",
        workflow.version,
        workflow.commit_sha.as_deref().unwrap_or("built-in"),
        workflow.statuses.join(", ")
    )
}

/// The issue and its timeline from two requests. A move landing between them would show an
/// old status above a timeline that already has the new move, so read again until the last
/// move matches the status.
async fn issue_with_timeline(client: &Client, id: &str) -> anyhow::Result<(Issue, Vec<Event>)> {
    let mut attempts = 0;
    loop {
        let issue = client.get_issue(id).await?;
        let events = client.issue_events(id).await?;
        // Comments do not change the status, so compare with the last transition.
        let last_move = events.iter().rev().find_map(|event| match &event.detail {
            EventDetail::Transition { to, .. } => Some(to),
            EventDetail::Comment { .. } => None,
        });
        let consistent = last_move.is_none_or(|to| *to == issue.status);
        attempts += 1;
        if consistent || attempts == 3 {
            return Ok((issue, events));
        }
    }
}

fn render_show(issue: &Issue, pull_requests: &[PullRequest], events: &[Event]) -> String {
    let mut text = format!(
        "{}  {}\nStatus: {}\n",
        issue.id, issue.title, issue.status_name
    );
    if !issue.body.is_empty() {
        text.push('\n');
        text.push_str(issue.body.trim_end());
        text.push('\n');
    }
    text.push_str(&render_pull_requests(pull_requests));
    text.push_str("\nTimeline\n");
    if events.is_empty() {
        text.push_str("  No transitions yet.\n");
    }
    for event in events {
        let who = format!(
            "  {}  {} ({})  ",
            event.created_at.format("%Y-%m-%d %H:%M:%S UTC"),
            event.actor.name,
            event.actor.role,
        );
        match &event.detail {
            EventDetail::Transition {
                from_name,
                to_name,
                delivery_id,
                ..
            } => {
                text.push_str(&format!("{who}{from_name} → {to_name}"));
                if let Some(delivery_id) = delivery_id {
                    text.push_str(&format!("  [delivery {delivery_id}]"));
                }
                text.push('\n');
            }
            EventDetail::Comment { body } => {
                text.push_str(&format!("{who}commented:\n"));
                for line in body.trim_end().lines() {
                    if line.is_empty() {
                        text.push('\n');
                    } else {
                        text.push_str(&format!("    {line}\n"));
                    }
                }
            }
        }
    }
    text
}

/// The `Pull requests` section of `pl issue show`.
fn render_pull_requests(pull_requests: &[PullRequest]) -> String {
    let mut text = String::from("\nPull requests\n");
    if pull_requests.is_empty() {
        text.push_str("  None linked.\n");
    }
    for pr in pull_requests {
        let checks = if pr.checks == ChecksState::None {
            "no checks".to_string()
        } else {
            format!(
                "checks {} {}/{}",
                pr.checks.as_str(),
                pr.checks_passed,
                pr.checks_total
            )
        };
        let threads = if pr.open_threads == 1 {
            "1 open thread".to_string()
        } else {
            format!("{} open threads", pr.open_threads)
        };
        text.push_str(&format!(
            "  #{} {}  {}  {}  {}\n    {}\n",
            pr.number,
            pr.state_word(),
            pr.branch,
            checks,
            threads,
            pr.url,
        ));
    }
    text
}

/// `pl pr unlinked`.
fn render_unlinked(pull_requests: &[PullRequest]) -> String {
    if pull_requests.is_empty() {
        return "No unlinked pull requests.\n".to_string();
    }
    pull_requests
        .iter()
        .map(|pr| {
            format!(
                "#{}  {}  {}  {}\n",
                pr.number,
                pr.state_word(),
                pr.branch,
                pr.title
            )
        })
        .collect()
}

fn render_list(issues: &[Issue]) -> String {
    if issues.is_empty() {
        return "No issues.\n".to_string();
    }
    let id_width = issues.iter().map(|i| i.id.len()).max().unwrap_or(0);
    let status_width = issues
        .iter()
        .map(|i| i.status_name.len())
        .max()
        .unwrap_or(0);
    issues
        .iter()
        .map(|i| {
            format!(
                "{:id_width$}  {:status_width$}  {}\n",
                i.id, i.status_name, i.title
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(text: &str) -> Result<Config, toml::de::Error> {
        toml::from_str(&format!("server_url = \"http://x\"\ntoken = \"t\"\n{text}"))
    }

    fn pull_request(number: i64) -> PullRequest {
        PullRequest {
            number,
            title: "Fix it".into(),
            branch: "feature/pl-7-fix".into(),
            url: format!("https://github.com/gannonh/punchlist-sandbox/pull/{number}"),
            author_login: "gannonh".into(),
            state: punchlist_api::PullRequestState::Open,
            draft: true,
            merge_state: "clean".into(),
            checks: ChecksState::Passing,
            checks_total: 2,
            checks_passed: 2,
            open_threads: 0,
            issue_id: Some("PL-7".into()),
            updated_at: "2026-10-10T12:00:00Z".parse().unwrap(),
        }
    }

    fn event(role: punchlist_api::Role, name: &str, detail: EventDetail) -> Event {
        Event {
            seq: 1,
            issue_id: "PL-7".into(),
            actor: punchlist_api::Actor {
                id: "00000000-0000-0000-0000-000000000000".parse().unwrap(),
                name: name.into(),
                role,
            },
            created_at: "2026-10-10T12:30:45Z".parse().unwrap(),
            detail,
        }
    }

    #[test]
    fn renders_pull_requests_on_an_issue() {
        assert_eq!(
            render_pull_requests(&[]),
            "\nPull requests\n  None linked.\n"
        );

        let mut failing = pull_request(11);
        failing.state = punchlist_api::PullRequestState::Merged;
        failing.checks = ChecksState::Failing;
        failing.checks_passed = 1;
        failing.open_threads = 1;
        let mut bare = pull_request(10);
        bare.draft = false;
        bare.checks = ChecksState::None;
        bare.checks_total = 0;
        bare.checks_passed = 0;
        bare.open_threads = 3;
        assert_eq!(
            render_pull_requests(&[pull_request(12), failing, bare]),
            "\nPull requests\n\
             \x20 #12 draft  feature/pl-7-fix  checks passing 2/2  0 open threads\n\
             \x20   https://github.com/gannonh/punchlist-sandbox/pull/12\n\
             \x20 #11 merged  feature/pl-7-fix  checks failing 1/2  1 open thread\n\
             \x20   https://github.com/gannonh/punchlist-sandbox/pull/11\n\
             \x20 #10 open  feature/pl-7-fix  no checks  3 open threads\n\
             \x20   https://github.com/gannonh/punchlist-sandbox/pull/10\n"
        );
    }

    #[test]
    fn renders_unlinked_pull_requests() {
        assert_eq!(render_unlinked(&[]), "No unlinked pull requests.\n");
        let mut closed = pull_request(8);
        closed.state = punchlist_api::PullRequestState::Closed;
        assert_eq!(
            render_unlinked(&[pull_request(9), closed]),
            "#9  draft  feature/pl-7-fix  Fix it\n#8  closed  feature/pl-7-fix  Fix it\n"
        );
    }

    #[test]
    fn timeline_shows_comments_and_deliveries() {
        let issue = Issue {
            id: "PL-7".into(),
            title: "Fix".into(),
            body: String::new(),
            status: "agent_review".into(),
            status_name: "Agent Review".into(),
            created_at: "2026-10-10T12:00:00Z".parse().unwrap(),
            updated_at: "2026-10-10T12:00:00Z".parse().unwrap(),
        };
        let mut moved = EventDetail::transition(
            "in_progress".into(),
            "agent_review".into(),
            "sha256:ab".into(),
        );
        if let EventDetail::Transition { delivery_id, .. } = &mut moved {
            *delivery_id = Some("d-1".into());
        }
        let events = [
            event(punchlist_api::Role::Github, "gannonh", moved),
            event(
                punchlist_api::Role::Github,
                "gannonh",
                EventDetail::Comment {
                    body: "Closed without merging.\n\nSee #12.\n".into(),
                },
            ),
        ];
        assert_eq!(
            render_show(&issue, &[], &events),
            "PL-7  Fix\nStatus: Agent Review\n\
             \nPull requests\n  None linked.\n\
             \nTimeline\n\
             \x20 2026-10-10 12:30:45 UTC  gannonh (github)  In Progress → Agent Review  [delivery d-1]\n\
             \x20 2026-10-10 12:30:45 UTC  gannonh (github)  commented:\n\
             \x20   Closed without merging.\n\
             \n\
             \x20   See #12.\n"
        );
    }

    #[test]
    fn renders_the_workflow() {
        let info = WorkflowInfo {
            version: "sha256:ab".into(),
            commit_sha: None,
            statuses: vec!["backlog".into(), "todo".into()],
        };
        assert_eq!(
            render_workflow(&info),
            "Version   sha256:ab\nCommit    built-in\nStatuses  backlog, todo\n"
        );
    }

    #[test]
    fn formats_durations() {
        assert_eq!(format_duration(None), "—");
        assert_eq!(format_duration(Some(4200)), "4.2s");
        assert_eq!(format_duration(Some(999)), "0.9s");
        assert_eq!(format_duration(Some(83_000)), "1m 23s");
        assert_eq!(format_duration(Some(60_000)), "1m 0s");
    }

    #[test]
    fn formats_ages() {
        assert_eq!(format_age(-3), "0s");
        assert_eq!(format_age(4), "4s");
        assert_eq!(format_age(59), "59s");
        assert_eq!(format_age(60), "1m");
        assert_eq!(format_age(3599), "59m");
        assert_eq!(format_age(7200), "2h");
    }

    #[test]
    fn truncates_long_lines() {
        assert_eq!(truncate_line("short"), "short");
        let long = "x".repeat(250);
        assert_eq!(truncate_line(&long), format!("{}…", "x".repeat(200)));
    }

    #[test]
    fn runner_table_parses_and_rejects_unknown_keys() {
        let parsed = config("[runner]\nname = \"sartre\"\nmax_concurrent = 2\n").unwrap();
        assert_eq!(parsed.runner.name.as_deref(), Some("sartre"));
        assert_eq!(parsed.runner.max_concurrent, Some(2));
        assert_eq!(config("").unwrap().runner, RunnerSection::default());
        assert!(config("[runner]\nbogus = 1\n").is_err());
    }

    #[test]
    fn runner_config_layers_flags_table_and_defaults() {
        let defaults = runner_config(
            &config("").unwrap(),
            RunnerStart::default(),
            Some("/h"),
            "box",
        );
        assert_eq!(defaults.name, "box");
        assert_eq!(defaults.agents, vec!["claude-code".to_string()]);
        assert_eq!(
            defaults.worktree_root,
            PathBuf::from("/h/.local/share/punchlist/worktrees")
        );
        assert_eq!(defaults.claude_command, PathBuf::from("claude"));
        assert_eq!((defaults.model, defaults.max_concurrent), (None, 1));

        let table = config(
            "[runner]\nname = \"t\"\nworktree_root = \"~/wt\"\nmodel = \"haiku\"\nmax_concurrent = 3\n",
        )
        .unwrap();
        let flags = RunnerStart {
            name: Some("f".into()),
            ..RunnerStart::default()
        };
        let merged = runner_config(&table, flags, Some("/h"), "box");
        assert_eq!(merged.name, "f");
        assert_eq!(merged.worktree_root, PathBuf::from("/h/wt"));
        assert_eq!(merged.model.as_deref(), Some("haiku"));
        assert_eq!(merged.max_concurrent, 3);
    }
}
