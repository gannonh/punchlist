//! `pl`, the Punchlist command line.

mod linear;

pub use linear::{IssuesPage, parse_comments, parse_issues, parse_labels, parse_project};

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context;
use chrono::{DateTime, Utc};
use clap::{Parser, Subcommand};
use punchlist_api::{
    ActorInfo, ChecksState, CreateAgent, CreateIssue, CreateMilestone, CreateProject, CreatedAgent,
    Event, EventDetail, GateChecklist, ImportReport, ImportState, ImportStatus, Issue, Project,
    Proof, ProofKind, PullRequest, Run, RunOutcome, Runner, SearchResults, UpdateIssue,
    WorkflowInfo, Workspace, gate_lines,
};
use punchlist_client::{Client, ClientError};
use serde::Deserialize;

#[derive(Debug, Parser)]
#[command(name = "pl", version, about = "Drive Punchlist from a terminal")]
pub struct Cli {
    /// Config file with `server_url` and `token`.
    #[arg(long, env = "PUNCHLIST_CONFIG", global = true)]
    pub config: Option<PathBuf>,

    /// The workspace of the workspace-level commands, by prefix (`PL`) or id; else the
    /// config's `workspace`, else your account's only workspace. Issue commands take it from
    /// the issue id.
    #[arg(long, env = "PUNCHLIST_WORKSPACE", global = true)]
    pub workspace: Option<String>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// List the workspaces of your account.
    #[command(subcommand)]
    Workspace(WorkspaceCommand),
    /// Work with issues.
    #[command(subcommand)]
    Issue(IssueCommand),
    /// Work with actors: people, agents and runners.
    #[command(subcommand)]
    Actor(ActorCommand),
    /// Work with runners.
    #[command(subcommand)]
    Runner(RunnerCommand),
    /// Work with runs.
    #[command(subcommand)]
    Run(RunCommand),
    /// Attach and list proof: screenshots, videos and reports of an issue's work.
    #[command(subcommand)]
    Proof(ProofCommand),
    /// Work with pull requests.
    #[command(name = "pr", subcommand)]
    PullRequest(PullRequestCommand),
    /// Work with the workspace's projects.
    #[command(subcommand)]
    Project(ProjectCommand),
    /// Work with a project's milestones.
    #[command(subcommand)]
    Milestone(MilestoneCommand),
    /// Read the workspace's workflow.
    #[command(subcommand)]
    Workflow(WorkflowCommand),
    /// Import issues from Linear.
    #[command(subcommand)]
    Import(ImportCommand),
    /// Find the workspace's issues whose id, title, body or comments contain a text, in any
    /// case, with the line where it matched.
    Search {
        /// One phrase of at least 3 characters, matched literally.
        #[arg(required = true, num_args = 1..)]
        query: Vec<String>,
        /// The most issues to print: 50 by default, at most 100.
        #[arg(long)]
        limit: Option<u32>,
    },
    /// Serve Punchlist's MCP tools on stdio for one issue. The runner starts this for each
    /// agent run.
    Mcp(McpArgs),
}

#[derive(Debug, Subcommand)]
pub enum ImportCommand {
    /// Import one Linear project: its issues with their statuses, labels, milestones, parents
    /// and comments. Reads Linear with the personal API key in LINEAR_API_KEY, which is never
    /// sent to the server. Rerun it to bring later changes in; nothing is imported twice.
    Linear {
        /// The team's key, such as KAT.
        #[arg(long)]
        team: String,
        /// The project's name in that team.
        #[arg(long)]
        project: String,
    },
    /// Print what the workspace has from Linear, and the last import with its notes or error.
    Status,
}

#[derive(Debug, Subcommand)]
pub enum WorkspaceCommand {
    /// Print each workspace's prefix, name and repository, with `*` on the one `pl` uses.
    List,
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
    /// The run the agent works in, for `wait_for_person`.
    #[arg(long, env = "PUNCHLIST_RUN")]
    pub run: Option<String>,
}

#[derive(Debug, Subcommand)]
pub enum ProofCommand {
    /// Upload a file as proof for an issue. Its type comes from its extension.
    Attach {
        id: String,
        path: PathBuf,
        /// What the file shows.
        #[arg(long)]
        title: String,
        /// image, video or report; checked against the file's type.
        #[arg(long, value_parser = parse_kind)]
        kind: Option<ProofKind>,
    },
    /// List an issue's proof, oldest first.
    List { id: String },
}

fn parse_kind(s: &str) -> Result<ProofKind, String> {
    ProofKind::parse(s).ok_or_else(|| format!("`{s}` is not image, video or report"))
}

#[derive(Debug, Subcommand)]
pub enum PullRequestCommand {
    /// List pull requests whose branch and title name no issue.
    Unlinked,
}

#[derive(Debug, Subcommand)]
pub enum ProjectCommand {
    /// Create a project in the workspace.
    Create { name: String },
    /// List the workspace's projects, each with its milestones and their PASS when text.
    List,
}

#[derive(Debug, Subcommand)]
pub enum MilestoneCommand {
    /// Create a milestone in a project.
    Create {
        name: String,
        /// The project's name.
        #[arg(long)]
        project: String,
        /// What must be true for the milestone to pass.
        #[arg(long, default_value = "")]
        pass_when: String,
    },
}

/// What `pl issue edit` sets or clears: at least one of these.
#[derive(Debug, clap::Args)]
#[group(required = true, multiple = true)]
pub struct IssueEdit {
    #[arg(long)]
    pub title: Option<String>,
    /// Markdown.
    #[arg(long, conflicts_with = "body_file")]
    pub body: Option<String>,
    /// A file holding the body, or `-` for stdin.
    #[arg(long)]
    pub body_file: Option<PathBuf>,
    /// A label; repeat it for more. Replaces the issue's labels.
    #[arg(long, conflicts_with = "no_labels")]
    pub label: Vec<String>,
    /// Clear the labels.
    #[arg(long)]
    pub no_labels: bool,
    /// The parent issue's id, such as PL-1.
    #[arg(long, conflicts_with = "no_parent")]
    pub parent: Option<String>,
    /// Clear the parent.
    #[arg(long)]
    pub no_parent: bool,
    /// A project's name.
    #[arg(long, conflicts_with = "no_project")]
    pub project: Option<String>,
    /// Clear the project, and with it the milestone.
    #[arg(long)]
    pub no_project: bool,
    /// A milestone's name, looked up in --project, else in the issue's project.
    #[arg(long, conflicts_with_all = ["no_milestone", "no_project"])]
    pub milestone: Option<String>,
    /// Clear the milestone.
    #[arg(long)]
    pub no_milestone: bool,
}

#[derive(Debug, Subcommand)]
pub enum ActorCommand {
    /// Mint an agent actor with its own token. It acts on the workspace's issues and nothing
    /// else.
    Create {
        /// Create an agent: the only kind of actor this makes.
        #[arg(long, required = true)]
        agent: bool,
        /// The agent key: claude-code, codex, opencode, pi or cursor.
        #[arg(long)]
        kind: String,
        /// What the timeline calls the agent. The kind's name by default, such as `Codex`.
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        model: Option<String>,
        /// The name of the person the agent acts for. You by default.
        #[arg(long)]
        owner: Option<String>,
    },
    /// List the workspace's people, agents and runners.
    List,
}

#[derive(Debug, Subcommand)]
pub enum RunnerCommand {
    /// Register this machine as a runner and work on the issues it can claim until stopped.
    Start(Box<RunnerStart>),
    /// List registered runners and their last heartbeat.
    List,
}

/// Flags override the `[runner]` table in the config file.
#[derive(Debug, Default, clap::Args)]
pub struct RunnerStart {
    /// The name `pl runner list` shows. Default: the machine's hostname.
    #[arg(long)]
    pub name: Option<String>,
    /// The runner's state: repository clones, worktrees, MCP configs and Codex homes.
    /// Default: `~/.local/share/punchlist/runner`.
    #[arg(long)]
    pub state_dir: Option<String>,
    /// Deprecated: `--state-dir`.
    #[arg(long, hide = true, conflicts_with = "state_dir")]
    pub worktree_root: Option<String>,
    /// Passed to `claude --model`, such as `haiku`.
    #[arg(long)]
    pub model: Option<String>,
    /// The `claude` executable.
    #[arg(long)]
    pub claude_command: Option<String>,
    /// The agents this runner can run, reported to the server when it registers:
    /// `claude-code`, `codex`. Default: `claude-code`.
    #[arg(long, value_delimiter = ',')]
    pub agents: Option<Vec<String>>,
    /// The model a Codex run starts with, such as `gpt-6-luna`. Default: Codex's built-in
    /// default model. The model, provider and proxy settings in your own Codex
    /// `config.toml` do not reach a run.
    #[arg(long)]
    pub codex_model: Option<String>,
    /// The reasoning effort a Codex run starts with, one the model offers, such as `low`
    /// or `high`. Default: `medium`.
    #[arg(long)]
    pub codex_effort: Option<String>,
    /// The `codex` executable.
    #[arg(long)]
    pub codex_command: Option<String>,
    /// Issues to work on at once.
    #[arg(long)]
    pub max_concurrent: Option<usize>,
}

#[derive(Debug, Subcommand)]
pub enum RunCommand {
    /// Print every log line of a run.
    Log { run_id: String },
    /// List an issue's runs, newest first, each with its attempts.
    List { issue: String },
}

#[derive(Debug, Subcommand)]
pub enum IssueCommand {
    /// Create an issue and print its id.
    Create {
        #[arg(long)]
        title: String,
        /// Markdown.
        #[arg(long, default_value = "", conflicts_with = "body_file")]
        body: String,
        /// A file holding the body, or `-` for stdin.
        #[arg(long)]
        body_file: Option<PathBuf>,
    },
    /// Show an issue and its timeline.
    Show { id: String },
    /// List the workspace's issues.
    List,
    /// Request a transition to another status, such as `todo` or `agent_review`. A refusal
    /// names the rule or gate.
    #[command(visible_alias = "transition")]
    Move { id: String, status: String },
    /// Change an issue's title, body and labels, or set or clear its parent, project and
    /// milestone.
    Edit {
        id: String,
        #[command(flatten)]
        edit: IssueEdit,
    },
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
    /// The workspace `pl` uses when `--workspace` and `PUNCHLIST_WORKSPACE` name none.
    pub workspace: Option<String>,
    #[serde(default)]
    pub runner: RunnerSection,
}

/// The optional `[runner]` table.
#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RunnerSection {
    pub name: Option<String>,
    pub agents: Option<Vec<String>>,
    #[serde(alias = "worktree_root")]
    pub state_dir: Option<String>,
    pub claude_command: Option<String>,
    pub model: Option<String>,
    pub codex_command: Option<String>,
    pub codex_model: Option<String>,
    pub codex_effort: Option<String>,
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
        return punchlist_mcp::serve_stdio(client, args.issue, args.run).await;
    }
    let path = match cli.config {
        Some(path) => path,
        None => Config::default_path()?,
    };
    let config = Config::load(&path)?;
    let client = Client::new(&config.server_url, &config.token)?;
    let named = cli.workspace.or_else(|| config.workspace.clone());
    let workspace = match &cli.command {
        Command::Workspace(WorkspaceCommand::List) => {
            let workspaces = client.workspaces().await?;
            let chosen = match pick_workspace(&workspaces, named.as_deref()) {
                Ok(chosen) => Some(chosen),
                Err(error) if named.is_some() => return Err(error),
                Err(_) => None,
            };
            write!(out, "{}", render_workspaces(&workspaces, chosen))?;
            return Ok(());
        }
        command if workspace_level(command) => {
            let workspaces = client.workspaces().await?;
            Some(
                pick_workspace(&workspaces, named.as_deref())?
                    .id
                    .to_string(),
            )
        }
        _ => None,
    };
    let client = match &workspace {
        Some(id) => client.in_workspace(id),
        None => client,
    };
    let workspace = || workspace.as_deref().expect("a workspace-level command");
    match cli.command {
        Command::Workspace(_) => unreachable!("handled above"),
        Command::Issue(command) => issue(&client, command, out).await,
        Command::Actor(ActorCommand::Create {
            agent: _,
            kind,
            name,
            model,
            owner,
        }) => {
            let request = CreateAgent {
                kind,
                name,
                model,
                owner,
            };
            write!(
                out,
                "{}",
                render_created(&client.create_agent(&request).await?)
            )?;
            Ok(())
        }
        Command::Actor(ActorCommand::List) => {
            write!(
                out,
                "{}",
                render_actors(&client.list_actors().await?.actors)
            )?;
            Ok(())
        }
        Command::Runner(RunnerCommand::List) => {
            let list = client.list_runners().await?;
            write!(out, "{}", render_runners(&list.runners, Utc::now()))?;
            Ok(())
        }
        Command::Runner(RunnerCommand::Start(flags)) => {
            init_logging();
            let home = std::env::var("HOME").ok();
            if flags.worktree_root.is_some() {
                eprintln!("--worktree-root is deprecated; use --state-dir");
            }
            let (mut runner, move_from) =
                runner_config(&config, *flags, home.as_deref(), &hostname(), Path::exists);
            runner.workspace = Some(workspace().to_string());
            if let Some(from) = move_from {
                let claude_dir = std::env::var_os("CLAUDE_CONFIG_DIR")
                    .map(PathBuf::from)
                    .or_else(|| home.as_ref().map(|h| Path::new(h).join(".claude")));
                punchlist_runner::move_state_dir(&from, &runner.state_dir, claude_dir.as_deref())
                    .map_err(anyhow::Error::msg)?;
                eprintln!("moved {} to {}", from.display(), runner.state_dir.display());
            }
            punchlist_runner::run(runner, shutdown_signal()).await
        }
        Command::Proof(ProofCommand::Attach {
            id,
            path,
            title,
            kind,
        }) => {
            let proof = client.attach_proof(&id, &path, &title, kind).await?;
            writeln!(
                out,
                "Attached {}: {} {:?} ({}) to {} for attempt {}",
                proof.id,
                proof.kind.as_str(),
                proof.title,
                format_size(proof.size),
                proof.issue_id,
                proof.attempt
            )?;
            Ok(())
        }
        Command::Proof(ProofCommand::List { id }) => {
            let proof = client.list_proof(&id).await?;
            if proof.is_empty() {
                writeln!(out, "No proof attached to {id}.")?;
            }
            for proof in &proof {
                writeln!(out, "{}", proof_line(proof))?;
            }
            Ok(())
        }
        Command::PullRequest(PullRequestCommand::Unlinked) => {
            let pull_requests = client.unlinked_pull_requests().await?;
            write!(out, "{}", render_unlinked(&pull_requests))?;
            Ok(())
        }
        Command::Project(ProjectCommand::Create { name }) => {
            let project = client.create_project(&CreateProject { name }).await?;
            writeln!(out, "Created project {}.", project.name)?;
            Ok(())
        }
        Command::Project(ProjectCommand::List) => {
            let projects = client.list_projects().await?.projects;
            write!(out, "{}", render_projects(&projects))?;
            Ok(())
        }
        Command::Milestone(MilestoneCommand::Create {
            name,
            project,
            pass_when,
        }) => {
            let projects = client.list_projects().await?.projects;
            let project = find_project(&projects, &project)?;
            let request = CreateMilestone { name, pass_when };
            let id = project.id.to_string();
            let milestone = client.create_milestone(&id, &request).await?;
            writeln!(
                out,
                "Created milestone {} in {}.",
                milestone.name, project.name
            )?;
            Ok(())
        }
        Command::Import(ImportCommand::Linear { team, project }) => {
            import_linear(&client, &team, &project, workspace(), out).await
        }
        Command::Import(ImportCommand::Status) => {
            let status = client.import_status(workspace()).await?;
            write!(out, "{}", render_import_status(&status))?;
            Ok(())
        }
        Command::Search { query, limit } => {
            let results = client.search(&query.join(" "), limit).await?;
            write!(out, "{}", render_search(&results))?;
            Ok(())
        }
        Command::Workflow(WorkflowCommand::Show) => {
            write!(out, "{}", render_workflow(&client.workflow().await?))?;
            Ok(())
        }
        Command::Mcp(_) => unreachable!("handled above"),
        Command::Run(RunCommand::List { issue }) => {
            write!(
                out,
                "{}",
                render_run_list(&client.issue_runs(&issue).await?)
            )?;
            Ok(())
        }
        Command::Run(RunCommand::Log { run_id }) => {
            let log = client.run_log(&run_id).await?;
            for line in &log.lines {
                writeln!(out, "{}", line.line)?;
            }
            Ok(())
        }
    }
}

/// Whether the command acts on a workspace rather than on an issue or a run, so `pl` names
/// the workspace in its routes.
fn workspace_level(command: &Command) -> bool {
    match command {
        Command::Issue(command) => {
            matches!(command, IssueCommand::Create { .. } | IssueCommand::List)
        }
        Command::Actor(_)
        | Command::Runner(_)
        | Command::PullRequest(_)
        | Command::Project(_)
        | Command::Milestone(_)
        | Command::Workflow(_)
        | Command::Import(_)
        | Command::Search { .. } => true,
        Command::Workspace(_) | Command::Run(_) | Command::Proof(_) | Command::Mcp(_) => false,
    }
}

/// The workspace `named` by prefix (any case) or id, else the account's only one.
fn pick_workspace<'a>(
    workspaces: &'a [Workspace],
    named: Option<&str>,
) -> anyhow::Result<&'a Workspace> {
    let prefixes = || {
        let mut prefixes: Vec<&str> = workspaces.iter().map(|w| w.issue_prefix.as_str()).collect();
        prefixes.sort_unstable();
        prefixes.join(", ")
    };
    match (named, workspaces) {
        (Some(named), _) => (workspaces.iter())
            .find(|w| w.issue_prefix.eq_ignore_ascii_case(named) || w.id.to_string() == named)
            .ok_or_else(|| anyhow::anyhow!("no workspace {named} in your account: {}", prefixes())),
        (None, [only]) => Ok(only),
        (None, _) => anyhow::bail!("name a workspace with --workspace: {}", prefixes()),
    }
}

/// `pl workspace list`: prefix, name and repository, `*` on the one `pl` uses.
fn render_workspaces(workspaces: &[Workspace], chosen: Option<&Workspace>) -> String {
    let prefix_width = workspaces
        .iter()
        .map(|w| w.issue_prefix.len())
        .max()
        .unwrap_or(0);
    let name_width = workspaces.iter().map(|w| w.name.len()).max().unwrap_or(0);
    workspaces
        .iter()
        .map(|w| {
            format!(
                "{} {:prefix_width$}  {:name_width$}  {}\n",
                if chosen == Some(w) { '*' } else { ' ' },
                w.issue_prefix,
                w.name,
                w.repository
            )
        })
        .collect()
}

/// How long `pl import linear` waits for the server's job.
const IMPORT_WAIT: Duration = Duration::from_secs(15 * 60);

async fn import_linear(
    client: &Client,
    team: &str,
    project: &str,
    workspace: &str,
    out: &mut dyn Write,
) -> anyhow::Result<()> {
    let started = std::time::Instant::now();
    let snapshot = linear::Linear::from_env()?.read(team, project).await?;
    let comments: usize = snapshot.issues.iter().map(|i| i.comments.len()).sum();
    writeln!(
        out,
        "Read {}/{} from Linear: {} issues, {} comments, {} milestones in {}.",
        team,
        snapshot.project.name,
        snapshot.issues.len(),
        comments,
        snapshot.project.milestones.len(),
        format_duration(Some(started.elapsed().as_millis() as i64)),
    )?;
    let mut run = client.import_linear(workspace, &snapshot).await?;
    let mut warned = None;
    while matches!(run.state, ImportState::Queued | ImportState::Running) {
        anyhow::ensure!(
            started.elapsed() < IMPORT_WAIT,
            "import {} is still {} after {}; `pl import status` shows how it ends",
            run.id,
            run.state.as_str(),
            format_duration(Some(IMPORT_WAIT.as_millis() as i64)),
        );
        if run.error.is_some() && run.error != warned {
            eprintln!(
                "import {} failed an attempt and will retry: {}",
                run.id,
                run.error.as_deref().unwrap_or_default()
            );
            warned = run.error.clone();
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        run = client.import_run(workspace, run.id).await?;
    }
    let elapsed = format_duration(Some(started.elapsed().as_millis() as i64));
    match (run.state, &run.report) {
        (ImportState::Done, Some(report)) => {
            writeln!(out, "Import {} done.", run.id)?;
            write!(out, "{}", render_import_report(report))?;
            writeln!(out, "Elapsed {elapsed}.")?;
            Ok(())
        }
        _ => anyhow::bail!(
            "import {} failed after {elapsed}: {}",
            run.id,
            run.error.as_deref().unwrap_or("no reason recorded")
        ),
    }
}

/// What one import created and updated per record type, then its notes.
fn render_import_report(report: &ImportReport) -> String {
    let (c, u) = (&report.created, &report.updated);
    let rows = [
        ("Issues", c.issues, u.issues),
        ("Comments", c.comments, u.comments),
        ("Projects", c.projects, u.projects),
        ("Milestones", c.milestones, u.milestones),
        ("People", c.people, u.people),
    ];
    let mut text = format!("{:<12}{:>8}{:>9}\n", "", "Created", "Updated");
    for (name, created, updated) in rows {
        text.push_str(&format!("{name:<12}{created:>8}{updated:>9}\n"));
    }
    if !report.notes.is_empty() {
        text.push_str("Notes\n");
        for note in &report.notes {
            text.push_str(&format!("  {note}\n"));
        }
    }
    text
}

/// `pl import status`: the totals, then the last import.
fn render_import_status(status: &ImportStatus) -> String {
    let t = &status.totals;
    let mut text = String::from("Imported from Linear\n");
    for (name, count) in [
        ("Issues", t.issues),
        ("Comments", t.comments),
        ("Projects", t.projects),
        ("Milestones", t.milestones),
        ("People", t.people),
    ] {
        text.push_str(&format!("  {name:<12}{count}\n"));
    }
    let Some(run) = &status.last_run else {
        text.push_str("No imports yet.\n");
        return text;
    };
    let at = run.finished_at.unwrap_or(run.created_at);
    text.push_str(&format!(
        "Last import {}  {}  {}\n",
        run.id,
        run.state.as_str(),
        at.format("%Y-%m-%d %H:%M:%S UTC")
    ));
    if let Some(error) = &run.error {
        text.push_str(&format!("Error: {error}\n"));
    }
    if let Some(report) = &run.report {
        text.push_str(&render_import_report(report));
    }
    text
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

/// Flags, then the `[runner]` table, then defaults. With the default state directory, also
/// the old default to move there first, when it `exists` and the new one does not: the state
/// directory was `~/.local/share/punchlist/worktrees` until it held more than worktrees.
pub fn runner_config(
    config: &Config,
    flags: RunnerStart,
    home: Option<&str>,
    hostname: &str,
    exists: impl Fn(&Path) -> bool,
) -> (punchlist_runner::RunnerConfig, Option<PathBuf>) {
    let table = &config.runner;
    let chosen = flags
        .state_dir
        .or(flags.worktree_root)
        .or_else(|| table.state_dir.clone());
    let state_dir = expand_home(
        chosen
            .as_deref()
            .unwrap_or("~/.local/share/punchlist/runner"),
        home,
    );
    let old = expand_home("~/.local/share/punchlist/worktrees", home);
    let move_from = (chosen.is_none() && !exists(&state_dir) && exists(&old)).then_some(old);
    let claude_command = flags
        .claude_command
        .or_else(|| table.claude_command.clone())
        .unwrap_or_else(|| "claude".to_string());
    let codex_command = flags
        .codex_command
        .or_else(|| table.codex_command.clone())
        .unwrap_or_else(|| "codex".to_string());
    let runner = punchlist_runner::RunnerConfig {
        server_url: config.server_url.clone(),
        token: config.token.clone(),
        workspace: None,
        name: flags
            .name
            .or_else(|| table.name.clone())
            .unwrap_or_else(|| hostname.to_string()),
        agents: flags
            .agents
            .or_else(|| table.agents.clone())
            .unwrap_or_else(|| vec!["claude-code".to_string()]),
        state_dir,
        claude_command: expand_home(&claude_command, home),
        mcp_command: std::env::current_exe().unwrap_or_else(|_| PathBuf::from("pl")),
        model: flags.model.or_else(|| table.model.clone()),
        codex_command: expand_home(&codex_command, home),
        codex_model: flags.codex_model.or_else(|| table.codex_model.clone()),
        codex_effort: flags
            .codex_effort
            .or_else(|| table.codex_effort.clone())
            .unwrap_or_else(|| "medium".to_string()),
        max_concurrent: flags.max_concurrent.or(table.max_concurrent).unwrap_or(1),
        heartbeat_interval: Duration::from_secs(10),
        claim_wait: Duration::from_secs(20),
    };
    (runner, move_from)
}

fn render_created(created: &CreatedAgent) -> String {
    let actor = &created.actor;
    let mut text = format!(
        "Created agent {}\nId     {}\nKind   {}\n",
        actor.name,
        actor.id,
        actor.kind.as_deref().unwrap_or("-"),
    );
    if let Some(model) = &actor.model {
        text.push_str(&format!("Model  {model}\n"));
    }
    text.push_str(&format!(
        "Owner  {}\nToken  {}\nThe token is shown once; only its hash is stored.\n",
        actor.owner.as_deref().unwrap_or("-"),
        created.token,
    ));
    text
}

fn render_actors(actors: &[ActorInfo]) -> String {
    let rows: Vec<[String; 6]> = actors
        .iter()
        .map(|a| {
            let or_dash = |value: &Option<String>| value.clone().unwrap_or_else(|| "-".into());
            [
                a.name.clone(),
                a.role.to_string(),
                or_dash(&a.kind),
                or_dash(&a.model),
                or_dash(&a.owner),
                a.id.to_string(),
            ]
        })
        .collect();
    let header = ["NAME", "ROLE", "KIND", "MODEL", "OWNER", "ID"].map(String::from);
    let widths: Vec<usize> = (0..6)
        .map(|i| {
            std::iter::once(&header)
                .chain(&rows)
                .map(|row| row[i].len())
                .max()
                .unwrap_or(0)
        })
        .collect();
    std::iter::once(&header)
        .chain(&rows)
        .map(|row| {
            let cells: Vec<String> = row
                .iter()
                .zip(&widths)
                .map(|(cell, width)| format!("{cell:width$}"))
                .collect();
            format!("{}\n", cells.join("  ").trim_end())
        })
        .collect()
}

/// How the timeline names an actor: `Codex (agent, codex)`, `Gannon (person)`.
fn actor_marking(actor: &punchlist_api::Actor) -> String {
    match &actor.kind {
        Some(kind) => format!("{} ({}, {kind})", actor.name, actor.role),
        None => format!("{} ({})", actor.name, actor.role),
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
            let paused: String = (r.paused.iter())
                .map(|pause| {
                    let until = pause.until.format("%Y-%m-%d %H:%M:%S UTC");
                    format!("  {} paused until {until}", pause.agent)
                })
                .collect();
            format!(
                "{:name_width$}  {:agents_width$}  heartbeat {} ago{paused}\n",
                r.name,
                agents,
                format_age((now - r.last_heartbeat).num_seconds()),
            )
        })
        .collect()
}

/// `512 B`, `12.3 kB` or `4.0 MB`, in powers of 1000.
fn format_size(bytes: i64) -> String {
    let size = bytes as f64;
    if bytes < 1_000 {
        format!("{bytes} B")
    } else if bytes < 1_000_000 {
        format!("{:.1} kB", size / 1e3)
    } else if bytes < 1_000_000_000 {
        format!("{:.1} MB", size / 1e6)
    } else {
        format!("{:.1} GB", size / 1e9)
    }
}

/// One proof record: when, kind, title, size, uploader and id. The title is its uploader's
/// text, quoted so it stays on the line.
fn proof_line(proof: &Proof) -> String {
    format!(
        "{}  {}  {:?}  {}  {}  {}",
        proof.created_at.format("%Y-%m-%d %H:%M:%S UTC"),
        proof.kind.as_str(),
        proof.title,
        format_size(proof.size),
        actor_marking(&proof.actor),
        proof.id
    )
}

/// The `Proof` section of `pl issue show`.
fn render_proof(proof: &[Proof]) -> String {
    let mut text = String::from("\nProof\n");
    if proof.is_empty() {
        text.push_str("  None attached.\n");
    }
    for proof in proof {
        text.push_str(&format!("  {}\n", proof_line(proof)));
    }
    text
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

/// The model an attempt ran on, as the issue page's run card says it: what the agent
/// reported, beside what the runner pinned, if anything.
fn model_label(pinned: Option<&str>, reported: Option<&str>, running: bool) -> String {
    match (reported, pinned) {
        (Some(reported), None) => format!("{reported} (agent default)"),
        (Some(reported), Some(pinned)) if pinned == reported => format!("{reported} (pinned)"),
        (Some(reported), Some(pinned)) => format!("{reported} (pinned {pinned})"),
        (None, None) if running => "waiting for the agent".into(),
        (None, Some(pinned)) if running => format!("waiting for the agent, pinned {pinned}"),
        (None, None) => "agent default".into(),
        (None, Some(pinned)) => format!("{pinned} (pinned)"),
    }
}

/// A run and its latest attempt on one line, indented two spaces.
fn run_line(run: &Run) -> String {
    let mut text = format!(
        "  {}  {}  model {}  runner {}  started {}  duration {}  {}",
        run.id,
        run.agent_name,
        model_label(
            run.model.as_deref(),
            run.reported_model.as_deref(),
            run.outcome == RunOutcome::Running
        ),
        run.runner.name,
        run.started_at.format("%Y-%m-%d %H:%M:%S UTC"),
        format_duration(run.duration_ms),
        run.outcome.as_str(),
    );
    if let (Some(input), Some(output)) = (run.input_tokens, run.output_tokens) {
        text.push_str(&format!("  tokens {input} in / {output} out"));
    }
    if let Some(cost) = &run.cost_usd {
        text.push_str(&format!("  cost ${cost}"));
    }
    if run.outcome == RunOutcome::Failed
        && let Some(reason) = &run.failure_reason
    {
        text.push_str(&format!("  ({reason})"));
    }
    text.push('\n');
    text
}

/// `pl run list`: each run, and under it each attempt with its runner, outcome and reason.
fn render_run_list(runs: &[Run]) -> String {
    let mut text = String::from("Runs\n");
    if runs.is_empty() {
        text.push_str("  No runs yet.\n");
    }
    for run in runs {
        text.push_str(&run_line(run));
        for attempt in &run.attempts {
            text.push_str(&format!(
                "    attempt {}  model {}  runner {}  {}",
                attempt.attempt,
                model_label(
                    attempt.model.as_deref(),
                    attempt.reported_model.as_deref(),
                    attempt.state == RunOutcome::Running
                ),
                attempt.runner.name,
                attempt.state.as_str()
            ));
            if let Some(reason) = &attempt.failure_reason {
                text.push_str(&format!("  ({reason})"));
            }
            text.push('\n');
        }
    }
    text
}

fn render_runs(runs: &[Run]) -> String {
    let mut text = String::from("\nRuns\n");
    if runs.is_empty() {
        text.push_str("  No runs yet.\n");
    }
    for run in runs {
        text.push_str(&run_line(run));
        for line in &run.log_tail {
            text.push_str(&format!("    {}\n", truncate_line(&line.line)));
        }
    }
    text
}

async fn issue(client: &Client, command: IssueCommand, out: &mut dyn Write) -> anyhow::Result<()> {
    match command {
        IssueCommand::Create {
            title,
            body,
            body_file,
        } => {
            let body = match body_file {
                Some(path) => read_body_file(&path)?,
                None => body,
            };
            let written = client.create_issue(&CreateIssue { title, body }).await?;
            writeln!(out, "{}", written.row.issue.id)?;
        }
        IssueCommand::Show { id } => {
            let (issue, events) = issue_with_timeline(client, &id).await?;
            let pull_requests = client.issue_pull_requests(&id).await?;
            let proof = client.list_proof(&id).await?;
            let gates = client.issue_gates(&id).await?;
            let runs = client.issue_runs(&id).await?;
            write!(
                out,
                "{}{}",
                render_show(&issue, &pull_requests, &proof, gates.as_ref(), &events),
                render_runs(&runs)
            )?;
        }
        IssueCommand::List => {
            let list = client.list_issues().await?;
            let issues: Vec<Issue> = list.issues.into_iter().map(|row| row.issue).collect();
            write!(out, "{}", render_list(&issues))?;
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
            if let Some(problem) = &moved.warning {
                writeln!(
                    out,
                    "warning: no runner can claim {} under the active workflow: {problem}",
                    moved.issue.id
                )?;
            }
        }
        IssueCommand::Comment { id, body } => {
            client.comment(&id, &body).await?;
            writeln!(out, "Commented on {id}.")?;
        }
        IssueCommand::Edit { id, edit } => {
            let request = issue_edit(client, &id, edit).await?;
            let issue = client.update_issue(&id, &request).await?.row.issue;
            let none = || "none".to_string();
            writeln!(
                out,
                "{}  {}\n  labels {} · parent {} · project {} · milestone {}",
                issue.id,
                issue.title,
                Some(issue.labels.join(", "))
                    .filter(|l| !l.is_empty())
                    .unwrap_or_else(none),
                issue.parent.unwrap_or_else(none),
                issue.project.map_or_else(none, |p| p.name),
                issue.milestone.map_or_else(none, |m| m.name),
            )?;
        }
    }
    Ok(())
}

/// The text of `--body-file`: the file, or stdin for `-`.
fn read_body_file(path: &std::path::Path) -> anyhow::Result<String> {
    if path.as_os_str() == "-" {
        return Ok(std::io::read_to_string(std::io::stdin())?);
    }
    std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", path.display()))
}

/// A project by its name.
fn find_project<'a>(projects: &'a [Project], name: &str) -> anyhow::Result<&'a Project> {
    (projects.iter())
        .find(|project| project.name == name)
        .ok_or_else(|| anyhow::anyhow!("no project named {name} in this workspace"))
}

/// The `PATCH` for `pl issue edit`, with the body read from `--body-file` and project and milestone names turned into their ids.
/// A milestone's name is looked up in `--project`, else in the issue's project.
async fn issue_edit(client: &Client, id: &str, edit: IssueEdit) -> anyhow::Result<UpdateIssue> {
    let body = match &edit.body_file {
        Some(path) => Some(read_body_file(path)?),
        None => edit.body,
    };
    let labels = (!edit.label.is_empty() || edit.no_labels).then_some(edit.label);
    let mut request = UpdateIssue {
        title: edit.title,
        body,
        labels,
        parent: edit.parent.map(Some).or(edit.no_parent.then_some(None)),
        project: edit.no_project.then_some(None),
        milestone: edit.no_milestone.then_some(None),
        ..UpdateIssue::default()
    };
    if edit.project.is_none() && edit.milestone.is_none() {
        return Ok(request);
    }
    // The projects of the issue's workspace.
    let prefix = id.rsplit_once('-').map_or(id, |(prefix, _)| prefix);
    let workspaces = client.workspaces().await?;
    let projects = match pick_workspace(&workspaces, Some(prefix)) {
        Ok(workspace) => client.clone().in_workspace(&workspace.id.to_string()),
        Err(_) => client.clone(),
    };
    let projects = projects.list_projects().await?.projects;
    let project = match &edit.project {
        Some(name) => {
            let project = find_project(&projects, name)?;
            request.project = Some(Some(project.id));
            project
        }
        None => {
            let current = client.get_issue(id).await?.project;
            let current = current.ok_or_else(|| {
                anyhow::anyhow!("{id} has no project; give --project with --milestone")
            })?;
            find_project(&projects, &current.name)?
        }
    };
    if let Some(name) = &edit.milestone {
        let milestone = (project.milestones.iter())
            .find(|milestone| milestone.name == *name)
            .ok_or_else(|| {
                anyhow::anyhow!("project {} has no milestone named {name}", project.name)
            })?;
        request.milestone = Some(Some(milestone.id));
    }
    Ok(request)
}

/// A refused move whose gates ran lists every gate's result under the message.
fn with_gates(error: ClientError) -> anyhow::Error {
    let ClientError::Refused(body) = &error else {
        return error.into();
    };
    if body.gates.is_empty() {
        return error.into();
    }
    anyhow::anyhow!("{error}\n{}", body.gate_lines().join("\n"))
}

/// `pl workflow show`.
fn render_workflow(workflow: &WorkflowInfo) -> String {
    let invalid = workflow.invalid.as_ref().map(|problem| {
        format!("Invalid   {problem}; in force until a valid .punchlist/workflow.toml is pushed\n")
    });
    let mut out = format!(
        "Version   {}\n{}Commit    {}\nStatuses  {}\n",
        workflow.version,
        invalid.unwrap_or_default(),
        workflow.commit_sha.as_deref().unwrap_or("built-in"),
        (workflow.statuses.iter())
            .map(|status| status.key.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    if let Some(refused) = &workflow.refused {
        out += &format!("Refused   {}: {}\n", refused.commit_sha, refused.reason);
    }
    out
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
            EventDetail::Comment { .. }
            | EventDetail::Refusal { .. }
            | EventDetail::Created
            | EventDetail::Edited { .. }
            | EventDetail::Run
            | EventDetail::PullRequest
            | EventDetail::Proof
            | EventDetail::Workflow => None,
        });
        let consistent = last_move.is_none_or(|to| *to == issue.status);
        attempts += 1;
        if consistent || attempts == 3 {
            return Ok((issue, events));
        }
    }
}

fn render_show(
    issue: &Issue,
    pull_requests: &[PullRequest],
    proof: &[Proof],
    gates: Option<&GateChecklist>,
    events: &[Event],
) -> String {
    let mut text = format!(
        "{}  {}\nStatus: {}\n",
        issue.id, issue.title, issue.status_name
    );
    if !issue.labels.is_empty() {
        text.push_str(&format!("Labels: {}\n", issue.labels.join(", ")));
    }
    if let Some(parent) = &issue.parent {
        text.push_str(&format!("Parent: {parent}\n"));
    }
    if let Some(project) = &issue.project {
        text.push_str(&format!("Project: {}\n", project.name));
    }
    if let Some(milestone) = &issue.milestone {
        text.push_str(&format!("Milestone: {}\n", milestone.name));
    }
    if let Some(linear) = &issue.linear {
        text.push_str(&format!("Linear: {}  {}\n", linear.identifier, linear.url));
    }
    if let Some(why) = &issue.waiting_on_person {
        text.push_str(&format!("waiting on a person: {why}\n"));
    }
    if let Some(wait) = &issue.waiting_for_usage_limit {
        text.push_str(&format!("{wait}\n"));
    }
    if !issue.body.is_empty() {
        text.push('\n');
        text.push_str(issue.body.trim_end());
        text.push('\n');
    }
    text.push_str(&render_pull_requests(pull_requests));
    text.push_str(&render_proof(proof));
    if let Some(gates) = gates {
        text.push_str(&render_gates(gates));
    }
    text.push_str("\nTimeline\n");
    if events.is_empty() {
        text.push_str("  No transitions yet.\n");
    }
    for event in events {
        let who = format!(
            "  {}  {}  ",
            event.created_at.format("%Y-%m-%d %H:%M:%S UTC"),
            actor_marking(&event.actor),
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
            EventDetail::Refusal {
                from_name,
                to_name,
                gate,
                reason,
                delivery_id,
                ..
            } => {
                text.push_str(&format!(
                    "{who}refused {from_name} → {to_name}: gate {gate} failed: {reason}"
                ));
                if let Some(delivery_id) = delivery_id {
                    text.push_str(&format!("  [delivery {delivery_id}]"));
                }
                text.push('\n');
            }
            EventDetail::Created => text.push_str(&format!("{who}created\n")),
            EventDetail::Edited { fields } => {
                text.push_str(&format!("{who}edited {}\n", fields.join(", ")));
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
            // Only the event stream carries these; a timeline has none.
            EventDetail::Run
            | EventDetail::PullRequest
            | EventDetail::Proof
            | EventDetail::Workflow => {}
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

/// The `Gates` section of `pl issue show`: the checklist for the issue's next gated
/// transition, then how many pass.
fn render_gates(checklist: &GateChecklist) -> String {
    let passed = checklist.gates.iter().filter(|gate| gate.passed).count();
    format!(
        "\nGates for {} → {}\n{}\n  {passed} of {} gates pass\n",
        checklist.from_name,
        checklist.to_name,
        gate_lines(&checklist.gates).join("\n"),
        checklist.gates.len(),
    )
}

/// `pl project list`.
fn render_projects(projects: &[Project]) -> String {
    if projects.is_empty() {
        return "No projects.\n".to_string();
    }
    let mut text = String::new();
    for project in projects {
        text.push_str(&format!("{}\n", project.name));
        if project.milestones.is_empty() {
            text.push_str("  No milestones.\n");
        }
        for milestone in &project.milestones {
            text.push_str(&format!("  {}", milestone.name));
            if !milestone.pass_when.is_empty() {
                text.push_str(&format!("  PASS when {}", milestone.pass_when));
            }
            text.push('\n');
        }
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

fn render_search(results: &SearchResults) -> String {
    if results.hits.is_empty() {
        return "No matches.\n".to_string();
    }
    let mut text = String::new();
    for hit in &results.hits {
        // The server makes a snippet one line; a title is as its author wrote it.
        let title: String = (hit.title.chars())
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect();
        text.push_str(&format!("{}  {title}\n", hit.issue_id));
        if let Some(s) = &hit.snippet {
            let field = s.field.as_str();
            text.push_str(&format!(
                "    {field}: {}[{}]{}\n",
                s.before, s.hit, s.after
            ));
        }
    }
    if results.more {
        text.push_str("… more matches; narrow the query\n");
    }
    text
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
            check_runs: Vec::new(),
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
                kind: None,
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
    fn a_runner_shows_each_agent_it_has_paused_with_the_time() {
        let now: DateTime<Utc> = "2026-10-23T17:15:00Z".parse().unwrap();
        let runner = Runner {
            id: "00000000-0000-0000-0000-000000000000".parse().unwrap(),
            name: "sartre".into(),
            agents: vec!["claude-code".into(), "codex".into()],
            registered_at: now,
            last_heartbeat: now,
            paused: vec![punchlist_api::AgentPause {
                agent: "claude-code".into(),
                until: "2026-10-23T18:20:00Z".parse().unwrap(),
            }],
        };
        assert_eq!(
            render_runners(&[runner], now),
            "sartre  claude-code,codex  heartbeat 0s ago  claude-code paused until 2026-10-23 18:20:00 UTC\n"
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
            assignee: None,
            labels: Vec::new(),
            parent: None,
            project: None,
            milestone: None,
            waiting_on_person: Some("3 runs in Agent Review ended without moving it".into()),
            waiting_for_usage_limit: None,
            linear: None,
        };
        let mut moved = EventDetail::transition(
            "in_progress".into(),
            "agent_review".into(),
            "sha256:ab".into(),
        );
        if let EventDetail::Transition { delivery_id, .. } = &mut moved {
            *delivery_id = Some("d-1".into());
        }
        let gate = |gate: &str, passed, reason: &str| punchlist_api::GateResult {
            gate: gate.into(),
            passed,
            reason: reason.into(),
            outside: Vec::new(),
            outside_more: 0,
        };
        let gates = GateChecklist {
            from: "agent_review".into(),
            from_name: "Agent Review".into(),
            to: "human_review".into(),
            to_name: "Human Review".into(),
            gates: vec![
                gate(
                    "ci_green",
                    false,
                    "1 of 3 checks failed on 3f2a1c4: `e2e` (failure)",
                ),
                gate(
                    "mergeable",
                    true,
                    "pull request #12 has no conflicts and GitHub's merge state is clean",
                ),
            ],
        };
        let events = [
            event(punchlist_api::Role::Person, "Gannon", EventDetail::Created),
            event(
                punchlist_api::Role::Person,
                "Gannon",
                EventDetail::Edited {
                    fields: vec!["title".into(), "labels".into()],
                },
            ),
            event(punchlist_api::Role::Github, "gannonh", moved),
            event(
                punchlist_api::Role::Github,
                "gannonh",
                EventDetail::Comment {
                    body: "Closed without merging.\n\nSee #12.\n".into(),
                },
            ),
            event(
                punchlist_api::Role::Github,
                "gannonh",
                EventDetail::Refusal {
                    from: "merging".into(),
                    from_name: "Merging".into(),
                    to: "done".into(),
                    to_name: "Done".into(),
                    gate: "proof_attached".into(),
                    reason: "no proof".into(),
                    workflow_version: "sha256:ab".into(),
                    delivery_id: Some("d-2".into()),
                },
            ),
        ];
        assert_eq!(
            render_show(&issue, &[], &[], Some(&gates), &events),
            "PL-7  Fix\nStatus: Agent Review\n\
             waiting on a person: 3 runs in Agent Review ended without moving it\n\
             \nPull requests\n  None linked.\n\
             \nProof\n  None attached.\n\
             \nGates for Agent Review → Human Review\n\
             \x20 ci_green: fail (1 of 3 checks failed on 3f2a1c4: `e2e` (failure))\n\
             \x20 mergeable: pass (pull request #12 has no conflicts and GitHub's merge state is clean)\n\
             \x20 1 of 2 gates pass\n\
             \nTimeline\n\
             \x20 2026-10-10 12:30:45 UTC  Gannon (person)  created\n\
             \x20 2026-10-10 12:30:45 UTC  Gannon (person)  edited title, labels\n\
             \x20 2026-10-10 12:30:45 UTC  gannonh (github)  In Progress → Agent Review  [delivery d-1]\n\
             \x20 2026-10-10 12:30:45 UTC  gannonh (github)  commented:\n\
             \x20   Closed without merging.\n\
             \n\
             \x20   See #12.\n\
             \x20 2026-10-10 12:30:45 UTC  gannonh (github)  refused Merging → Done: gate proof_attached failed: no proof  [delivery d-2]\n"
        );
    }

    #[test]
    fn timeline_marks_each_role_and_an_agent_kind() {
        let comment = || EventDetail::Comment { body: "Hi".into() };
        let mut codex = event(punchlist_api::Role::Agent, "Codex", comment());
        codex.actor.kind = Some("codex".into());
        let events = [
            event(punchlist_api::Role::Person, "Gannon", comment()),
            codex,
            event(punchlist_api::Role::Runner, "sartre", comment()),
            event(punchlist_api::Role::Github, "gannonh", comment()),
        ];
        let issue = Issue {
            id: "PL-7".into(),
            title: "Fix".into(),
            body: String::new(),
            status: "todo".into(),
            status_name: "Todo".into(),
            created_at: "2026-10-10T12:00:00Z".parse().unwrap(),
            updated_at: "2026-10-10T12:00:00Z".parse().unwrap(),
            assignee: None,
            labels: Vec::new(),
            parent: None,
            project: None,
            milestone: None,
            waiting_on_person: None,
            waiting_for_usage_limit: None,
            linear: None,
        };
        let text = render_show(&issue, &[], &[], None, &events);
        let who: Vec<&str> = text
            .lines()
            .filter_map(|line| line.strip_prefix("  2026-10-10 12:30:45 UTC  "))
            .collect();
        assert_eq!(
            who,
            [
                "Gannon (person)  commented:",
                "Codex (agent, codex)  commented:",
                "sartre (runner)  commented:",
                "gannonh (github)  commented:",
            ]
        );
    }

    #[test]
    fn renders_actors() {
        let actor = |name: &str, role, kind: Option<&str>, owner: Option<&str>| ActorInfo {
            id: "00000000-0000-0000-0000-000000000000".parse().unwrap(),
            name: name.into(),
            role,
            kind: kind.map(Into::into),
            model: None,
            owner: owner.map(Into::into),
            created_at: "2026-10-10T12:00:00Z".parse().unwrap(),
        };
        let mut codex = actor(
            "Codex",
            punchlist_api::Role::Agent,
            Some("codex"),
            Some("Gannon"),
        );
        codex.model = Some("gpt-6-luna".into());
        let id = "00000000-0000-0000-0000-000000000000";
        assert_eq!(
            render_actors(&[
                actor("Gannon", punchlist_api::Role::Person, None, None),
                codex.clone(),
            ]),
            format!(
                "NAME    ROLE    KIND   MODEL       OWNER   ID\n\
                 Gannon  person  -      -           -       {id}\n\
                 Codex   agent   codex  gpt-6-luna  Gannon  {id}\n"
            )
        );
        let created = CreatedAgent {
            actor: codex,
            token: "plt_x".into(),
        };
        assert_eq!(
            render_created(&created),
            format!(
                "Created agent Codex\nId     {id}\nKind   codex\nModel  gpt-6-luna\n\
                 Owner  Gannon\nToken  plt_x\nThe token is shown once; only its hash is stored.\n"
            )
        );
    }

    #[test]
    fn renders_the_workflow() {
        let info = WorkflowInfo {
            version: "sha256:ab".into(),
            invalid: None,
            commit_sha: None,
            statuses: ["backlog", "todo"]
                .map(|key| punchlist_api::StatusInfo {
                    key: key.into(),
                    name: key.into(),
                    locked_to: Vec::new(),
                    dispatch: None,
                    closed: false,
                    on_merge_to: None,
                })
                .to_vec(),
            refused: None,
        };
        assert_eq!(
            render_workflow(&info),
            "Version   sha256:ab\nCommit    built-in\nStatuses  backlog, todo\n"
        );
        let info = WorkflowInfo {
            invalid: Some("line 4, `statuses`: the runtime needs the status `start`".into()),
            refused: Some(punchlist_api::WorkflowRefusal {
                commit_sha: "abc".into(),
                at: chrono::DateTime::UNIX_EPOCH,
                reason: "line 4, `statuses`: bad".into(),
            }),
            ..info
        };
        assert_eq!(
            render_workflow(&info),
            "Version   sha256:ab\n\
             Invalid   line 4, `statuses`: the runtime needs the status `start`; \
             in force until a valid .punchlist/workflow.toml is pushed\n\
             Commit    built-in\nStatuses  backlog, todo\nRefused   abc: line 4, `statuses`: bad\n"
        );
    }

    #[test]
    fn formats_sizes() {
        assert_eq!(format_size(999), "999 B");
        assert_eq!(format_size(12_345), "12.3 kB");
        assert_eq!(format_size(4_000_000), "4.0 MB");
        assert_eq!(format_size(2_500_000_000), "2.5 GB");
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
    fn run_list_shows_each_attempt_under_its_run() {
        let at = |s: &str| DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc);
        let runner = |name: &str| punchlist_api::RunnerRef {
            id: Default::default(),
            name: name.into(),
        };
        let attempt = |n, name, state, reason: Option<&str>| punchlist_api::AttemptInfo {
            attempt: n,
            runner: runner(name),
            model: Some("haiku".into()),
            reported_model: (n == 1).then(|| "claude-haiku-5-5".into()),
            duration_ms: None,
            input_tokens: None,
            output_tokens: None,
            cost_usd: None,
            state,
            failure_reason: reason.map(String::from),
            started_at: at("2026-10-16T10:00:00Z"),
            finished_at: None,
        };
        let run = Run {
            id: Default::default(),
            issue_id: "PL-1".into(),
            agent: "claude-code".into(),
            agent_name: "Claude Code".into(),
            branch: "feature/pl-1".into(),
            outcome: RunOutcome::Running,
            started_at: at("2026-10-16T10:00:00Z"),
            finished_at: None,
            attempt: 2,
            model: Some("haiku".into()),
            reported_model: None,
            runner: runner("sartre"),
            failure_reason: None,
            duration_ms: None,
            input_tokens: None,
            output_tokens: None,
            cost_usd: None,
            log_tail: vec![],
            attempts: vec![
                attempt(1, "sartre", RunOutcome::Failed, Some("runner restarted")),
                attempt(2, "sartre", RunOutcome::Running, None),
            ],
        };
        assert_eq!(
            render_run_list(std::slice::from_ref(&run)),
            "Runs\n\
             \x20 00000000-0000-0000-0000-000000000000  Claude Code  model waiting for the agent, pinned haiku  runner sartre  started 2026-10-16 10:00:00 UTC  duration —  running\n\
             \x20   attempt 1  model claude-haiku-5-5 (pinned haiku)  runner sartre  failed  (runner restarted)\n\
             \x20   attempt 2  model waiting for the agent, pinned haiku  runner sartre  running\n"
        );
        assert_eq!(
            [
                model_label(None, Some("claude-opus-5-5"), false),
                model_label(Some("gpt-6-luna"), Some("gpt-6-luna"), true),
                model_label(None, None, true),
                model_label(None, None, false),
                model_label(Some("haiku"), None, false),
            ],
            [
                "claude-opus-5-5 (agent default)",
                "gpt-6-luna (pinned)",
                "waiting for the agent",
                "agent default",
                "haiku (pinned)",
            ]
        );
        assert_eq!(render_run_list(&[]), "Runs\n  No runs yet.\n");

        let mut finished = run.clone();
        finished.outcome = RunOutcome::Succeeded;
        finished.input_tokens = Some(1000);
        finished.output_tokens = Some(250);
        assert!(run_line(&finished).ends_with("succeeded  tokens 1000 in / 250 out\n"));
        finished.cost_usd = Some("0.0453".into());
        assert!(
            run_line(&finished).ends_with("succeeded  tokens 1000 in / 250 out  cost $0.0453\n")
        );
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
    fn config_names_a_workspace_and_rejects_unknown_keys() {
        assert_eq!(
            config("workspace = \"GW\"\n").unwrap().workspace.as_deref(),
            Some("GW")
        );
        assert_eq!(config("").unwrap().workspace, None);
        let unknown = config("workspaces = \"GW\"\n").unwrap_err().to_string();
        assert!(unknown.contains("unknown field `workspaces`"), "{unknown}");
    }

    #[test]
    fn picks_and_lists_workspaces() {
        let workspace = |prefix: &str, name: &str, repository: &str| Workspace {
            id: format!(
                "00000000-0000-0000-0000-0000000000{:x}",
                prefix.as_bytes()[0]
            )
            .parse()
            .unwrap(),
            name: name.into(),
            issue_prefix: prefix.into(),
            repository: repository.into(),
        };
        let both = [
            workspace("GW", "Groundwork", "gannonh/groundwork"),
            workspace("PL", "Punchlist", "gannonh/punchlist"),
        ];
        let pick = |named| pick_workspace(&both, named).map(|w| w.issue_prefix.as_str());
        assert_eq!(pick(Some("GW")).unwrap(), "GW");
        assert_eq!(pick(Some("pl")).unwrap(), "PL");
        let id = both[1].id.to_string();
        assert_eq!(pick(Some(id.as_str())).unwrap(), "PL");
        assert_eq!(
            pick(None).unwrap_err().to_string(),
            "name a workspace with --workspace: GW, PL"
        );
        assert_eq!(
            pick(Some("XX")).unwrap_err().to_string(),
            "no workspace XX in your account: GW, PL"
        );
        assert_eq!(pick_workspace(&both[1..], None).unwrap().issue_prefix, "PL");
        assert_eq!(
            render_workspaces(&both, Some(&both[0])),
            "* GW  Groundwork  gannonh/groundwork\n  PL  Punchlist   gannonh/punchlist\n"
        );
    }

    #[test]
    fn runner_config_layers_flags_table_and_defaults() {
        let (defaults, _) = runner_config(
            &config("").unwrap(),
            RunnerStart::default(),
            Some("/h"),
            "box",
            |_| false,
        );
        assert_eq!(defaults.name, "box");
        assert_eq!(defaults.agents, vec!["claude-code".to_string()]);
        assert_eq!(defaults.claude_command, PathBuf::from("claude"));
        assert_eq!(defaults.codex_command, PathBuf::from("codex"));
        assert_eq!(defaults.codex_model, None);
        assert_eq!(defaults.codex_effort, "medium");
        assert_eq!((defaults.model, defaults.max_concurrent), (None, 1));

        let table = config(
            "[runner]\nname = \"t\"\nstate_dir = \"~/wt\"\nmodel = \"haiku\"\nmax_concurrent = 3\n",
        )
        .unwrap();
        let flags = RunnerStart {
            name: Some("f".into()),
            agents: Some(vec!["claude-code".into(), "codex".into()]),
            codex_model: Some("gpt-6-luna".into()),
            codex_effort: Some("low".into()),
            ..RunnerStart::default()
        };
        let (merged, _) = runner_config(&table, flags, Some("/h"), "box", |_| false);
        assert_eq!(merged.name, "f");
        assert_eq!(merged.agents, ["claude-code", "codex"]);
        assert_eq!(merged.codex_model.as_deref(), Some("gpt-6-luna"));
        assert_eq!(merged.codex_effort, "low");
        assert_eq!(merged.state_dir, PathBuf::from("/h/wt"));
        assert_eq!(merged.model.as_deref(), Some("haiku"));
        assert_eq!(merged.max_concurrent, 3);
    }

    #[test]
    fn runner_state_dir_resolves_and_moves_only_the_old_default() {
        fn resolve(
            table: &str,
            flags: RunnerStart,
            existing: &[&str],
        ) -> (PathBuf, Option<PathBuf>) {
            let (runner, move_from) =
                runner_config(&config(table).unwrap(), flags, Some("/h"), "box", |p| {
                    existing.iter().any(|e| p == Path::new(e))
                });
            (runner.state_dir, move_from)
        }
        let old = "/h/.local/share/punchlist/worktrees";
        let new = "/h/.local/share/punchlist/runner";

        // No option.
        assert_eq!(
            resolve("", RunnerStart::default(), &[]),
            (PathBuf::from(new), None)
        );
        // `--state-dir`.
        let state_dir = RunnerStart {
            state_dir: Some("~/state".into()),
            ..RunnerStart::default()
        };
        assert_eq!(
            resolve("", state_dir, &[old]),
            (PathBuf::from("/h/state"), None)
        );
        // `--worktree-root`, the deprecated name.
        let worktree_root = RunnerStart {
            worktree_root: Some("/tmp/x".into()),
            ..RunnerStart::default()
        };
        assert_eq!(
            resolve("", worktree_root, &[old]),
            (PathBuf::from("/tmp/x"), None)
        );
        // The table's old key.
        assert_eq!(
            resolve(
                "[runner]\nworktree_root = \"~/wt\"\n",
                RunnerStart::default(),
                &[old]
            ),
            (PathBuf::from("/h/wt"), None)
        );
        // An existing old default moves to the new one, unless the new one exists.
        assert_eq!(
            resolve("", RunnerStart::default(), &[old]),
            (PathBuf::from(new), Some(PathBuf::from(old)))
        );
        assert_eq!(
            resolve("", RunnerStart::default(), &[old, new]),
            (PathBuf::from(new), None)
        );
    }
}
