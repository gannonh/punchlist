//! `pl`, the Punchlist command line.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Context;
use clap::{Parser, Subcommand};
use punchlist_api::{CreateIssue, Event, EventDetail, Issue};
use punchlist_client::Client;
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
    /// Move an issue to another status, such as `todo` or `in_progress`.
    Move { id: String, status: String },
}

/// `~/.config/punchlist/config.toml`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub server_url: String,
    pub token: String,
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
    let path = match cli.config {
        Some(path) => path,
        None => Config::default_path()?,
    };
    let config = Config::load(&path)?;
    let client = Client::new(&config.server_url, &config.token)?;
    match cli.command {
        Command::Issue(command) => issue(&client, command, out).await,
    }
}

async fn issue(client: &Client, command: IssueCommand, out: &mut dyn Write) -> anyhow::Result<()> {
    match command {
        IssueCommand::Create { title, body } => {
            let issue = client.create_issue(&CreateIssue { title, body }).await?;
            writeln!(out, "{}", issue.id)?;
        }
        IssueCommand::Show { id } => {
            let (issue, events) = issue_with_timeline(client, &id).await?;
            write!(out, "{}", render_show(&issue, &events))?;
        }
        IssueCommand::List => {
            let list = client.list_issues().await?;
            write!(out, "{}", render_list(&list.issues))?;
        }
        IssueCommand::Move { id, status } => {
            let moved = client.move_issue(&id, &status).await?;
            let EventDetail::Transition { from_name, .. } = &moved.event.detail;
            writeln!(
                out,
                "{}  {} → {}",
                moved.issue.id, from_name, moved.issue.status_name
            )?;
        }
    }
    Ok(())
}

/// The issue and its timeline from two requests. A move landing between them would show an
/// old status above a timeline that already has the new move, so read again until the last
/// move matches the status.
async fn issue_with_timeline(client: &Client, id: &str) -> anyhow::Result<(Issue, Vec<Event>)> {
    let mut attempts = 0;
    loop {
        let issue = client.get_issue(id).await?;
        let events = client.issue_events(id).await?;
        let consistent = events.last().is_none_or(|event| {
            let EventDetail::Transition { to, .. } = &event.detail;
            *to == issue.status
        });
        attempts += 1;
        if consistent || attempts == 3 {
            return Ok((issue, events));
        }
    }
}

fn render_show(issue: &Issue, events: &[Event]) -> String {
    let mut text = format!(
        "{}  {}\nStatus: {}\n",
        issue.id, issue.title, issue.status_name
    );
    if !issue.body.is_empty() {
        text.push('\n');
        text.push_str(issue.body.trim_end());
        text.push('\n');
    }
    text.push_str("\nTimeline\n");
    if events.is_empty() {
        text.push_str("  No transitions yet.\n");
    }
    for event in events {
        let EventDetail::Transition {
            from_name, to_name, ..
        } = &event.detail;
        text.push_str(&format!(
            "  {}  {} ({})  {} → {}\n",
            event.created_at.format("%Y-%m-%d %H:%M:%S UTC"),
            event.actor.name,
            event.actor.role,
            from_name,
            to_name,
        ));
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
