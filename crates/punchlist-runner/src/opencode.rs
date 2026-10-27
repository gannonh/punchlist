//! The OpenCode adapter: runs `opencode run --standalone --format json --auto` in the
//! worktree with the prompt on stdin, streams its JSON records as log lines, and sums the
//! tokens and cost of its `step_finish` records.
//! New code: Kata Symphony has no OpenCode adapter.
//!
//! OpenCode has no `--strict-mcp-config`, and merges every configuration it finds. The run
//! gets only what the runner gives it because its global configuration directory and its
//! data directory (`XDG_CONFIG_HOME`, `XDG_DATA_HOME`) are the run's own, empty but for the
//! file `OPENCODE_CONFIG` names, which attaches Punchlist's MCP server; project
//! configuration, the worktree's `AGENTS.md` among it, and Claude Code's files are off. So
//! the person's plugins, MCP servers and `AGENTS.md` never load, and a reviewing run takes no
//! instructions from the tree it reviews. `--standalone` runs OpenCode's server inside the
//! one process, in the run's process group, rather than on the person's background service.
//!
//! OpenCode 2 keeps sign-ins in its database, not in `auth.json`, so the run's is a copy:
//! `opencode auth export` in the person's data directory, piped into `opencode auth import`
//! in the run's, API keys only.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use punchlist_api::{Claim, RunOutcome};
use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::sync::{mpsc, watch};

use crate::RunnerConfig;
use crate::agent::{Agent, AgentExit, AgentResult, plain_decimal, run_streaming, with_env};

/// The arguments after the executable. The prompt goes on stdin.
pub(crate) fn opencode_args(model: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = ["run", "--standalone", "--format", "json", "--auto"]
        .map(String::from)
        .into();
    if let Some(model) = model {
        args.push("--model".into());
        args.push(model.into());
    }
    args
}

/// The run's configuration file in `home`.
fn config_file(home: &Path) -> PathBuf {
    home.join("opencode.json")
}

/// Variables of the person's that would add configuration.
const PERSONS: [&str; 3] = [
    "OPENCODE_CONFIG_DIR",
    "OPENCODE_CONFIG_CONTENT",
    "OPENCODE_CLI_CONFIG_CONTENT",
];

/// What OpenCode needs in any process the runner starts, over the runner's environment: the
/// run's global configuration directory, project configuration and Claude Code's files off,
/// and none of the person's own configuration variables.
fn isolated(home: &Path) -> Vec<(&'static str, Option<OsString>)> {
    let set = |value: &str| Some(OsString::from(value));
    let mut env: Vec<(&'static str, Option<OsString>)> =
        PERSONS.iter().map(|name| (*name, None)).collect();
    env.extend([
        (
            "XDG_CONFIG_HOME",
            Some(home.join("config").into_os_string()),
        ),
        // Either turns project configuration off, and the first wins when both are set.
        ("OPENCODE_CONFIG_PROJECT_DISABLE", set("1")),
        ("OPENCODE_DISABLE_PROJECT_CONFIG", set("1")),
        ("OPENCODE_DISABLE_CLAUDE_CODE", set("1")),
    ]);
    env
}

/// The run's data directory, which holds its database (sign-in and session), in place of the
/// person's, and of any database the person's `OPENCODE_DB` names.
fn run_data(home: &Path) -> [(&'static str, Option<OsString>); 2] {
    [
        ("XDG_DATA_HOME", Some(home.join("data").into_os_string())),
        ("OPENCODE_DB", None),
    ]
}

/// The environment of a run in `home`, over the attempt's: `isolated`, `run_data` and
/// `OPENCODE_CONFIG`. `gh`,
/// when it is the runner's own login (no `GH_CONFIG_DIR` in the attempt's environment), reads
/// it from `person_gh`, since `XDG_CONFIG_HOME` no longer leads there.
// ponytail: the agent's commands get the run's `XDG_CONFIG_HOME` and `XDG_DATA_HOME` too, so
// a tool that keeps its configuration or data there (pnpm's store) starts empty in each run.
// The cache and state directories stay the person's, so Playwright's browsers are found.
pub(crate) fn run_env(
    home: &Path,
    attempt: &[(&'static str, Option<OsString>)],
    person_gh: Option<PathBuf>,
) -> Vec<(&'static str, Option<OsString>)> {
    let mut env = attempt.to_vec();
    env.extend(isolated(home));
    env.extend(run_data(home));
    env.push(("OPENCODE_CONFIG", Some(config_file(home).into_os_string())));
    if !attempt.iter().any(|(name, _)| *name == "GH_CONFIG_DIR") {
        env.push(("GH_CONFIG_DIR", person_gh.map(PathBuf::into_os_string)));
    }
    env
}

/// The person's `gh` configuration directory, as `gh` finds it.
pub(crate) fn person_gh_dir() -> Option<PathBuf> {
    let var = |name| std::env::var_os(name).filter(|value| !value.is_empty());
    var("GH_CONFIG_DIR").map(PathBuf::from).or_else(|| {
        let config = var("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::home_dir().map(|home| home.join(".config")))?;
        Some(config.join("gh"))
    })
}

/// Writes the run's configuration into `home`, readable only by this user: Punchlist's MCP
/// server for the claimed issue, with the run's agent token, and no update checks.
pub(crate) fn write_config(
    home: &Path,
    config: &RunnerConfig,
    claim: &Claim,
) -> std::io::Result<PathBuf> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let path = config_file(home);
    let json = json!({
        "mcp": {
            "punchlist": {
                "type": "local",
                "command": [config.mcp_command, "mcp"],
                "environment": {
                    "PUNCHLIST_SERVER_URL": config.server_url,
                    "PUNCHLIST_TOKEN": claim.agent_token,
                    "PUNCHLIST_ISSUE": claim.issue.id,
                    "PUNCHLIST_RUN": claim.run_id,
                },
                "enabled": true
            }
        },
        "autoupdate": false
    });
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&path)?
        .write_all(json.to_string().as_bytes())?;
    Ok(path)
}

/// How long `opencode auth export` or `import` may take.
const AUTH_TIMEOUT: Duration = Duration::from_secs(60);

/// Copies the person's OpenCode API keys into the run's database in `home`, and returns the
/// line for the run log; the keys never reach it. A person with no key, or whose sign-in
/// cannot be read, gets a run that relies on the runner's environment (`OPENCODE_API_KEY`,
/// `OPENAI_API_KEY` and the like). The export reads the person's data directory with the
/// run's empty configuration, so none of the person's plugins or MCP servers start.
// ponytail: OAuth sign-ins are left out: a run refreshing a copy would rotate the token the
// person's own OpenCode holds. Share them when OpenCode can read a sign-in from a file again.
pub(crate) async fn copy_sign_in(command: &Path, home: &Path) -> Result<String, String> {
    let exported = {
        let mut export = Command::new(command);
        with_env(&mut export, &isolated(home));
        export
            .args(["auth", "export", "--standalone"])
            .current_dir(home)
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output()
    };
    let output = match tokio::time::timeout(AUTH_TIMEOUT, exported).await {
        Ok(Ok(output)) if output.status.success() => output.stdout,
        Ok(Ok(output)) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let last = stderr.lines().rfind(|line| !line.trim().is_empty());
            return Ok(no_sign_in(&format!(
                "`opencode auth export` failed: {}",
                last.unwrap_or("no message")
            )));
        }
        Ok(Err(e)) => {
            return Ok(no_sign_in(&format!(
                "cannot run `opencode auth export`: {e}"
            )));
        }
        Err(_) => return Ok(no_sign_in("`opencode auth export` timed out")),
    };
    let Ok(credentials) = serde_json::from_slice::<Vec<Value>>(&output) else {
        return Ok(no_sign_in("`opencode auth export` printed no list"));
    };
    let (keys, others): (Vec<Value>, Vec<Value>) = (credentials.into_iter())
        .partition(|credential| credential.pointer("/value/type") == Some(&json!("key")));
    let skipped = if others.is_empty() {
        String::new()
    } else {
        let names: Vec<&str> = (others.iter())
            .filter_map(|credential| credential["integrationID"].as_str())
            .collect();
        format!("; not OAuth sign-ins ({})", names.join(", "))
    };
    if keys.is_empty() {
        return Ok(no_sign_in(&format!(
            "the person has no OpenCode API key{skipped}"
        )));
    }
    let mut import = Command::new(command);
    with_env(
        &mut import,
        &[isolated(home), run_data(home).into()].concat(),
    );
    let child = import
        .args(["auth", "import", "--standalone"])
        .current_dir(home)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn();
    let mut child = child.map_err(|e| format!("cannot run `opencode auth import`: {e}"))?;
    let mut stdin = child.stdin.take().expect("stdin is piped");
    let imported = async {
        stdin.write_all(&serde_json::to_vec(&keys)?).await?;
        drop(stdin);
        child.wait_with_output().await
    };
    match tokio::time::timeout(AUTH_TIMEOUT, imported).await {
        Ok(Ok(output)) if output.status.success() => Ok(format!(
            "copied {} OpenCode API key(s) from the person's sign-in{skipped}",
            keys.len()
        )),
        Ok(Ok(output)) => Err(format!(
            "`opencode auth import` failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )),
        Ok(Err(e)) => Err(format!("cannot run `opencode auth import`: {e}")),
        Err(_) => Err("`opencode auth import` timed out".to_string()),
    }
}

fn no_sign_in(why: &str) -> String {
    format!("no OpenCode sign-in copied: {why}; relying on the runner's environment")
}

/// What the runner reads from OpenCode's records: the `step_finish` records' tokens and
/// cost, and the last `error` record.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Stream {
    steps: usize,
    input_tokens: i64,
    output_tokens: i64,
    /// The steps' cost summed, as plain decimal digits; `None` once a step had none.
    cost_usd: Option<String>,
    error: Option<String>,
}

/// A `step_finish` record's part, with its cost as written, so no digit goes through an `f64`.
#[derive(Deserialize)]
struct StepFinish<'a> {
    #[serde(borrow)]
    part: StepPart<'a>,
}

#[derive(Deserialize)]
struct StepPart<'a> {
    #[serde(borrow, default)]
    cost: Option<&'a RawValue>,
    #[serde(default)]
    tokens: Tokens,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Tokens {
    input: i64,
    output: i64,
    reasoning: i64,
    cache: Cache,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Cache {
    read: i64,
    write: i64,
}

/// The longest error text a failure reason quotes, in characters.
const ERROR_CHARS: usize = 200;

impl Stream {
    /// Reads one stdout record. Input counts cache reads and writes as Claude Code's does,
    /// and output counts reasoning. A line that is not JSON changes nothing.
    pub(crate) fn read(&mut self, line: &str) {
        let Ok(record) = serde_json::from_str::<Value>(line) else {
            return;
        };
        match record["type"].as_str() {
            Some("step_finish") => {
                let Ok(StepFinish { part }) = serde_json::from_str::<StepFinish>(line) else {
                    return;
                };
                let tokens = part.tokens;
                self.input_tokens += tokens.input + tokens.cache.read + tokens.cache.write;
                self.output_tokens += tokens.output + tokens.reasoning;
                let cost = part.cost.and_then(|raw| plain_decimal(raw.get()));
                self.cost_usd = match (self.steps, &self.cost_usd, cost) {
                    (0, _, cost) => cost,
                    (_, Some(sum), Some(cost)) => add_decimal(sum, &cost),
                    _ => None,
                };
                self.steps += 1;
            }
            Some("error") => {
                let error = &record["error"];
                let name = (error["name"].as_str()).or_else(|| error["type"].as_str());
                let message = (error["message"].as_str())
                    .or_else(|| error.pointer("/data/message").and_then(Value::as_str));
                let text = match (name, message) {
                    (Some(name), Some(message)) => format!("{name}: {message}"),
                    (Some(text), None) | (None, Some(text)) => text.to_string(),
                    (None, None) => "no message".to_string(),
                };
                self.error = Some(text.chars().take(ERROR_CHARS).collect());
            }
            _ => {}
        }
    }
}

/// The sum of two plain decimals, as plain decimal digits; `None` past 30 characters.
fn add_decimal(a: &str, b: &str) -> Option<String> {
    let split = |n: &str| -> (String, String) {
        let (whole, fraction) = n.split_once('.').unwrap_or((n, ""));
        (whole.to_string(), fraction.to_string())
    };
    let ((a_whole, a_fraction), (b_whole, b_fraction)) = (split(a), split(b));
    let places = a_fraction.len().max(b_fraction.len());
    let scaled = |whole: &str, fraction: &str| -> Option<u128> {
        format!("{whole}{fraction:0<places$}").parse().ok()
    };
    let sum = scaled(&a_whole, &a_fraction)? + scaled(&b_whole, &b_fraction)?;
    let digits = format!("{sum:0>width$}", width = places + 1);
    let (whole, fraction) = digits.split_at(digits.len() - places);
    let fraction = fraction.trim_end_matches('0');
    let text = if fraction.is_empty() {
        whole.to_string()
    } else {
        format!("{whole}.{fraction}")
    };
    (text.len() <= 30).then_some(text)
}

/// Succeeded when OpenCode exited 0, reported no error and finished a step.
pub(crate) fn outcome_of(exit_code: Option<i32>, stream: Stream) -> AgentResult {
    let reason = match (exit_code, &stream.error) {
        (_, Some(error)) => Some(format!("opencode reported an error ({error})")),
        (Some(0), None) if stream.steps == 0 => {
            Some("opencode exited without a step_finish".to_string())
        }
        (Some(0), None) => None,
        (Some(code), None) => Some(format!("opencode exited with code {code}")),
        (None, None) => Some("opencode was killed by a signal".to_string()),
    };
    let finished = stream.steps > 0;
    AgentResult {
        outcome: if reason.is_none() {
            RunOutcome::Succeeded
        } else {
            RunOutcome::Failed
        },
        reason,
        input_tokens: finished.then_some(stream.input_tokens),
        output_tokens: finished.then_some(stream.output_tokens),
        usage_limit: None,
        cost_usd: stream.cost_usd,
    }
}

/// Runs `opencode` in `worktree` with `prompt` on stdin and `env` (from `run_env`) over the
/// runner's environment, as `run_streaming` does.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_opencode(
    command: &Path,
    args: &[String],
    env: &[(&'static str, Option<OsString>)],
    worktree: &Path,
    prompt: &str,
    marker: &str,
    lines: mpsc::Sender<String>,
    stop: watch::Receiver<bool>,
) -> AgentExit {
    let agent = Agent {
        name: "opencode",
        command,
        args,
        env,
        read: Stream::read,
        outcome: outcome_of,
    };
    run_streaming(agent, worktree, prompt, marker, lines, stop).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::RunHome;
    use crate::agent::tests::fake_agent;
    use std::os::unix::fs::PermissionsExt;

    fn fixture(name: &str) -> String {
        let path = format!(
            "{}/tests/fixtures/opencode/{name}",
            env!("CARGO_MANIFEST_DIR")
        );
        std::fs::read_to_string(path).unwrap()
    }

    fn read_all(text: &str) -> Stream {
        let mut stream = Stream::default();
        text.lines().for_each(|line| stream.read(line));
        stream
    }

    /// Two steps in the shape of OpenCode 2.1.2's `step_finish` record (its `--format json`
    /// emitter), with the token counts and costs of two steps its database recorded, a text
    /// record between them and a line that is not JSON.
    const TWO_STEPS: &str = r#"{"type":"step_start","timestamp":1793117522405,"sessionID":"ses_1","part":{"id":"prt_1","sessionID":"ses_1","messageID":"msg_1","type":"step-start","snapshot":"36addb5b"}}
{"type":"step_finish","timestamp":1793117523405,"sessionID":"ses_1","part":{"id":"prt_2","sessionID":"ses_1","messageID":"msg_1","type":"step-finish","reason":"tool-calls","snapshot":"36addb5b","cost":0.00342879,"tokens":{"total":268448,"input":4741,"output":2155,"reasoning":1072,"cache":{"write":0,"read":260480}}}}
not json
{"type":"text","timestamp":1793117524405,"sessionID":"ses_1","part":{"id":"prt_3","sessionID":"ses_1","messageID":"msg_2","type":"text","text":"ok"}}
{"type":"step_finish","timestamp":1793117525405,"sessionID":"ses_1","part":{"id":"prt_4","sessionID":"ses_1","messageID":"msg_2","type":"step-finish","reason":"stop","snapshot":"36addb5b","cost":1.2e-7,"tokens":{"total":158957,"input":1226,"output":355,"reasoning":576,"cache":{"write":3,"read":156800}}}}"#;

    #[test]
    fn the_arguments_run_standalone_in_json_with_permissions_approved() {
        assert_eq!(
            opencode_args(Some("openai/gpt-6-luna")),
            [
                "run",
                "--standalone",
                "--format",
                "json",
                "--auto",
                "--model",
                "openai/gpt-6-luna"
            ]
        );
        assert_eq!(
            opencode_args(None),
            ["run", "--standalone", "--format", "json", "--auto"]
        );
    }

    #[test]
    fn a_runs_environment_and_config_hold_only_the_run_and_punchlists_mcp_server() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("opencode-home/run-1");
        let home = RunHome::create(&home).unwrap();
        let config = RunnerConfig {
            server_url: "http://127.0.0.1:7878".into(),
            token: "person-token".into(),
            workspace: None,
            name: "r1".into(),
            agents: vec!["opencode".into()],
            state_dir: temp.path().into(),
            claude_command: "claude".into(),
            mcp_command: "/usr/bin/pl".into(),
            model: None,
            codex_command: "codex".into(),
            codex_model: None,
            codex_effort: "medium".into(),
            opencode_command: "opencode".into(),
            opencode_model: None,
            max_concurrent: 1,
            heartbeat_interval: Duration::from_secs(10),
            claim_wait: Duration::ZERO,
        };
        let claim: Claim = serde_json::from_value(json!({
            "run_id": "00000000-0000-0000-0000-00000000000a",
            "attempt": 1,
            "agent": "opencode",
            "issue": {
                "id": "PL-1", "title": "t", "body": "", "status": "in_progress",
                "status_name": "In Progress", "created_at": "2026-10-27T00:00:00Z",
                "updated_at": "2026-10-27T00:00:00Z"
            },
            "repository": {"owner": "acme", "name": "widgets", "default_branch": "main"},
            "branch": "feature/pl-1",
            "lease_expires_at": "2026-10-27T00:00:00Z",
            "prompt": "p",
            "agent_token": "agent-token",
            "session_id": "00000000-0000-0000-0000-000000000005",
            "next_line": 1
        }))
        .unwrap();

        let path = write_config(home.path(), &config, &claim).unwrap();
        assert_eq!(path, home.path().join("opencode.json"));
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            written,
            json!({
                "mcp": {"punchlist": {
                    "type": "local",
                    "command": ["/usr/bin/pl", "mcp"],
                    "environment": {
                        "PUNCHLIST_SERVER_URL": "http://127.0.0.1:7878",
                        "PUNCHLIST_TOKEN": "agent-token",
                        "PUNCHLIST_ISSUE": "PL-1",
                        "PUNCHLIST_RUN": "00000000-0000-0000-0000-00000000000a"
                    },
                    "enabled": true
                }},
                "autoupdate": false
            })
        );

        let os = |s: &str| Some(OsString::from(s));
        let under = |dir: &str| Some(home.path().join(dir).into_os_string());
        let attempt = [("PUNCHLIST_ATTEMPT", os("a-1"))];
        let expected_isolation = [
            ("OPENCODE_CONFIG_DIR", None),
            ("OPENCODE_CONFIG_CONTENT", None),
            ("OPENCODE_CLI_CONFIG_CONTENT", None),
            ("XDG_CONFIG_HOME", under("config")),
            ("OPENCODE_CONFIG_PROJECT_DISABLE", os("1")),
            ("OPENCODE_DISABLE_PROJECT_CONFIG", os("1")),
            ("OPENCODE_DISABLE_CLAUDE_CODE", os("1")),
            ("XDG_DATA_HOME", under("data")),
            ("OPENCODE_DB", None),
            ("OPENCODE_CONFIG", under("opencode.json")),
        ];
        let env = run_env(home.path(), &attempt, Some("/home/p/.config/gh".into()));
        let mut expected = attempt.to_vec();
        expected.extend(expected_isolation.clone());
        expected.push(("GH_CONFIG_DIR", os("/home/p/.config/gh")));
        assert_eq!(env, expected);

        // A run acting as the workspace's App keeps the App's `gh` configuration.
        let app = [("GH_CONFIG_DIR", os("/s/github/run/gh"))];
        let mut expected = app.to_vec();
        expected.extend(expected_isolation);
        assert_eq!(run_env(home.path(), &app, None), expected);
    }

    #[test]
    fn step_finish_records_sum_their_tokens_and_cost_and_other_lines_change_nothing() {
        let stream = read_all(TWO_STEPS);
        // Input: 4741 + 260480 + 0, then 1226 + 156800 + 3. Output: 2155 + 1072, then 355 + 576.
        assert_eq!(
            stream,
            Stream {
                steps: 2,
                input_tokens: 423_250,
                output_tokens: 4158,
                cost_usd: Some("0.00342891".into()),
                error: None,
            }
        );
        assert_eq!(
            outcome_of(Some(0), stream),
            AgentResult {
                outcome: RunOutcome::Succeeded,
                reason: None,
                input_tokens: Some(423_250),
                output_tokens: Some(4158),
                usage_limit: None,
                cost_usd: Some("0.00342891".into()),
            }
        );
        // A step without a cost leaves the attempt with none, not a partial sum.
        let no_cost = TWO_STEPS.replace(r#""cost":1.2e-7,"#, "");
        assert_eq!(read_all(&no_cost).cost_usd, None);
    }

    #[test]
    fn decimals_add_exactly() {
        assert_eq!(add_decimal("0.1", "0.2").as_deref(), Some("0.3"));
        assert_eq!(add_decimal("0.9", "0.1").as_deref(), Some("1"));
        assert_eq!(
            add_decimal("12", "0.0000001").as_deref(),
            Some("12.0000001")
        );
        assert_eq!(add_decimal("0", "0").as_deref(), Some("0"));
    }

    #[test]
    fn the_recorded_error_run_fails_with_opencodes_error() {
        // OpenCode 2.1.2's output on `opencode-go`'s free model once its free usage ran out:
        // a `step_start`, then the `error` record, and exit code 1.
        let stream = read_all(&fixture("free-limit.jsonl"));
        let reason = "opencode reported an error (provider.free-limit: Free usage limit reached. \
                      Connect a provider or subscribe to OpenCode Go to keep going, or switch \
                      models.)";
        assert_eq!(
            outcome_of(Some(1), stream),
            AgentResult {
                outcome: RunOutcome::Failed,
                reason: Some(reason.into()),
                input_tokens: None,
                output_tokens: None,
                usage_limit: None,
                cost_usd: None,
            }
        );
    }

    #[test]
    fn the_outcome_needs_exit_0_no_error_and_a_step_finish() {
        let reason = |code: Option<i32>, text: &str| outcome_of(code, read_all(text)).reason;
        assert_eq!(reason(Some(0), TWO_STEPS), None);
        assert_eq!(
            reason(Some(0), "{\"type\":\"text\"}").as_deref(),
            Some("opencode exited without a step_finish")
        );
        let error = format!(
            "{TWO_STEPS}\n{{\"type\":\"error\",\"error\":{{\"name\":\"APIError\",\"message\":\"{}\"}}}}",
            "x".repeat(300)
        );
        let cut = format!("APIError: {}", "x".repeat(300));
        assert_eq!(
            reason(Some(0), &error),
            Some(format!(
                "opencode reported an error ({})",
                &cut[..ERROR_CHARS]
            ))
        );
        assert_eq!(
            reason(Some(1), TWO_STEPS).as_deref(),
            Some("opencode exited with code 1")
        );
        assert_eq!(
            reason(None, TWO_STEPS).as_deref(),
            Some("opencode was killed by a signal")
        );
        // A failed attempt keeps the tokens its steps used.
        let failed = outcome_of(Some(1), read_all(TWO_STEPS));
        assert_eq!(failed.outcome, RunOutcome::Failed);
        assert_eq!(failed.input_tokens, Some(423_250));
    }

    #[test]
    fn a_runs_home_is_private_replaces_a_killed_runs_and_goes_when_dropped() {
        let temp = tempfile::tempdir().unwrap();
        let path = RunHome::path_for(temp.path(), "opencode-home", "run-1").unwrap();
        assert_eq!(path, temp.path().join("opencode-home/run-1"));
        std::fs::create_dir_all(path.join("data/opencode")).unwrap();
        std::fs::write(path.join("data/opencode/opencode.db"), "left").unwrap();
        let home = RunHome::create(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
        assert_eq!(std::fs::read_dir(&path).unwrap().count(), 0);
        drop(home);
        assert!(!path.exists());
    }

    /// A fake `opencode` whose `auth export` prints an API key and an OAuth sign-in, and whose
    /// `auth import` keeps what it reads and the data directory it was given.
    fn fake_auth(dir: &Path, export: &str) -> PathBuf {
        let seen = dir.display();
        fake_agent(
            dir,
            &format!(
                r#"case "$1 $2 $3" in
  "auth export --standalone") echo "$XDG_CONFIG_HOME ${{XDG_DATA_HOME-unset}} $OPENCODE_DISABLE_PROJECT_CONFIG" > {seen}/export-env; {export} ;;
  "auth import --standalone") cat > {seen}/imported; echo "$XDG_CONFIG_HOME $XDG_DATA_HOME" > {seen}/import-env; echo "Imported 1 credential" ;;
  *) exit 64 ;;
esac"#
            ),
        )
    }

    #[tokio::test]
    async fn the_persons_api_keys_are_copied_into_the_runs_database_and_oauth_is_left_out() {
        let temp = tempfile::tempdir().unwrap();
        let home = RunHome::create(&temp.path().join("home")).unwrap();
        let export = r#"echo '[{"id":"c1","integrationID":"opencode-go","label":"k","active":true,"value":{"type":"key","key":"sk-1"}},{"id":"c2","integrationID":"openai","label":"o","active":true,"value":{"type":"oauth","refresh":"r","access":"a","expires":1}}]'"#;
        let command = fake_auth(temp.path(), export);
        let line = copy_sign_in(&command, home.path()).await.unwrap();
        assert_eq!(
            line,
            "copied 1 OpenCode API key(s) from the person's sign-in; not OAuth sign-ins (openai)"
        );
        let imported: Value =
            serde_json::from_str(&std::fs::read_to_string(temp.path().join("imported")).unwrap())
                .unwrap();
        assert_eq!(
            imported,
            json!([{"id":"c1","integrationID":"opencode-go","label":"k","active":true,"value":{"type":"key","key":"sk-1"}}])
        );
        // The export reads the person's data with the run's empty configuration; the import
        // writes into the run's data directory.
        let config = home.path().join("config");
        let seen = |file: &str| std::fs::read_to_string(temp.path().join(file)).unwrap();
        let person_data = std::env::var("XDG_DATA_HOME").unwrap_or("unset".into());
        assert_eq!(
            seen("export-env").trim(),
            format!("{} {person_data} 1", config.display())
        );
        assert_eq!(
            seen("import-env").trim(),
            format!(
                "{} {}",
                config.display(),
                home.path().join("data").display()
            )
        );

        // No key, or an export that fails: the run relies on the runner's environment.
        let none = fake_auth(temp.path(), "echo '[]'");
        assert_eq!(
            copy_sign_in(&none, home.path()).await.unwrap(),
            "no OpenCode sign-in copied: the person has no OpenCode API key; relying on the \
             runner's environment"
        );
        let failing = fake_auth(temp.path(), "echo 'Error: database is locked' >&2; exit 1");
        assert_eq!(
            copy_sign_in(&failing, home.path()).await.unwrap(),
            "no OpenCode sign-in copied: `opencode auth export` failed: Error: database is \
             locked; relying on the runner's environment"
        );
    }

    async fn run_fake(script: &str) -> (AgentExit, Vec<String>, tempfile::TempDir) {
        let temp = tempfile::tempdir().unwrap();
        let command = fake_agent(temp.path(), script);
        let (tx, mut rx) = mpsc::channel(10_000);
        let (_stop_tx, stop_rx) = watch::channel(false);
        let exit = run_opencode(
            &command,
            &opencode_args(Some("opencode-go/gpt-6-luna")),
            &[("XDG_CONFIG_HOME", Some("/run/config".into()))],
            temp.path(),
            "Build the issue.",
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
    async fn a_fake_opencode_streams_its_records_and_succeeds_with_their_tokens() {
        let steps = std::env::temp_dir().join(format!("opencode-steps-{}", std::process::id()));
        std::fs::write(&steps, TWO_STEPS).unwrap();
        let script = format!(
            "echo \"$* | $(pwd) | $XDG_CONFIG_HOME | $(cat)\" > args.txt\ncat {}\necho warn >&2",
            steps.display()
        );
        let (exit, lines, temp) = run_fake(&script).await;
        std::fs::remove_file(&steps).unwrap();
        assert_eq!(
            exit,
            AgentExit::Finished(AgentResult {
                outcome: RunOutcome::Succeeded,
                reason: None,
                input_tokens: Some(423_250),
                output_tokens: Some(4158),
                usage_limit: None,
                cost_usd: Some("0.00342891".into()),
            })
        );
        // Every stdout record in order, unchanged; stderr's lines prefixed, read apart.
        let (stderr, stdout): (Vec<String>, Vec<String>) = lines
            .into_iter()
            .partition(|line| line.starts_with("stderr: "));
        assert_eq!(stdout, TWO_STEPS.lines().collect::<Vec<_>>());
        assert_eq!(stderr, ["stderr: warn"]);
        let args = std::fs::read_to_string(temp.path().join("args.txt")).unwrap();
        assert_eq!(
            args.trim(),
            format!(
                "run --standalone --format json --auto --model opencode-go/gpt-6-luna | {} | \
                 /run/config | Build the issue.",
                temp.path().canonicalize().unwrap().display()
            )
        );
    }

    #[tokio::test]
    async fn stopping_a_run_kills_opencode_and_what_it_started() {
        let temp = tempfile::tempdir().unwrap();
        let pidfile = temp.path().join("child.pid");
        let script = format!("sleep 30 &\necho $! > {}\nwait", pidfile.display());
        let command = fake_agent(temp.path(), &script);
        let (tx, _rx) = mpsc::channel(16);
        let (stop_tx, stop_rx) = watch::channel(false);
        let dir = temp.path().to_path_buf();
        let run = tokio::spawn(async move {
            run_opencode(
                &command,
                &opencode_args(None),
                &[],
                &dir,
                "p",
                "test",
                tx,
                stop_rx,
            )
            .await
        });
        let mut pid = String::new();
        for _ in 0..40 {
            pid = std::fs::read_to_string(&pidfile).unwrap_or_default();
            if !pid.trim().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let pid = pid.trim().to_string();
        assert!(!pid.is_empty(), "the fake opencode never started its child");
        stop_tx.send(true).unwrap();
        assert_eq!(run.await.unwrap(), AgentExit::Stopped);
        crate::agent::tests::assert_dies(&pid).await;
    }
}
