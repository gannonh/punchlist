// Copied from kata-symphony apps/symphony/src/codex/app_server.rs at e7c160f5; trimmed: kept the handshake (`initialize`, `initialized`, `thread/start`), `turn/start`, the receive loop with its turn outcomes, auto-approval and the message helpers; dropped the docker and ssh workers, dynamic tools, the `AgentEvent` stream, rate limits, the triage handle, the input-required heuristics and the workspace check (worktree.rs checks the path). Token accounting is not Symphony's `token_accounting.rs`: the app-server reports the thread's running totals, which is all a one-turn thread needs. Written for Punchlist: the process group and stop signal, the run log lines, the answer to requests it does not handle, the per-run `CODEX_HOME`, and the model, effort and MCP server sent in `thread/start`.
//! The Codex adapter: runs `codex app-server` in the worktree, starts a thread and one turn
//! with the prompt over JSON-RPC on its stdio, streams its messages as log lines, and reads
//! the turn's outcome and token counts.
//!
//! Codex has no `--strict-mcp-config`. The run gets only what the runner gives it because
//! `CODEX_HOME` is a directory made for the run, holding nothing but a link to the person's
//! sign-in; apps, plugins and hooks are off; the worktree is an untrusted project, so Codex
//! loads no `.codex/` configuration and no `AGENTS.md` from the repository; and every skill
//! Codex finds, the person's and the repository's, is disabled for the thread.
//!
//! 1. Spawn `<codex> app-server` with the worktree as cwd and the run's `CODEX_HOME`.
//! 2. Handshake: `initialize(id=1)` → await response → `initialized` → `skills/list(id=4)`
//!    → await response → `thread/start(id=2)` → await response.
//! 3. Turn: `turn/start(id=3)` → await response → stream events until `turn/completed`,
//!    failure or timeout.
//! 4. Stop: kill the subprocess and its process group.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use punchlist_api::{Claim, RunOutcome};
use serde_json::{Value, json};
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout, Command};
use tokio::sync::{mpsc, watch};

use crate::RunnerConfig;
use crate::agent::{
    AgentExit, AgentResult, MAX_RECORD_BYTES, RUNNER_MARKER, kill_group, read_record,
};

const INITIALIZE_ID: u64 = 1;
const THREAD_START_ID: u64 = 2;
const TURN_START_ID: u64 = 3;
const SKILLS_LIST_ID: u64 = 4;

/// The name Codex knows Punchlist's MCP server by.
const MCP_SERVER: &str = "punchlist";

/// How long a response in the handshake may take.
const READ_TIMEOUT: Duration = Duration::from_secs(60);
/// The longest a turn may go without a message.
const TURN_TIMEOUT: Duration = Duration::from_secs(3600);

/// Codex's `mcp_servers` config for one run: Punchlist's MCP server for the claimed issue,
/// with the run's agent token. It goes to Codex in `thread/start`, over stdin, so the token
/// is in no file and no process listing.
pub(crate) fn mcp_servers(config: &RunnerConfig, claim: &Claim) -> Value {
    json!({
        MCP_SERVER: {
            "command": config.mcp_command,
            "args": ["mcp"],
            "env": {
                "PUNCHLIST_SERVER_URL": config.server_url,
                "PUNCHLIST_TOKEN": claim.agent_token,
                "PUNCHLIST_ISSUE": claim.issue.id,
            }
        }
    })
}

/// The variable Codex's app-server takes a sign-in from when it is set, before any stored
/// one. It is the only one: the app-server does not read `CODEX_API_KEY` or `OPENAI_API_KEY`.
const SIGN_IN_ENV: &str = "CODEX_ACCESS_TOKEN";

/// The `CODEX_HOME` of one run: a directory only this user can read, holding a link to the
/// person's Codex sign-in and, while the run goes on, Codex's own state (its databases and
/// shell snapshots). A link, not a copy: Codex writes a refreshed token in place, through
/// the link, so the person's sign-in stays current and removing the directory removes no
/// credential. Dropping this removes the directory.
pub(crate) struct CodexHome(PathBuf);

impl CodexHome {
    /// Makes `<root>/codex-home/<run_id>`, outside the worktree, linked to the sign-in in the
    /// person's own Codex home.
    pub(crate) fn create(root: &Path, run_id: &str) -> std::io::Result<CodexHome> {
        let person = person_home(std::env::var_os("CODEX_HOME"), std::env::home_dir())?;
        let env_sign_in = std::env::var_os(SIGN_IN_ENV).is_some_and(|token| !token.is_empty());
        CodexHome::linked_to(&person, &run_home(root, run_id)?, env_sign_in)
    }

    /// The link is made whenever the person has an `auth.json`. Without one the home has no
    /// link, which is enough when `env_sign_in` says the runner's environment signs Codex
    /// in, and is refused otherwise.
    // ponytail: a stored sign-in reaches the run only from `auth.json`. One kept in the
    // keyring (`cli_auth_credentials_store`) has no file to link; have the runner pass the
    // tokens over the app-server's `account/login/start` when someone needs that.
    fn linked_to(person_home: &Path, home: &Path, env_sign_in: bool) -> std::io::Result<CodexHome> {
        use std::os::unix::fs::DirBuilderExt;
        let sign_in = person_home.join("auth.json");
        let stored = sign_in.is_file();
        if !stored && !env_sign_in {
            return Err(std::io::Error::other(format!(
                "no Codex sign-in at {} and no {SIGN_IN_ENV}: run `codex login` as the person \
                 who runs the runner (a sign-in kept in the keyring is not supported)",
                sign_in.display()
            )));
        }
        // What a run killed with its runner left behind.
        let _ = std::fs::remove_dir_all(home);
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(home)?;
        // From here a failure removes the directory.
        let home = CodexHome(home.to_path_buf());
        if stored {
            std::os::unix::fs::symlink(sign_in, home.0.join("auth.json"))?;
        }
        Ok(home)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for CodexHome {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_dir_all(&self.0) {
            tracing::warn!("cannot remove the Codex home {}: {e}", self.0.display());
        }
    }
}

/// Where one run's home goes under the runner's worktree root. Absolute: Codex gets it as
/// `CODEX_HOME` with the worktree as its directory, and a relative root would name another
/// place from there.
fn run_home(root: &Path, run_id: &str) -> std::io::Result<PathBuf> {
    Ok(std::path::absolute(root)?.join("codex-home").join(run_id))
}

/// The person's own Codex home, as Codex finds it: `CODEX_HOME` when it is set and not
/// empty, else `.codex` in the home directory. Absolute, so a link to it holds anywhere.
fn person_home(codex_home: Option<OsString>, home: Option<PathBuf>) -> std::io::Result<PathBuf> {
    let dir = match codex_home.filter(|dir| !dir.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => home
            .ok_or_else(|| std::io::Error::other("HOME is not set"))?
            .join(".codex"),
    };
    std::path::absolute(dir)
}

/// The `thread/start` parameters of one run: the runner's model and reasoning effort, no
/// sandbox or approvals, and `mcp_servers` as the only MCP servers.
// ponytail: `danger-full-access` and no approvals, as the Claude adapter's
// `bypassPermissions`: the agent has to reach git's directory in the clone, the network
// and `gh`. Codex's own `workspace-write` sandbox allows none of the three; give the
// run a container or a sandbox policy with those holes when untrusted issue text needs
// a tighter box.
pub(crate) fn thread_params(
    model: Option<&str>,
    effort: &str,
    mcp_servers: Value,
    worktree: &Path,
) -> Value {
    // Codex trusts a project it starts with full access, and a trusted project's `.codex/`
    // adds MCP servers and hooks and its `AGENTS.md` adds instructions. Say first that this
    // one is not trusted, under the path the runner uses and under the one it resolves to:
    // Codex may look the project up by either.
    let untrusted = json!({"trust_level": "untrusted"});
    let mut projects = serde_json::Map::new();
    projects.insert(worktree.to_string_lossy().into_owned(), untrusted.clone());
    if let Ok(resolved) = worktree.canonicalize() {
        projects.insert(resolved.to_string_lossy().into_owned(), untrusted);
    }
    json!({
        "approvalPolicy": "never",
        "sandbox": "danger-full-access",
        "cwd": worktree.to_string_lossy(),
        "model": model,
        // Codex keeps no copy of the session under its home's `sessions`.
        "ephemeral": true,
        "config": {
            "model_reasoning_effort": effort,
            "mcp_servers": mcp_servers,
            "projects": projects
        }
    })
}

/// The app-server's stdio, the run log, and the tokens the thread has used so far.
struct Session {
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    lines: mpsc::Sender<String>,
    input_tokens: u64,
    output_tokens: u64,
}

impl Session {
    /// JSON-encode `message`, append a newline, and write to subprocess stdin.
    async fn send(&mut self, message: &Value) -> Result<(), String> {
        let line = format!("{message}\n");
        let written = async {
            self.stdin.write_all(line.as_bytes()).await?;
            self.stdin.flush().await
        };
        written
            .await
            .map_err(|e| format!("cannot write to codex: {e}"))
    }

    /// The next JSON message, logged; `None` when the subprocess closed its output. A line
    /// that is not JSON is logged and skipped. Streaming deltas are not logged: the item
    /// that completes carries their text whole. Nor is the answer to `skills/list`: it
    /// names the person's own skills and where they are, and the run log is the team's.
    async fn next(&mut self, timeout: Duration) -> Result<Option<Value>, String> {
        loop {
            let read = read_record(&mut self.stdout, MAX_RECORD_BYTES);
            let Ok(read) = tokio::time::timeout(timeout, read).await else {
                return Err(format!("codex sent nothing for {}s", timeout.as_secs()));
            };
            let Some(text) = read.map_err(|e| format!("cannot read from codex: {e}"))? else {
                return Ok(None);
            };
            if text.is_empty() {
                continue;
            }
            let payload = serde_json::from_str::<Value>(&text).ok();
            let delta = (payload.as_ref())
                .and_then(|payload| payload.get("method")?.as_str())
                .is_some_and(|method| method.to_ascii_lowercase().ends_with("delta"));
            let skills = payload.as_ref().is_some_and(|payload| {
                payload.get("method").is_none()
                    && payload.get("id").and_then(Value::as_u64) == Some(SKILLS_LIST_ID)
            });
            if !delta && !skills {
                let _ = self.lines.send(text).await;
            }
            if let Some(payload) = payload {
                return Ok(Some(payload));
            }
        }
    }

    /// Read messages until the response to `request_id` is found, and return its `result`.
    /// A request from Codex that arrives first is answered; notifications are skipped.
    async fn await_response(&mut self, request_id: u64) -> Result<Value, String> {
        loop {
            let Some(payload) = self.next(READ_TIMEOUT).await? else {
                return Err("codex exited during the handshake".to_string());
            };
            // A request from Codex has an id of its own numbering, and a method.
            if payload.get("method").is_some() {
                self.answer_request(&payload).await?;
                continue;
            }
            if payload.get("id").and_then(Value::as_u64) != Some(request_id) {
                continue;
            }
            if let Some(error) = payload.get("error") {
                return Err(format!("codex refused request {request_id}: {error}"));
            }
            return (payload.get("result").cloned())
                .ok_or_else(|| format!("codex's response to request {request_id} has no result"));
        }
    }

    /// Perform the startup handshake and return the `thread_id`.
    ///
    /// Order: `initialize(id=1)` → await → `initialized` (no id) → `skills/list(id=4)` →
    /// await → `thread/start(id=2)` → await.
    async fn start_thread(&mut self, mut thread_params: Value) -> Result<String, String> {
        self.send(&json!({
            "method": "initialize",
            "id": INITIALIZE_ID,
            "params": {
                "capabilities": {"experimentalApi": true},
                "clientInfo": {
                    "name": "punchlist-runner",
                    "title": "Punchlist Runner",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }
        }))
        .await?;
        self.await_response(INITIALIZE_ID).await?;
        self.send(&json!({"method": "initialized", "params": {}}))
            .await?;
        // Skills are not configuration Codex reads from its home: it finds them under the
        // person's home directory and in the worktree, trusted or not, lists them to the
        // model and puts a skill's text in the turn when the prompt names it. So ask which
        // it found and start the thread with each one disabled, and with none of its own.
        self.send(&json!({
            "method": "skills/list",
            "id": SKILLS_LIST_ID,
            "params": {"cwds": [thread_params["cwd"]], "forceReload": true}
        }))
        .await?;
        let listed = self.await_response(SKILLS_LIST_ID).await?;
        let found = (listed.get("data").and_then(Value::as_array))
            .ok_or("codex's answer to skills/list has no list of skills")?;
        let disabled: Vec<Value> = (found.iter())
            .flat_map(|entry| entry["skills"].as_array().into_iter().flatten())
            .map(|skill| json!({"path": skill["path"], "enabled": false}))
            .collect();
        let line = format!("disabled the {} skills Codex found", disabled.len());
        let _ = self.lines.send(line).await;
        thread_params["config"]["skills"] = json!({
            "include_instructions": false,
            "bundled": {"enabled": false},
            "config": disabled
        });
        self.send(&json!({
            "method": "thread/start",
            "id": THREAD_START_ID,
            "params": thread_params
        }))
        .await?;
        let thread = self.await_response(THREAD_START_ID).await?;
        (thread.pointer("/thread/id").and_then(Value::as_str))
            .map(String::from)
            .ok_or_else(|| format!("invalid thread payload: {thread}"))
    }

    /// Run a single agent turn: send `turn/start`, then stream messages until the turn
    /// completes, fails or times out.
    async fn run_turn(
        &mut self,
        thread_id: &str,
        worktree: &Path,
        prompt: &str,
    ) -> Result<(), String> {
        self.send(&json!({
            "method": "turn/start",
            "id": TURN_START_ID,
            "params": {
                "threadId": thread_id,
                "input": [{"type": "text", "text": prompt}],
                "cwd": worktree.to_string_lossy()
            }
        }))
        .await?;
        self.await_response(TURN_START_ID).await?;

        loop {
            let Some(payload) = self.next(TURN_TIMEOUT).await? else {
                return Err("codex exited during the turn".to_string());
            };

            match payload.get("method").and_then(Value::as_str) {
                // The thread's running totals. It has this one turn, so they are the run's.
                Some("thread/tokenUsage/updated") => {
                    let total = |key: &str| {
                        payload
                            .pointer(&format!("/params/tokenUsage/total/{key}"))
                            .and_then(Value::as_u64)
                    };
                    self.input_tokens = total("inputTokens").unwrap_or(self.input_tokens);
                    self.output_tokens = total("outputTokens").unwrap_or(self.output_tokens);
                }
                Some("turn/completed") => {
                    // Check if turn/completed carries a failure status
                    // (e.g. usageLimitExceeded, model unavailable)
                    let status = (payload.pointer("/params/turn/status"))
                        .and_then(Value::as_str)
                        .unwrap_or("completed");
                    if status == "completed" {
                        return Ok(());
                    }
                    let message = (payload.pointer("/params/turn/error/message"))
                        .and_then(Value::as_str)
                        .unwrap_or("no error message");
                    return Err(format!("codex turn {status}: {message}"));
                }
                Some(method @ ("turn/failed" | "turn/cancelled")) => {
                    let params = payload.get("params").unwrap_or(&Value::Null);
                    return Err(format!("codex {}: {params}", method.replace('/', " ")));
                }
                _ => self.answer_request(&payload).await?,
            }
        }
    }

    /// Answers a request from Codex, so it does not wait forever; a notification needs no
    /// answer. The approval policy is `never`, so a request for approval is approved. An
    /// elicitation is accepted from Punchlist's MCP server and declined from any other. A
    /// request this runner does not handle gets a JSON-RPC error.
    async fn answer_request(&mut self, payload: &Value) -> Result<(), String> {
        let (Some(id), Some(method)) = (
            payload.get("id"),
            payload.get("method").and_then(Value::as_str),
        ) else {
            return Ok(());
        };
        let answer = match method {
            "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
                json!({"id": id, "result": {"decision": "acceptForSession"}})
            }
            "execCommandApproval" | "applyPatchApproval" => {
                json!({"id": id, "result": {"decision": "approved_for_session"}})
            }
            "mcpServer/elicitation/request" => {
                let server = payload.pointer("/params/serverName");
                let ours = server.and_then(Value::as_str) == Some(MCP_SERVER);
                let action = if ours { "accept" } else { "decline" };
                json!({"id": id, "result": {"action": action}})
            }
            other => json!({"id": id, "error": {
                "code": -32601,
                "message": format!("the Punchlist runner does not handle {other}")
            }}),
        };
        self.send(&answer).await
    }
}

/// Runs one Codex turn in `worktree` with `prompt`, with `home` (a `CodexHome`) as its
/// `CODEX_HOME` and `thread_params` (from `thread_params`) for its thread. The app-server's
/// messages go to `lines` as they arrive, without the streaming deltas; its stderr lines go
/// there prefixed with `stderr: `. Like `run_claude`, the agent runs in its own process group, which is killed
/// when the turn ends and when `stop` becomes true, and that returns `Stopped`.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_codex(
    codex_command: &Path,
    home: &Path,
    env: &[(&'static str, Option<OsString>)],
    thread_params: Value,
    worktree: &Path,
    prompt: &str,
    marker: &str,
    lines: mpsc::Sender<String>,
    mut stop: watch::Receiver<bool>,
) -> AgentExit {
    let mut command = Command::new(codex_command);
    crate::agent::with_env(&mut command, env);
    let spawned = command
        .arg("app-server")
        // The account's apps (connectors) and plugins bring MCP servers of their own, even
        // into an empty home; hooks run the person's or the repository's commands.
        .args(["-c", "features.apps=false"])
        .args(["-c", "features.plugins=false"])
        .args(["-c", "features.hooks=false"])
        .args(["-c", "features.skill_mcp_dependency_install=false"])
        .env("CODEX_HOME", home)
        // It would put Codex's state databases somewhere runs and the person share.
        .env_remove("CODEX_SQLITE_HOME")
        .current_dir(worktree)
        // The agent reads untrusted issue text; keep the person's config path out of its reach.
        .env_remove("PUNCHLIST_CONFIG")
        .env(RUNNER_MARKER, marker)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .process_group(0)
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(e) => {
            return AgentExit::Finished(AgentResult {
                outcome: RunOutcome::Failed,
                reason: Some(format!("cannot start `{}`: {e}", codex_command.display())),
                input_tokens: None,
                output_tokens: None,
            });
        }
    };
    let group = child.id();
    let stderr = child.stderr.take().expect("stderr is piped");
    let stderr_lines = lines.clone();
    let stderr_task = tokio::spawn(async move {
        let mut reader = BufReader::new(stderr);
        while let Ok(Some(line)) = read_record(&mut reader, MAX_RECORD_BYTES).await {
            let _ = stderr_lines.send(format!("stderr: {line}")).await;
        }
    });
    let mut session = Session {
        stdin: child.stdin.take().expect("stdin is piped"),
        stdout: BufReader::new(child.stdout.take().expect("stdout is piped")),
        lines,
        input_tokens: 0,
        output_tokens: 0,
    };

    let turn = tokio::select! {
        turn = async {
            let thread_id = session.start_thread(thread_params).await?;
            session.run_turn(&thread_id, worktree, prompt).await
        } => Some(turn),
        _ = async { let _ = stop.wait_for(|stopped| *stopped).await; } => None,
    };
    // The app-server does not exit when a turn ends. Kill it and whatever it started.
    kill_group(group);
    let _ = child.kill().await;
    // The pipe is closed now; take what is left on it.
    let _ = tokio::time::timeout(Duration::from_secs(1), stderr_task).await;
    let Some(turn) = turn else {
        return AgentExit::Stopped;
    };
    let reason = turn.err();
    AgentExit::Finished(AgentResult {
        outcome: if reason.is_none() {
            RunOutcome::Succeeded
        } else {
            RunOutcome::Failed
        },
        reason,
        input_tokens: Some(session.input_tokens.min(i64::MAX as u64) as i64),
        output_tokens: Some(session.output_tokens.min(i64::MAX as u64) as i64),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::tests::fake_agent;

    /// A fake `codex app-server` that answers each request with the lines a real one sent
    /// after it. `tests/fixtures/codex/` is the stdout of codex-cli 0.162.1 on `gpt-6-luna`
    /// in a live check of this adapter: a run that fixed a failing check, answered a review
    /// thread and requested Human Review. It is cut where each request was answered, and
    /// trimmed: hook, account and rate-limit notifications are dropped, and so are the
    /// deltas of every message but the first. The answer to `thread/start` and its
    /// `thread/started` were recorded again, from the same version without a turn, once the
    /// adapter asked for an ephemeral thread; the turn's messages carry the id of the thread
    /// they were recorded in. `skills_list.jsonl` is not a recording: it is an answer in
    /// the shape 0.162.1 gives, with one skill of the person's and one of the repository's.
    fn replay() -> String {
        let fixtures = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/codex");
        format!(
            r#"[ "$1" = app-server ] || exit 64
echo "$*" > args.txt
echo "$CODEX_HOME" > codex-home.txt
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*) cat {fixtures}/initialize.jsonl ;;
    *'"method":"skills/list"'*) echo "$line" > skills-list.json; cat {fixtures}/skills_list.jsonl ;;
    *'"method":"thread/start"'*) echo "$line" > thread-start.json; cat {fixtures}/thread_start.jsonl ;;
    *'"method":"turn/start"'*) echo "$line" > turn-start.json; cat {fixtures}/turn_start.jsonl ;;
  esac
done"#
        )
    }

    async fn run_fake(script: &str) -> (AgentExit, Vec<String>, tempfile::TempDir) {
        let temp = tempfile::tempdir().unwrap();
        let command = fake_agent(temp.path(), script);
        let (tx, mut rx) = mpsc::channel(10_000);
        let (_stop_tx, stop_rx) = watch::channel(false);
        let mcp = json!({"punchlist": {"command": "pl", "args": ["mcp"]}});
        let home = temp.path().join("codex-home");
        let exit = run_codex(
            &command,
            &home,
            &[],
            thread_params(Some("gpt-6-luna"), "low", mcp, temp.path()),
            temp.path(),
            "Review the pull request.",
            "test",
            tx,
            stop_rx,
        )
        .await;
        let mut out = Vec::new();
        while let Some(line) = rx.recv().await {
            out.push(line);
        }
        (exit, out, temp)
    }

    #[tokio::test]
    async fn a_recorded_session_succeeds_with_its_token_counts_and_log_lines() {
        let (exit, lines, temp) = run_fake(&replay()).await;
        // The last `thread/tokenUsage/updated` of the recording reports these totals.
        assert_eq!(
            exit,
            AgentExit::Finished(AgentResult {
                outcome: RunOutcome::Succeeded,
                reason: None,
                input_tokens: Some(RECORDED_INPUT_TOKENS),
                output_tokens: Some(RECORDED_OUTPUT_TOKENS),
            })
        );

        // Every message of the recording is a log line, in order, except the deltas.
        let recorded: Vec<String> = ["initialize", "thread_start", "turn_start"]
            .iter()
            .flat_map(|name| {
                let path = format!(
                    "{}/tests/fixtures/codex/{name}.jsonl",
                    env!("CARGO_MANIFEST_DIR")
                );
                let text = std::fs::read_to_string(path).unwrap();
                text.lines().map(String::from).collect::<Vec<_>>()
            })
            .collect();
        let is_delta = |line: &&String| {
            let message: Value = serde_json::from_str(line).unwrap();
            let method = message["method"].as_str().unwrap_or_default();
            method.to_ascii_lowercase().ends_with("delta")
        };
        let deltas = recorded.iter().filter(is_delta).count();
        assert!(deltas > 0, "the recording has streaming deltas");
        let kept: Vec<&String> = recorded.iter().filter(|l| !is_delta(l)).collect();
        // The answer to `skills/list` is not a log line: the runner says how many instead.
        let mut lines = lines;
        assert_eq!(lines.remove(1), "disabled the 2 skills Codex found");
        assert!(!lines.iter().any(|line| line.contains("/home/person")));
        assert_eq!(lines.iter().collect::<Vec<_>>(), kept);
        assert_eq!(lines.len(), RECORDED_LOG_LINES);
        assert!(lines[0].starts_with(r#"{"id":1,"result":{"userAgent":"punchlist-runner/"#));
        assert!(
            lines
                .last()
                .unwrap()
                .starts_with(r#"{"method":"turn/completed","#)
        );

        // What Codex was started with: the home it was given, and apps, plugins, hooks and
        // skill installs off.
        let seen = |file: &str| std::fs::read_to_string(temp.path().join(file)).unwrap();
        let home = temp.path().join("codex-home");
        assert_eq!(seen("codex-home.txt").trim(), home.to_str().unwrap());
        assert_eq!(
            seen("args.txt").trim(),
            "app-server -c features.apps=false -c features.plugins=false -c features.hooks=false \
             -c features.skill_mcp_dependency_install=false"
        );

        // What the adapter asked for: the model and effort, no sandbox or approvals,
        // Punchlist's MCP server and no other, the worktree as an untrusted project, every
        // skill Codex listed for the worktree disabled, and the prompt as the turn's input.
        let sent = |file: &str| -> Value { serde_json::from_str(&seen(file)).unwrap() };
        let thread = sent("thread-start.json");
        assert_eq!(thread["params"]["model"], "gpt-6-luna");
        let config = thread["params"]["config"].as_object().unwrap();
        assert_eq!(config["model_reasoning_effort"], "low");
        assert_eq!(
            config["mcp_servers"]
                .as_object()
                .unwrap()
                .keys()
                .collect::<Vec<_>>(),
            ["punchlist"]
        );
        let worktree = temp.path().to_str().unwrap();
        assert_eq!(config["projects"][worktree]["trust_level"], "untrusted");
        assert_eq!(
            sent("skills-list.json")["params"],
            json!({"cwds": [worktree], "forceReload": true})
        );
        assert_eq!(
            config["skills"],
            json!({
                "include_instructions": false,
                "bundled": {"enabled": false},
                "config": [
                    {"path": "/home/person/.agents/skills/deploy/SKILL.md", "enabled": false},
                    {
                        "path": "/w/worktrees/acme/widgets/PL-1/.agents/skills/planted/SKILL.md",
                        "enabled": false
                    }
                ]
            })
        );
        assert_eq!(thread["params"]["approvalPolicy"], "never");
        assert_eq!(thread["params"]["sandbox"], "danger-full-access");
        assert_eq!(thread["params"]["ephemeral"], true);
        // Codex 0.162.1 honors it: the thread it answers with is ephemeral and has no
        // rollout file.
        let answer: Value = serde_json::from_str(&lines[1]).unwrap();
        assert_eq!(answer["id"], 2);
        assert_eq!(answer["result"]["thread"]["ephemeral"], true);
        assert_eq!(answer["result"]["thread"]["path"], Value::Null);
        assert_eq!(
            thread["params"]["config"]["mcp_servers"]["punchlist"]["args"][0],
            "mcp"
        );
        let turn = sent("turn-start.json");
        assert_eq!(
            turn["params"]["input"][0]["text"],
            "Review the pull request."
        );
        assert_eq!(turn["params"]["threadId"], RECORDED_THREAD_ID);
    }

    #[test]
    fn a_runs_home_holds_a_link_to_the_persons_sign_in_and_is_removed_when_dropped() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let person = temp.path().join("person");
        let home = temp.path().join("root/codex-home/run-1");

        // No sign-in, stored or in the environment: refused, naming both, and nothing is made.
        std::fs::create_dir(&person).unwrap();
        let refused = CodexHome::linked_to(&person, &home, false).err().unwrap();
        assert_eq!(
            refused.to_string(),
            format!(
                "no Codex sign-in at {}/auth.json and no CODEX_ACCESS_TOKEN: run `codex login` \
                 as the person who runs the runner (a sign-in kept in the keyring is not \
                 supported)",
                person.display()
            )
        );
        assert!(!home.exists());

        // A sign-in in the environment alone: an empty home, with no link.
        let entries = |home: &Path| -> Vec<_> {
            let entries = std::fs::read_dir(home).unwrap();
            entries.map(|entry| entry.unwrap().file_name()).collect()
        };
        let made = CodexHome::linked_to(&person, &home, true).unwrap();
        assert_eq!(entries(&home), Vec::<OsString>::new());
        drop(made);
        assert!(!home.exists());

        // A home left by a killed run is replaced: only the link is in the new one.
        std::fs::write(person.join("auth.json"), "the sign-in").unwrap();
        std::fs::write(person.join("config.toml"), "[mcp_servers.other]").unwrap();
        std::fs::create_dir_all(home.join("shell_snapshots")).unwrap();
        let made = CodexHome::linked_to(&person, &home, false).unwrap();
        assert_eq!(made.path(), home);
        let mode = std::fs::metadata(&home).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
        assert_eq!(entries(&home), ["auth.json"]);
        assert_eq!(
            std::fs::read_link(home.join("auth.json")).unwrap(),
            person.join("auth.json")
        );
        assert_eq!(
            std::fs::read_to_string(home.join("auth.json")).unwrap(),
            "the sign-in"
        );

        // Dropping it removes the home and leaves the person's sign-in.
        drop(made);
        assert!(!home.exists());

        // With both, the stored sign-in is still linked.
        let made = CodexHome::linked_to(&person, &home, true).unwrap();
        assert_eq!(entries(&home), ["auth.json"]);
        drop(made);
        assert_eq!(
            std::fs::read_to_string(person.join("auth.json")).unwrap(),
            "the sign-in"
        );
    }

    #[test]
    fn the_persons_codex_home_is_codex_home_when_set_else_dot_codex_and_always_absolute() {
        let home = || Some(PathBuf::from("/home/p"));
        let found =
            |codex_home: Option<&str>| person_home(codex_home.map(OsString::from), home()).unwrap();
        assert_eq!(found(None), PathBuf::from("/home/p/.codex"));
        assert_eq!(found(Some("")), PathBuf::from("/home/p/.codex"));
        assert_eq!(found(Some("/srv/codex")), PathBuf::from("/srv/codex"));
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(found(Some("rel/codex")), cwd.join("rel/codex"));

        // A run's home is absolute too, whatever the worktree root is.
        assert_eq!(
            run_home(Path::new("rel/root"), "run-1").unwrap(),
            cwd.join("rel/root/codex-home/run-1")
        );
        assert_eq!(
            run_home(Path::new("/w"), "run-1").unwrap(),
            PathBuf::from("/w/codex-home/run-1")
        );
        assert_eq!(
            person_home(None, None).unwrap_err().to_string(),
            "HOME is not set"
        );
    }

    const RECORDED_THREAD_ID: &str = "01a1311f-a14d-7052-9470-5458b555231a";
    const RECORDED_INPUT_TOKENS: i64 = 299_517;
    const RECORDED_OUTPUT_TOKENS: i64 = 2810;
    const RECORDED_LOG_LINES: usize = 96;

    #[tokio::test]
    async fn a_failed_turn_fails_the_run_with_codexs_message_and_keeps_the_tokens() {
        let script = r#"while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*) echo '{"id":1,"result":{}}' ;;
    *'"method":"skills/list"'*) echo '{"id":4,"result":{"data":[]}}' ;;
    *'"method":"thread/start"'*) echo '{"id":2,"result":{"thread":{"id":"t1"}}}' ;;
    *'"method":"turn/start"'*)
      echo '{"id":3,"result":{"turn":{"id":"u1"}}}'
      echo 'not json'
      echo '{"method":"thread/tokenUsage/updated","params":{"tokenUsage":{"total":{"inputTokens":120,"outputTokens":7}}}}'
      echo '{"method":"thread/tokenUsage/updated","params":{"tokenUsage":{"total":{"inputTokens":300,"outputTokens":9}}}}'
      echo '{"method":"turn/completed","params":{"turn":{"id":"u1","status":"failed","error":{"message":"usage limit reached"}}}}' ;;
  esac
done"#;
        let (exit, lines, _temp) = run_fake(script).await;
        assert_eq!(
            exit,
            AgentExit::Finished(AgentResult {
                outcome: RunOutcome::Failed,
                reason: Some("codex turn failed: usage limit reached".into()),
                input_tokens: Some(300),
                output_tokens: Some(9),
            })
        );
        assert_eq!(lines.len(), 8);
        assert_eq!(lines[4], "not json");
    }

    #[tokio::test]
    async fn a_request_from_codex_is_answered_so_the_turn_goes_on() {
        // The fake waits for the answer to each of its requests before it goes on. The first
        // comes before the response to `turn/start`, with that request's own id.
        let script = r#"while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*) echo '{"id":1,"result":{}}' ;;
    *'"method":"skills/list"'*) echo '{"id":4,"result":{"data":[]}}' ;;
    *'"method":"thread/start"'*) echo '{"id":2,"result":{"thread":{"id":"t1"}}}' ;;
    *'"method":"turn/start"'*)
      echo '{"id":3,"method":"item/commandExecution/requestApproval","params":{}}' ;;
    *'"decision":"acceptForSession"'*)
      echo '{"id":3,"result":{"turn":{"id":"u1"}}}'
      echo '{"id":1,"method":"account/chatgptAuthTokens/refresh","params":{}}' ;;
    *'does not handle account/chatgptAuthTokens/refresh'*)
      echo '{"id":2,"method":"mcpServer/elicitation/request","params":{"serverName":"other"}}' ;;
    *'"id":2,"result":{"action":"decline"}'*)
      echo '{"id":4,"method":"mcpServer/elicitation/request","params":{"serverName":"punchlist"}}' ;;
    *'"id":4,"result":{"action":"accept"}'*)
      echo '{"method":"turn/completed","params":{"turn":{"id":"u1","status":"completed"}}}' ;;
  esac
done"#;
        let (exit, _, _temp) = run_fake(script).await;
        assert!(
            matches!(exit, AgentExit::Finished(ref r) if r.outcome == RunOutcome::Succeeded),
            "{exit:?}"
        );
    }

    #[tokio::test]
    async fn codex_exiting_mid_turn_and_the_stop_signal_end_the_run() {
        let script = r#"while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*) echo '{"id":1,"result":{}}' ;;
    *'"method":"skills/list"'*) echo '{"id":4,"result":{"data":[]}}' ;;
    *'"method":"thread/start"'*) echo '{"id":2,"error":{"code":-32600,"message":"no such model"}}'; exit 0 ;;
  esac
done"#;
        let (exit, _, _temp) = run_fake(script).await;
        let AgentExit::Finished(result) = exit else {
            panic!("expected a finished exit");
        };
        assert_eq!(result.outcome, RunOutcome::Failed);
        assert_eq!(
            result.reason.as_deref(),
            Some(r#"codex refused request 2: {"code":-32600,"message":"no such model"}"#)
        );

        let temp = tempfile::tempdir().unwrap();
        let command = fake_agent(temp.path(), "sleep 30");
        let (tx, _rx) = mpsc::channel(16);
        let (stop_tx, stop_rx) = watch::channel(false);
        let dir = temp.path().to_path_buf();
        let run = tokio::spawn(async move {
            run_codex(
                &command,
                &dir,
                &[],
                json!({}),
                &dir,
                "p",
                "test",
                tx,
                stop_rx,
            )
            .await
        });
        tokio::time::sleep(Duration::from_millis(200)).await;
        stop_tx.send(true).unwrap();
        assert_eq!(run.await.unwrap(), AgentExit::Stopped);
    }
}
