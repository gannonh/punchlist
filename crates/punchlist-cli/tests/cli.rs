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
    assert!(list.stdout.contains("heartbeat 0s ago") || list.stdout.contains("heartbeat 1s ago"));
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
