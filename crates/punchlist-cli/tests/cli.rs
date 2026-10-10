//! `pl` run as a binary against a real server on an ephemeral port.

use std::path::PathBuf;

use assert_cmd::Command;
use punchlist_api::{AppendLog, FinishRun, RegisterRunner, RunOutcome};
use punchlist_client::Client;
use punchlist_server::{AppState, Bootstrap, bootstrap, router};
use sqlx::PgPool;

struct Pl {
    config: PathBuf,
    url: String,
    token: String,
    _dir: tempfile::TempDir,
}

struct Output {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Pl {
    async fn start(pool: &PgPool) -> Pl {
        let person = bootstrap(
            pool,
            &Bootstrap {
                workspace_name: "Punchlist".into(),
                issue_prefix: "PL".into(),
                repository_owner: "gannonh".into(),
                repository_name: "punchlist".into(),
                person_name: "Gannon".into(),
            },
        )
        .await
        .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = router(AppState::new(pool.clone()));
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config.toml");
        std::fs::write(
            &config,
            format!(
                "server_url = \"http://{addr}\"\ntoken = \"{}\"\n",
                person.token
            ),
        )
        .unwrap();
        Pl {
            config,
            url: format!("http://{addr}"),
            token: person.token,
            _dir: dir,
        }
    }

    async fn run(&self, args: &[&str]) -> Output {
        let config = self.config.clone();
        let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        let output = tokio::task::spawn_blocking(move || {
            Command::cargo_bin("pl")
                .unwrap()
                .env("PUNCHLIST_CONFIG", config)
                .args(args)
                .output()
                .unwrap()
        })
        .await
        .unwrap();
        Output {
            code: output.status.code().unwrap(),
            stdout: String::from_utf8(output.stdout).unwrap(),
            stderr: String::from_utf8(output.stderr).unwrap(),
        }
    }
}

#[sqlx::test(migrator = "punchlist_server::MIGRATOR")]
async fn create_show_move_and_refuse(pool: PgPool) {
    let pl = Pl::start(&pool).await;

    let create = pl
        .run(&["issue", "create", "--title", "First", "--body", "Body"])
        .await;
    assert_eq!(
        (create.code, create.stdout.as_str(), create.stderr.as_str()),
        (0, "PL-1\n", "")
    );

    let show = pl.run(&["issue", "show", "PL-1"]).await;
    assert_eq!(show.code, 0);
    insta::assert_snapshot!(show.stdout, @r"
    PL-1  First
    Status: Backlog

    Body

    Pull requests
      None linked.

    Timeline
      No transitions yet.

    Runs
      No runs yet.
    ");

    let todo = pl.run(&["issue", "move", "PL-1", "todo"]).await;
    assert_eq!(
        (todo.code, todo.stdout.as_str()),
        (0, "PL-1  Backlog → Todo\n")
    );
    let start = pl.run(&["issue", "move", "PL-1", "start"]).await;
    assert_eq!(
        (start.code, start.stdout.as_str()),
        (0, "PL-1  Todo → Start\n")
    );

    let show = pl.run(&["issue", "show", "PL-1"]).await;
    insta::with_settings!({filters => vec![(r"\d{4}-\d\d-\d\d \d\d:\d\d:\d\d UTC", "[time]")]}, {
        insta::assert_snapshot!(show.stdout, @r"
        PL-1  First
        Status: Start

        Body

        Pull requests
          None linked.

        Timeline
          [time]  Gannon (person)  Backlog → Todo
          [time]  Gannon (person)  Todo → Start

        Runs
          No runs yet.
        ");
    });

    let refused = pl.run(&["issue", "move", "PL-1", "in_progress"]).await;
    assert_eq!(refused.code, 1);
    assert_eq!(refused.stdout, "");
    insta::assert_snapshot!(refused.stderr, @"error: refused: only a runner makes start → in_progress, not a person");

    pl.run(&["issue", "create", "--title", "Second"]).await;
    let refused = pl.run(&["issue", "move", "PL-2", "done"]).await;
    assert_eq!(refused.code, 1);
    insta::assert_snapshot!(refused.stderr, @"error: refused: no transition backlog → done exists in the workflow");

    let list = pl.run(&["issue", "list"]).await;
    insta::assert_snapshot!(list.stdout, @r"
    PL-1  Start    First
    PL-2  Backlog  Second
    ");
}

#[sqlx::test(migrator = "punchlist_server::MIGRATOR")]
async fn unknown_issue_and_missing_config(pool: PgPool) {
    let pl = Pl::start(&pool).await;
    let missing = pl.run(&["issue", "show", "PL-9"]).await;
    assert_eq!(missing.code, 1);
    insta::assert_snapshot!(missing.stderr, @"error: issue PL-9 not found (404 Not Found)");

    let output = Command::cargo_bin("pl")
        .unwrap()
        .env("PUNCHLIST_CONFIG", "/nonexistent/config.toml")
        .args(["issue", "list"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8(output.stderr).unwrap().starts_with(
        "error: cannot read /nonexistent/config.toml; it needs `server_url` and `token`"
    ));
}

fn time_filters() -> Vec<(&'static str, &'static str)> {
    vec![
        (r"\d{4}-\d\d-\d\d \d\d:\d\d:\d\d UTC", "[time]"),
        (
            r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}",
            "[run]",
        ),
        (r"heartbeat \d+[smh] ago", "heartbeat [age] ago"),
    ]
}

async fn register(pl: &Pl, name: &str) -> Client {
    let person = Client::new(&pl.url, &pl.token).unwrap();
    let registered = person
        .register_runner(&RegisterRunner {
            name: name.into(),
            agents: vec!["claude-code".into()],
        })
        .await
        .unwrap();
    Client::new(&pl.url, &registered.token).unwrap()
}

#[sqlx::test(migrator = "punchlist_server::MIGRATOR")]
async fn runner_list_shows_registered_runner(pool: PgPool) {
    let pl = Pl::start(&pool).await;
    let empty = pl.run(&["runner", "list"]).await;
    assert_eq!((empty.code, empty.stdout.as_str()), (0, "No runners.\n"));

    register(&pl, "sartre").await;
    let list = pl.run(&["runner", "list"]).await;
    assert_eq!(list.code, 0);
    insta::with_settings!({filters => time_filters()}, {
        insta::assert_snapshot!(list.stdout, @"sartre  claude-code  heartbeat [age] ago");
    });
    let age: u32 = list
        .stdout
        .split("heartbeat ")
        .nth(1)
        .and_then(|rest| rest.split("s ago").next())
        .and_then(|n| n.parse().ok())
        .expect("a heartbeat age in seconds");
    assert!(age < 10, "{}", list.stdout);
}

#[sqlx::test(migrator = "punchlist_server::MIGRATOR")]
async fn a_claimed_run_shows_on_the_issue_and_in_the_log(pool: PgPool) {
    let pl = Pl::start(&pool).await;
    pl.run(&["issue", "create", "--title", "Do it"]).await;
    pl.run(&["issue", "move", "PL-1", "todo"]).await;
    pl.run(&["issue", "move", "PL-1", "start"]).await;

    let runner = register(&pl, "r1").await;
    let claim = runner.claim(0).await.unwrap().expect("a claim");
    let run_id = claim.run_id.to_string();
    runner
        .append_log(
            &run_id,
            &AppendLog {
                attempt: claim.attempt,
                first_line: 1,
                lines: vec!["cloning".into(), "editing".into(), "x".repeat(250)],
            },
        )
        .await
        .unwrap();

    let show = pl.run(&["issue", "show", "PL-1"]).await;
    assert_eq!(show.code, 0, "{}", show.stderr);
    let long = format!("{}…", "x".repeat(200));
    insta::with_settings!({filters => time_filters()}, {
        insta::assert_snapshot!(show.stdout.replace(&long, "[200 x]"), @r"
        PL-1  Do it
        Status: In Progress

        Pull requests
          None linked.

        Timeline
          [time]  Gannon (person)  Backlog → Todo
          [time]  Gannon (person)  Todo → Start
          [time]  r1 (runner)  Start → In Progress

        Runs
          [run]  Claude Code  runner r1  started [time]  duration —  running
            cloning
            editing
            [200 x]
        ");
    });

    runner
        .finish_run(
            &run_id,
            &FinishRun {
                attempt: claim.attempt,
                outcome: RunOutcome::Succeeded,
                reason: None,
                duration_ms: 83_000,
                input_tokens: Some(1000),
                output_tokens: Some(250),
            },
        )
        .await
        .unwrap();
    let show = pl.run(&["issue", "show", "PL-1"]).await;
    insta::with_settings!({filters => time_filters()}, {
        insta::assert_snapshot!(show.stdout.replace(&long, "[200 x]"), @r"
        PL-1  Do it
        Status: In Progress

        Pull requests
          None linked.

        Timeline
          [time]  Gannon (person)  Backlog → Todo
          [time]  Gannon (person)  Todo → Start
          [time]  r1 (runner)  Start → In Progress

        Runs
          [run]  Claude Code  runner r1  started [time]  duration 1m 23s  succeeded  tokens 1000 in / 250 out
            cloning
            editing
            [200 x]
        ");
    });

    let log = pl.run(&["run", "log", &run_id]).await;
    assert_eq!(log.code, 0);
    assert_eq!(
        log.stdout,
        format!("cloning\nediting\n{}\n", "x".repeat(250))
    );

    let unknown = pl
        .run(&["run", "log", "00000000-0000-0000-0000-000000000000"])
        .await;
    assert_eq!(unknown.code, 1);
    assert!(unknown.stderr.starts_with("error: "), "{}", unknown.stderr);
    assert!(unknown.stderr.contains("404"), "{}", unknown.stderr);
}

/// Inserts a pull request on the bootstrapped repository, the row webhook processing
/// writes. The rows here stand in for that processing, which the server's tests cover; these
/// tests cover how `pl` reads and prints them.
#[allow(clippy::too_many_arguments)]
async fn insert_pull_request(
    pool: &PgPool,
    number: i64,
    title: &str,
    branch: &str,
    draft: bool,
    checks: (&str, i32, i32),
    open_threads: i32,
    issue_id: Option<&str>,
) {
    sqlx::query(
        "INSERT INTO pull_request (repository_id, number, github_id, title, branch, head_sha,
                                   url, author_login, state, draft, merge_state, checks,
                                   checks_total, checks_passed, open_threads, issue_id,
                                   github_updated_at)
         SELECT id, $1, $1, $2, $3, 'abc123', $4, 'gannonh', 'open', $5, 'clean', $6, $7, $8,
                $9, $10, now()
         FROM repository",
    )
    .bind(number)
    .bind(title)
    .bind(branch)
    .bind(format!(
        "https://github.com/gannonh/punchlist/pull/{number}"
    ))
    .bind(draft)
    .bind(checks.0)
    .bind(checks.1)
    .bind(checks.2)
    .bind(open_threads)
    .bind(issue_id)
    .execute(pool)
    .await
    .unwrap();
}

#[sqlx::test(migrator = "punchlist_server::MIGRATOR")]
async fn a_linked_pull_request_shows_on_the_issue(pool: PgPool) {
    let pl = Pl::start(&pool).await;
    pl.run(&["issue", "create", "--title", "Fix"]).await;
    insert_pull_request(
        &pool,
        7,
        "PL-1: fix",
        "feature/pl-1-fix",
        true,
        ("passing", 2, 2),
        0,
        Some("PL-1"),
    )
    .await;
    insert_pull_request(
        &pool,
        6,
        "PL-1: first try",
        "feature/pl-1-try",
        false,
        ("none", 0, 0),
        1,
        Some("PL-1"),
    )
    .await;

    let show = pl.run(&["issue", "show", "PL-1"]).await;
    assert_eq!(show.code, 0, "{}", show.stderr);
    assert!(show.stdout.contains("#7 draft"), "{}", show.stdout);
    insta::assert_snapshot!(show.stdout, @r"
    PL-1  Fix
    Status: Backlog

    Pull requests
      #7 draft  feature/pl-1-fix  checks passing 2/2  0 open threads
        https://github.com/gannonh/punchlist/pull/7
      #6 open  feature/pl-1-try  no checks  1 open thread
        https://github.com/gannonh/punchlist/pull/6

    Timeline
      No transitions yet.

    Runs
      No runs yet.
    ");
}

#[sqlx::test(migrator = "punchlist_server::MIGRATOR")]
async fn unlinked_pull_requests_list_newest_first(pool: PgPool) {
    let pl = Pl::start(&pool).await;
    let none = pl.run(&["pr", "unlinked"]).await;
    assert_eq!(
        (none.code, none.stdout.as_str()),
        (0, "No unlinked pull requests.\n")
    );

    pl.run(&["issue", "create", "--title", "Fix"]).await;
    insert_pull_request(&pool, 3, "Stray", "stray", false, ("none", 0, 0), 0, None).await;
    insert_pull_request(
        &pool,
        4,
        "Also stray",
        "feature/other",
        true,
        ("none", 0, 0),
        0,
        None,
    )
    .await;
    insert_pull_request(
        &pool,
        5,
        "PL-1: fix",
        "feature/pl-1-fix",
        false,
        ("none", 0, 0),
        0,
        Some("PL-1"),
    )
    .await;

    let unlinked = pl.run(&["pr", "unlinked"]).await;
    assert_eq!(
        (unlinked.code, unlinked.stdout.as_str()),
        (
            0,
            "#4  draft  feature/other  Also stray\n#3  open  stray  Stray\n"
        )
    );
}

#[sqlx::test(migrator = "punchlist_server::MIGRATOR")]
async fn a_comment_and_a_github_move_show_on_the_timeline(pool: PgPool) {
    let pl = Pl::start(&pool).await;
    pl.run(&["issue", "create", "--title", "Fix"]).await;
    // Rows that webhook processing writes for a pull request closed without merging.
    sqlx::query(
        "WITH gh AS (
             INSERT INTO actor (workspace_id, name, role, github_login)
             SELECT id, 'octocat', 'github', 'octocat' FROM workspace RETURNING id, workspace_id
         ), c AS (
             INSERT INTO comment (issue_id, actor_id, body)
             SELECT 'PL-1', id, E'Closed without merging.\\nSee #7.' FROM gh RETURNING id, actor_id
         ), s AS (
             UPDATE workspace SET last_event_seq = 1 RETURNING id
         )
         INSERT INTO event (workspace_id, seq, issue_id, kind, actor_id, comment_id)
         SELECT s.id, 1, 'PL-1', 'comment', c.actor_id, c.id FROM s, c",
    )
    .execute(&pool)
    .await
    .unwrap();

    let show = pl.run(&["issue", "show", "PL-1"]).await;
    assert_eq!(show.code, 0, "{}", show.stderr);
    insta::with_settings!({filters => time_filters()}, {
        insta::assert_snapshot!(show.stdout, @r"
        PL-1  Fix
        Status: Backlog

        Pull requests
          None linked.

        Timeline
          [time]  octocat (github)  commented:
            Closed without merging.
            See #7.

        Runs
          No runs yet.
        ");
    });
}

#[test]
fn runner_start_help_lists_the_flags() {
    let output = Command::cargo_bin("pl")
        .unwrap()
        .args(["runner", "start", "--help"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let help = String::from_utf8(output.stdout).unwrap();
    for flag in [
        "--name",
        "--worktree-root",
        "--model",
        "--claude-command",
        "--max-concurrent",
    ] {
        assert!(help.contains(flag), "{flag} missing from:\n{help}");
    }
}

#[sqlx::test(migrator = "punchlist_server::MIGRATOR")]
async fn workflow_show_transition_and_comment(pool: PgPool) {
    let pl = Pl::start(&pool).await;

    let show = pl.run(&["workflow", "show"]).await;
    assert_eq!(show.code, 0, "{}", show.stderr);
    assert_eq!(
        show.stdout,
        format!(
            "Version   {}\nCommit    built-in\nStatuses  backlog, todo, start, in_progress, agent_review, human_review, merging, done, canceled\n",
            punchlist_core::default_workflow().version()
        )
    );

    pl.run(&["issue", "create", "--title", "First"]).await;
    let moved = pl.run(&["issue", "transition", "PL-1", "todo"]).await;
    assert_eq!(
        (moved.code, moved.stdout.as_str()),
        (0, "PL-1  Backlog → Todo\n")
    );
    let commented = pl
        .run(&["issue", "comment", "PL-1", "--body", "Looks right."])
        .await;
    assert_eq!(
        (commented.code, commented.stdout.as_str()),
        (0, "Commented on PL-1.\n")
    );
    let show = pl.run(&["issue", "show", "PL-1"]).await;
    assert!(
        show.stdout
            .contains("Gannon (person)  commented:\n    Looks right.\n"),
        "{}",
        show.stdout
    );
}

/// `pl mcp` over real pipes, as Claude Code runs it: the handshake, then a refused
/// `request_transition` and a `comment`, with the agent token a claim returns.
#[sqlx::test(migrator = "punchlist_server::MIGRATOR")]
async fn mcp_refuses_a_gated_transition_and_comments_as_the_agent(pool: PgPool) {
    use std::io::{BufRead, BufReader, Write};

    let pl = Pl::start(&pool).await;
    pl.run(&["issue", "create", "--title", "Do it"]).await;
    pl.run(&["issue", "move", "PL-1", "todo"]).await;
    pl.run(&["issue", "move", "PL-1", "start"]).await;
    let claim = register(&pl, "r1").await.claim(0).await.unwrap().unwrap();

    let url = pl.url.clone();
    let lines = tokio::task::spawn_blocking(move || {
        let mut child = std::process::Command::new(assert_cmd::cargo::cargo_bin("pl"))
            .arg("mcp")
            .env("PUNCHLIST_SERVER_URL", url)
            .env("PUNCHLIST_TOKEN", claim.agent_token)
            .env("PUNCHLIST_ISSUE", "PL-1")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        let mut exchange = |message: &str, reply: bool| {
            writeln!(stdin, "{message}").unwrap();
            let mut line = String::new();
            if reply {
                stdout.read_line(&mut line).unwrap();
            }
            line
        };
        exchange(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}"#,
            true,
        );
        exchange(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#, false);
        let refused = exchange(
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"request_transition","arguments":{"to":"agent_review"}}}"#,
            true,
        );
        let commented = exchange(
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"comment","arguments":{"body":"Started."}}}"#,
            true,
        );
        drop(stdin);
        assert!(child.wait().unwrap().success());
        [refused, commented]
    })
    .await
    .unwrap();

    let result = |line: &str| -> (bool, String) {
        let value: serde_json::Value = serde_json::from_str(line).unwrap();
        let result = &value["result"];
        (
            result["isError"].as_bool().unwrap_or(false),
            result["content"][0]["text"].as_str().unwrap().to_string(),
        )
    };
    let (is_error, text) = result(&lines[0]);
    assert!(is_error);
    assert_eq!(
        text.lines().next().unwrap(),
        "Refused (gate_failed): in_progress → agent_review needs gate pr_open: no open pull request is linked to PL-1"
    );
    assert_eq!(result(&lines[1]), (false, "Commented on PL-1.".to_string()));
    let show = pl.run(&["issue", "show", "PL-1"]).await;
    assert!(
        show.stdout
            .contains("Claude Code (agent)  commented:\n    Started.\n\n    (agent)\n"),
        "{}",
        show.stdout
    );
}
