// Copied from kata-symphony apps/symphony/src/codex/app_server.rs at e7c160f5; trimmed: kept the handshake (`initialize`, `initialized`, `thread/start`), `turn/start`, the receive loop with its turn outcomes and token accounting, auto-approval, the non-interactive answer to `requestUserInput`, `needs_input` and the message helpers; dropped the docker and ssh workers, dynamic tools, the `AgentEvent` stream, rate limits, the triage handle and the workspace check (worktree.rs checks the path). Written for Punchlist: the process group and stop signal, the run log lines, the answer to requests it does not handle, and the model and MCP server sent in `thread/start`.
//! The Codex adapter: runs `codex app-server` in the worktree, starts a thread and one turn
//! with the prompt over JSON-RPC on its stdio, streams its messages as log lines, and reads
//! the turn's outcome and token counts.
//!
//! 1. Spawn `<codex> app-server` with the worktree as cwd.
//! 2. Handshake: `initialize(id=1)` → await response → `initialized` → `thread/start(id=2)` → await response.
//! 3. Turn: `turn/start(id=3)` → await response → stream events until `turn/completed` / failure / timeout.
//! 4. Stop: kill the subprocess and its process group.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use punchlist_api::{Claim, RunOutcome};
use serde_json::{Value, json};
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout, Command};
use tokio::sync::{mpsc, watch};

use crate::RunnerConfig;
use crate::agent::{AgentExit, AgentResult, MAX_RECORD_BYTES, kill_group, read_record};
use crate::token_accounting::{TokenState, extract_token_delta};

/// Answer sent to Codex when the session is non-interactive and cannot provide
/// operator input.
const NON_INTERACTIVE_ANSWER: &str =
    "This is a non-interactive session. Operator input is unavailable.";

const INITIALIZE_ID: u64 = 1;
const THREAD_START_ID: u64 = 2;
const TURN_START_ID: u64 = 3;

/// How long a response in the handshake may take.
const READ_TIMEOUT: Duration = Duration::from_secs(60);
/// The longest a turn may go without a message.
const TURN_TIMEOUT: Duration = Duration::from_secs(3600);

/// Codex's `mcp_servers` config for one run: Punchlist's MCP server for the claimed issue,
/// with the run's agent token. It goes to Codex in `thread/start`, over stdin, so the token
/// is in no file and no process listing.
pub fn mcp_servers(config: &RunnerConfig, claim: &Claim) -> Value {
    json!({
        "punchlist": {
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

/// The app-server's stdio, the run log, and the tokens the turn has used so far.
struct Session {
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    lines: mpsc::Sender<String>,
    token_state: TokenState,
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
    /// that completes carries their text whole.
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
            if !delta {
                let _ = self.lines.send(text).await;
            }
            if let Some(payload) = payload {
                return Ok(Some(payload));
            }
        }
    }

    /// Read messages until the response to `request_id` is found, and return its `result`.
    /// Other messages are skipped.
    async fn await_response(&mut self, request_id: u64) -> Result<Value, String> {
        loop {
            let Some(payload) = self.next(READ_TIMEOUT).await? else {
                return Err("codex exited during the handshake".to_string());
            };
            // A request from Codex has an id of its own numbering, and a method.
            if payload.get("id").and_then(Value::as_u64) != Some(request_id)
                || payload.get("method").is_some()
            {
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
    /// Order: `initialize(id=1)` → await → `initialized` (no id) → `thread/start(id=2)` → await.
    // ponytail: `danger-full-access` and no approvals, as the Claude adapter's
    // `bypassPermissions`: the agent has to reach git's directory in the clone, the network
    // and `gh`. Codex's own `workspace-write` sandbox allows none of the three; give the
    // run a container or a sandbox policy with those holes when untrusted issue text needs
    // a tighter box.
    async fn start_thread(
        &mut self,
        worktree: &Path,
        model: Option<&str>,
        mcp_servers: Value,
    ) -> Result<String, String> {
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
        self.send(&json!({
            "method": "thread/start",
            "id": THREAD_START_ID,
            "params": {
                "approvalPolicy": "never",
                "sandbox": "danger-full-access",
                "cwd": worktree.to_string_lossy(),
                "model": model,
                "config": {"mcp_servers": mcp_servers}
            }
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

            // ── Token accounting ──────────────────────────
            let delta = extract_token_delta(&self.token_state, &payload);
            self.input_tokens += delta.input_tokens;
            self.output_tokens += delta.output_tokens;
            self.token_state.last_input = delta.input_reported;
            self.token_state.last_output = delta.output_reported;

            let Some(method) = payload.get("method").and_then(Value::as_str) else {
                continue;
            };
            let params = payload.get("params").cloned().unwrap_or(Value::Null);
            match method {
                "turn/completed" => {
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
                "turn/failed" => return Err(format!("codex turn failed: {params}")),
                "turn/cancelled" => return Err(format!("codex turn cancelled: {params}")),

                // The approval policy is `never`, so a request for approval is approved.
                "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
                    self.answer(&payload, json!({"decision": "acceptForSession"}))
                        .await?;
                }
                "execCommandApproval" | "applyPatchApproval" => {
                    self.answer(&payload, json!({"decision": "approved_for_session"}))
                        .await?;
                }
                "mcpServer/elicitation/request" => {
                    self.answer(&payload, json!({"action": "accept"})).await?;
                }
                "item/tool/requestUserInput" => {
                    let Some(answers) = build_non_interactive_answers(&params) else {
                        return Err("codex asked for input this session cannot give".into());
                    };
                    self.answer(&payload, json!({"answers": answers})).await?;
                }
                other if needs_input(other, &payload) => {
                    return Err(format!("codex needs operator input ({other})"));
                }
                // Any other request would wait for an answer forever.
                other if payload.get("id").is_some() => {
                    let error = json!({
                        "code": -32601,
                        "message": format!("the Punchlist runner does not handle {other}")
                    });
                    let id = payload.get("id");
                    self.send(&json!({"id": id, "error": error})).await?;
                }
                _ => {}
            }
        }
    }

    /// Sends `{"id": <id>, "result": <result>}` in answer to a request from Codex.
    async fn answer(&mut self, request: &Value, result: Value) -> Result<(), String> {
        self.send(&json!({"id": request.get("id"), "result": result}))
            .await
    }
}

/// Build non-interactive answers for all questions.
///
/// Returns `Some(answers_map)` if every question has a string `id` field.
/// Returns `None` if any question lacks one.
fn build_non_interactive_answers(params: &Value) -> Option<Value> {
    let questions = params.get("questions")?.as_array()?;
    if questions.is_empty() {
        return None;
    }

    let mut answers = serde_json::Map::new();
    for question in questions {
        let question_id = question.get("id")?.as_str()?;
        answers.insert(
            question_id.to_string(),
            json!({"answers": [NON_INTERACTIVE_ANSWER]}),
        );
    }

    Some(Value::Object(answers))
}

/// Detect whether `method` + `payload` indicate a generic input-required event.
///
/// - Method must start with `"turn/"`.
/// - Method is in the known input-required list, OR payload/params have a
///   `requiresInput`, `needsInput`, `input_required`, `inputRequired`, `type`
///   flag set to `true` / `"input_required"` / `"needs_input"`.
fn needs_input(method: &str, payload: &Value) -> bool {
    const INPUT_METHODS: &[&str] = &[
        "turn/input_required",
        "turn/needs_input",
        "turn/need_input",
        "turn/request_input",
        "turn/request_response",
        "turn/provide_input",
        "turn/approval_required",
    ];
    if !method.starts_with("turn/") {
        return false;
    }
    if INPUT_METHODS.contains(&method) {
        return true;
    }
    // Check payload and params for input-required flags
    let params = payload.get("params");
    payload_needs_input(payload) || params.is_some_and(payload_needs_input)
}

fn payload_needs_input(payload: &Value) -> bool {
    let flag = |key: &str| payload.get(key).and_then(Value::as_bool) == Some(true);
    let kind = payload.get("type").and_then(Value::as_str);
    flag("requiresInput")
        || flag("needsInput")
        || flag("input_required")
        || flag("inputRequired")
        || matches!(kind, Some("input_required" | "needs_input"))
}

/// Runs one Codex turn in `worktree` with `prompt`. The app-server's messages go to `lines`
/// as they arrive, without the streaming deltas; its stderr lines go there prefixed with
/// `stderr: `. Like `run_claude`, the agent runs in its own process group, which is killed
/// when the turn ends and when `stop` becomes true, and that returns `Stopped`.
pub async fn run_codex(
    codex_command: &Path,
    model: Option<&str>,
    mcp_servers: Value,
    worktree: &Path,
    prompt: &str,
    lines: mpsc::Sender<String>,
    mut stop: watch::Receiver<bool>,
) -> AgentExit {
    let spawned = Command::new(codex_command)
        .arg("app-server")
        .current_dir(worktree)
        // The agent reads untrusted issue text; keep the person's config path out of its reach.
        .env_remove("PUNCHLIST_CONFIG")
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
        token_state: TokenState::default(),
        input_tokens: 0,
        output_tokens: 0,
    };

    let turn = tokio::select! {
        turn = async {
            let thread_id = session.start_thread(worktree, model, mcp_servers).await?;
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
    use crate::agent::tests::fake_claude;

    /// A fake `codex app-server` that answers each request with the lines a real one sent
    /// after it. `tests/fixtures/codex/` is the stdout of codex-cli 0.162.1 on `gpt-6-luna`
    /// in a live check of this adapter: a run that fixed a failing check, answered a review
    /// thread and requested Human Review. It is cut where each request was answered, and
    /// trimmed: hook, account and rate-limit notifications are dropped, and so are the
    /// deltas of every message but the first.
    fn replay() -> String {
        let fixtures = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/codex");
        format!(
            r#"[ "$1" = app-server ] || exit 64
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*) cat {fixtures}/initialize.jsonl ;;
    *'"method":"thread/start"'*) echo "$line" > thread-start.json; cat {fixtures}/thread_start.jsonl ;;
    *'"method":"turn/start"'*) echo "$line" > turn-start.json; cat {fixtures}/turn_start.jsonl ;;
  esac
done"#
        )
    }

    async fn run_fake(script: &str) -> (AgentExit, Vec<String>, tempfile::TempDir) {
        let temp = tempfile::tempdir().unwrap();
        let command = fake_claude(temp.path(), script);
        let (tx, mut rx) = mpsc::channel(10_000);
        let (_stop_tx, stop_rx) = watch::channel(false);
        let mcp = json!({"punchlist": {"command": "pl", "args": ["mcp"]}});
        let exit = run_codex(
            &command,
            Some("gpt-6-luna"),
            mcp,
            temp.path(),
            "Review the pull request.",
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
        assert_eq!(lines.iter().collect::<Vec<_>>(), kept);
        assert_eq!(lines.len(), RECORDED_LOG_LINES);
        assert!(lines[0].starts_with(r#"{"id":1,"result":{"userAgent":"punchlist-runner/"#));
        assert!(
            lines
                .last()
                .unwrap()
                .starts_with(r#"{"method":"turn/completed","#)
        );

        // What the adapter asked for: the model, no sandbox or approvals, Punchlist's MCP
        // server, and the prompt as the turn's input.
        let sent = |file: &str| -> Value {
            serde_json::from_str(&std::fs::read_to_string(temp.path().join(file)).unwrap()).unwrap()
        };
        let thread = sent("thread-start.json");
        assert_eq!(thread["params"]["model"], "gpt-6-luna");
        assert_eq!(thread["params"]["approvalPolicy"], "never");
        assert_eq!(thread["params"]["sandbox"], "danger-full-access");
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

    const RECORDED_THREAD_ID: &str = "01a130f2-f3d8-7083-8649-3af6cd58bb7c";
    const RECORDED_INPUT_TOKENS: i64 = 299_517;
    const RECORDED_OUTPUT_TOKENS: i64 = 2810;
    const RECORDED_LOG_LINES: usize = 96;

    #[tokio::test]
    async fn a_failed_turn_fails_the_run_with_codexs_message_and_keeps_the_tokens() {
        let script = r#"while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*) echo '{"id":1,"result":{}}' ;;
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
        assert_eq!(lines.len(), 7);
        assert_eq!(lines[3], "not json");
    }

    #[tokio::test]
    async fn a_request_from_codex_is_answered_so_the_turn_goes_on() {
        // The fake waits for the answer to each of its requests before it goes on.
        let script = r#"while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*) echo '{"id":1,"result":{}}' ;;
    *'"method":"thread/start"'*) echo '{"id":2,"result":{"thread":{"id":"t1"}}}' ;;
    *'"method":"turn/start"'*)
      echo '{"id":3,"result":{"turn":{"id":"u1"}}}'
      echo '{"id":0,"method":"item/commandExecution/requestApproval","params":{}}' ;;
    *'"decision":"acceptForSession"'*)
      echo '{"id":1,"method":"account/chatgptAuthTokens/refresh","params":{}}' ;;
    *'does not handle account/chatgptAuthTokens/refresh'*)
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
        let command = fake_claude(temp.path(), "sleep 30");
        let (tx, _rx) = mpsc::channel(16);
        let (stop_tx, stop_rx) = watch::channel(false);
        let dir = temp.path().to_path_buf();
        let run = tokio::spawn(async move {
            run_codex(&command, None, json!({}), &dir, "p", tx, stop_rx).await
        });
        tokio::time::sleep(Duration::from_millis(200)).await;
        stop_tx.send(true).unwrap();
        assert_eq!(run.await.unwrap(), AgentExit::Stopped);
    }
}
