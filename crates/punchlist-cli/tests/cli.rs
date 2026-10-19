//! `pl` run as a binary against a real server on an ephemeral port.

use std::path::PathBuf;

use assert_cmd::Command;
use punchlist_api::{AppendLog, FinishRun, RegisterRunner, RunOutcome};
use punchlist_client::Client;
use punchlist_server::{AppState, Bootstrap, ProofStore, bootstrap, router};
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
        let dir = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = router(AppState::new(pool.clone()).with_proof(ProofStore {
            dir: dir.path().join("proof"),
            max_bytes: 1024 * 1024,
        }));
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

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
        self.run_with(args, &[]).await
    }

    async fn run_with(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        let config = self.config.clone();
        let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        let env: Vec<(String, String)> = (env.iter())
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let output = tokio::task::spawn_blocking(move || {
            Command::cargo_bin("pl")
                .unwrap()
                .env("PUNCHLIST_CONFIG", config)
                .env_remove("LINEAR_API_KEY")
                .envs(env)
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
    insta::with_settings!({filters => time_filters()}, {
        insta::assert_snapshot!(show.stdout, @r"
        PL-1  First
        Status: Backlog

        Body

        Pull requests
          None linked.

        Proof
          None attached.

        Timeline
          [time]  Gannon (person)  created

        Runs
          No runs yet.
        ");
    });

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

        Proof
          None attached.

        Timeline
          [time]  Gannon (person)  created
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
async fn proof_attach_list_and_show(pool: PgPool) {
    let pl = Pl::start(&pool).await;
    pl.run(&["issue", "create", "--title", "List"]).await;
    let shot = pl.config.with_file_name("shot.png");
    std::fs::write(&shot, vec![7u8; 12_345]).unwrap();
    let shot = shot.to_str().unwrap();

    let attach = pl
        .run(&[
            "proof",
            "attach",
            "PL-1",
            shot,
            "--title",
            "List at 1280×800",
        ])
        .await;
    assert_eq!(attach.code, 0, "{}", attach.stderr);
    insta::with_settings!({filters => time_filters()}, {
        insta::assert_snapshot!(attach.stdout, @r#"Attached [run]: image "List at 1280×800" (12.3 kB) to PL-1 for attempt 0"#);
    });
    let list = pl.run(&["proof", "list", "PL-1"]).await;
    insta::with_settings!({filters => time_filters()}, {
        insta::assert_snapshot!(list.stdout, @r#"[time]  image  "List at 1280×800"  12.3 kB  Gannon (person)  [run]"#);
    });
    let show = pl.run(&["issue", "show", "PL-1"]).await;
    insta::with_settings!({filters => time_filters()}, {
        insta::assert_snapshot!(show.stdout, @r#"
        PL-1  List
        Status: Backlog

        Pull requests
          None linked.

        Proof
          [time]  image  "List at 1280×800"  12.3 kB  Gannon (person)  [run]

        Timeline
          [time]  Gannon (person)  created

        Runs
          No runs yet.
        "#);
    });

    // A wrong kind and a type proof cannot be are refused before anything is stored.
    let wrong = pl
        .run(&[
            "proof", "attach", "PL-1", shot, "--title", "x", "--kind", "video",
        ])
        .await;
    assert_eq!(wrong.code, 1);
    insta::assert_snapshot!(wrong.stderr, @"error: `image/png` is image proof, not `video` (422 Unprocessable Entity)");
    let page = pl.config.with_file_name("page.html");
    std::fs::write(&page, "<p>").unwrap();
    let html = pl
        .run(&[
            "proof",
            "attach",
            "PL-1",
            page.to_str().unwrap(),
            "--title",
            "x",
        ])
        .await;
    assert_eq!(html.code, 1);
    assert!(
        html.stderr.ends_with(
            "page.html is not a file proof can be; use one of .png, .jpg, .jpeg, .gif, .webp, \
             .mp4, .webm, .mov, .txt, .log, .md, .xml, .pdf\n"
        ),
        "{}",
        html.stderr
    );
    let empty = pl.run(&["proof", "list", "PL-2"]).await;
    assert_eq!(empty.code, 1);
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
            models: Default::default(),
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

        Proof
          None attached.

        Gates for In Progress → Agent Review
          pr_open: fail (no open pull request is linked to PL-1)
          pr_ready: fail (no open pull request is linked to PL-1)
          pr_names_issue: fail (no open pull request is linked to PL-1)
          0 of 3 gates pass

        Timeline
          [time]  Gannon (person)  created
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
                lost: false,
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

        Proof
          None attached.

        Gates for In Progress → Agent Review
          pr_open: fail (no open pull request is linked to PL-1)
          pr_ready: fail (no open pull request is linked to PL-1)
          pr_names_issue: fail (no open pull request is linked to PL-1)
          0 of 3 gates pass

        Timeline
          [time]  Gannon (person)  created
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
    insta::with_settings!({filters => time_filters()}, {
        insta::assert_snapshot!(show.stdout, @r"
        PL-1  Fix
        Status: Backlog

        Pull requests
          #7 draft  feature/pl-1-fix  checks passing 2/2  0 open threads
            https://github.com/gannonh/punchlist/pull/7
          #6 open  feature/pl-1-try  no checks  1 open thread
            https://github.com/gannonh/punchlist/pull/6

        Proof
          None attached.

        Timeline
          [time]  Gannon (person)  created

        Runs
          No runs yet.
        ");
    });
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
             UPDATE workspace SET last_event_seq = 2 RETURNING id
         )
         INSERT INTO event (workspace_id, seq, issue_id, kind, actor_id, comment_id)
         SELECT s.id, 2, 'PL-1', 'comment', c.actor_id, c.id FROM s, c",
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

        Proof
          None attached.

        Timeline
          [time]  Gannon (person)  created
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
        "--agents",
        "--codex-model",
        "--codex-effort",
        "--codex-command",
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

    sqlx::query!(
        "UPDATE workspace SET workflow_refused_commit = 'abc123', workflow_refused_at = now(),
         workflow_refused_reason = 'status in use is not in the workflow: `todo` by 1 issue(s)'"
    )
    .execute(&pool)
    .await
    .unwrap();
    let show = pl.run(&["workflow", "show"]).await;
    assert!(
        show.stdout.ends_with(
            "\nRefused   abc123: status in use is not in the workflow: `todo` by 1 issue(s)\n"
        ),
        "{}",
        show.stdout
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

#[sqlx::test(migrator = "punchlist_server::MIGRATOR")]
async fn projects_milestones_and_an_issues_parent_project_and_milestone(pool: PgPool) {
    let pl = Pl::start(&pool).await;
    for title in ["Epic", "Slice", "Task"] {
        pl.run(&["issue", "create", "--title", title]).await;
    }
    assert_eq!(pl.run(&["project", "list"]).await.stdout, "No projects.\n");
    for args in [
        &["project", "create", "Punchlist"][..],
        &["project", "create", "Groundwork"],
        &[
            "milestone",
            "create",
            "Gate 0",
            "--project",
            "Punchlist",
            "--pass-when",
            "pl creates and moves an issue",
        ],
        &["milestone", "create", "Gate 1", "--project", "Punchlist"],
    ] {
        let created = pl.run(args).await;
        assert_eq!(created.code, 0, "{args:?}: {}", created.stderr);
    }
    let list = pl.run(&["project", "list"]).await;
    insta::assert_snapshot!(list.stdout, @r"
    Punchlist
      Gate 0  PASS when pl creates and moves an issue
      Gate 1
    Groundwork
      No milestones.
    ");
    let again = pl.run(&["project", "create", "Punchlist"]).await;
    assert_eq!(
        (again.code, again.stderr.as_str()),
        (
            1,
            "error: a project named Punchlist already exists (422 Unprocessable Entity)\n"
        )
    );

    let edit = async |args: &[&str]| pl.run(&[&["issue", "edit"][..], args].concat()).await;
    let placed = edit(&[
        "PL-2",
        "--parent",
        "PL-1",
        "--project",
        "Punchlist",
        "--milestone",
        "Gate 0",
    ])
    .await;
    assert_eq!(
        (placed.code, placed.stdout.as_str()),
        (
            0,
            "PL-2  parent PL-1 · project Punchlist · milestone Gate 0\n"
        )
    );
    let show = pl.run(&["issue", "show", "PL-2"]).await;
    insta::with_settings!({filters => time_filters()}, {
        insta::assert_snapshot!(show.stdout, @r"
        PL-2  Slice
        Status: Backlog
        Parent: PL-1
        Project: Punchlist
        Milestone: Gate 0

        Pull requests
          None linked.

        Proof
          None attached.

        Timeline
          [time]  Gannon (person)  created
          [time]  Gannon (person)  edited parent, project, milestone

        Runs
          No runs yet.
        ");
    });

    // The issue's own project is where a milestone is looked up, and another project leaves
    // no milestone.
    let gate1 = edit(&["PL-2", "--milestone", "Gate 1"]).await;
    assert_eq!(
        gate1.stdout,
        "PL-2  parent PL-1 · project Punchlist · milestone Gate 1\n"
    );
    let moved = edit(&["PL-2", "--project", "Groundwork"]).await;
    assert_eq!(
        moved.stdout,
        "PL-2  parent PL-1 · project Groundwork · milestone none\n"
    );

    edit(&["PL-3", "--parent", "PL-2"]).await;
    let refused = [
        (
            &["PL-1", "--parent", "PL-3"][..],
            "error: PL-3 is under PL-1 (PL-1 → PL-2 → PL-3), so it cannot be PL-1's parent (422 Unprocessable Entity)\n",
        ),
        (
            &["PL-1", "--milestone", "Gate 0"],
            "error: PL-1 has no project; give --project with --milestone\n",
        ),
        (
            &["PL-1", "--project", "Nope"],
            "error: no project named Nope in this workspace\n",
        ),
        (
            &["PL-1", "--project", "Groundwork", "--milestone", "Gate 0"],
            "error: project Groundwork has no milestone named Gate 0\n",
        ),
    ];
    for (args, stderr) in refused {
        let output = edit(args).await;
        assert_eq!(
            (output.code, output.stderr.as_str()),
            (1, stderr),
            "{args:?}"
        );
    }
    let nothing = edit(&["PL-1"]).await;
    assert_eq!(nothing.code, 2, "{}", nothing.stderr);

    let cleared = edit(&["PL-2", "--no-parent", "--no-project"]).await;
    assert_eq!(
        (cleared.code, cleared.stdout.as_str()),
        (0, "PL-2  parent none · project none · milestone none\n")
    );
    let show = pl.run(&["issue", "show", "PL-2"]).await;
    assert!(
        show.stdout.starts_with("PL-2  Slice\nStatus: Backlog\n\n"),
        "{}",
        show.stdout
    );
}

#[sqlx::test(migrator = "punchlist_server::MIGRATOR")]
async fn a_move_to_start_under_a_version_no_runner_can_claim_warns(pool: PgPool) {
    let pl = Pl::start(&pool).await;
    pl.run(&["issue", "create", "--title", "First"]).await;
    pl.run(&["issue", "move", "PL-1", "todo"]).await;
    // A version stored before the runtime required a dispatch rule for `in_progress`.
    let block = "[dispatch.status.in_progress]\nagent = \"claude-code\"\nprompt = \"prompts/in_progress.md\"\n";
    let old = include_str!("../../punchlist-core/src/default/workflow.toml").replace(block, "");
    sqlx::query(
        "WITH v AS (
             INSERT INTO workflow_version (workspace_id, hash, commit_sha, workflow_toml, prompts)
             SELECT id, 'sha256:old', 'c', $1, $2 FROM workspace RETURNING id)
         UPDATE workspace SET workflow_version_id = (SELECT id FROM v)",
    )
    .bind(&old)
    .bind(serde_json::json!({"prompts/agent_review.md": "Review it."}))
    .execute(&pool)
    .await
    .unwrap();
    let moved = pl.run(&["issue", "move", "PL-1", "start"]).await;
    assert_eq!(
        (moved.code, moved.stdout.as_str()),
        (
            0,
            "PL-1  Todo → Start\n\
             warning: no runner can claim PL-1 under the active workflow: \
             line 4, `statuses`: the runtime needs a dispatch rule for `in_progress`\n"
        )
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

    let shot = pl.config.with_file_name("shot.png");
    std::fs::write(&shot, b"\x89PNG\r\n\x1a\n").unwrap();
    let attach = serde_json::json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call",
        "params": {"name": "attach_proof",
                   "arguments": {"path": shot, "title": "List at 1280×800"}}})
    .to_string();
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
        let attached = exchange(&attach, true);
        drop(stdin);
        assert!(child.wait().unwrap().success());
        [refused, commented, attached]
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
        "Refused (gate_failed): gate pr_open failed: no open pull request is linked to PL-1"
    );
    assert_eq!(result(&lines[1]), (false, "Commented on PL-1.".to_string()));
    let (is_error, text) = result(&lines[2]);
    assert!(!is_error, "{text}");
    assert!(
        text.starts_with("Attached proof ")
            && text.ends_with(" to PL-1: image, 8 bytes, attempt 1."),
        "{text}"
    );
    let list = pl.run(&["proof", "list", "PL-1"]).await;
    insta::with_settings!({filters => time_filters()}, {
        insta::assert_snapshot!(list.stdout, @r#"[time]  image  "List at 1280×800"  8 B  Claude Code (agent, claude-code)  [run]"#);
    });
    let show = pl.run(&["issue", "show", "PL-1"]).await;
    assert!(
        show.stdout.contains(
            "Claude Code (agent, claude-code)  commented:\n    Started.\n\n    (agent)\n"
        ),
        "{}",
        show.stdout
    );
}

/// A stand-in for Linear's GraphQL endpoint that answers with the recorded fixtures, for a
/// personal key sent as it is.
async fn fake_linear() -> String {
    use axum::Json;
    use axum::http::{HeaderMap, StatusCode};
    use serde_json::Value;
    async fn answer(headers: HeaderMap, Json(request): Json<Value>) -> (StatusCode, String) {
        if headers.get("authorization").and_then(|v| v.to_str().ok()) != Some("lin_api_test") {
            return (StatusCode::BAD_REQUEST, fixture("account-disabled"));
        }
        let query = request["query"].as_str().unwrap_or_default();
        let name = if query.starts_with("query Project") {
            "project"
        } else if query.starts_with("query Comments") {
            "comments"
        } else if request["variables"]["after"].is_null() {
            "issues-1"
        } else {
            "issues-2"
        };
        (StatusCode::OK, fixture(name))
    }
    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!(
            "{}/tests/fixtures/linear/{name}.json",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = axum::Router::new().route("/graphql", axum::routing::post(answer));
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}/graphql")
}

#[sqlx::test(migrator = "punchlist_server::MIGRATOR")]
async fn import_linear_twice_then_status(pool: PgPool) {
    let pl = Pl::start(&pool).await;
    tokio::spawn(punchlist_server::work_jobs(AppState::new(pool.clone())));
    let linear = fake_linear().await;
    let workspace = Client::new(&pl.url, &pl.token)
        .unwrap()
        .session()
        .await
        .unwrap()
        .workspace_id
        .to_string();
    let args = [
        "import",
        "linear",
        "--team",
        "KAT",
        "--project",
        "Punchlist",
        "--workspace",
        &workspace,
    ];

    let no_key = pl.run(&args).await;
    assert_eq!(no_key.code, 1);
    assert_eq!(
        no_key.stderr,
        "error: LINEAR_API_KEY is not set; pl reads Linear with your personal API key from the environment and never stores it, such as `with-env pl import linear …`\n"
    );
    let refused = pl
        .run_with(
            &args,
            &[
                ("LINEAR_API_KEY", "lin_api_wrong"),
                ("LINEAR_API_URL", &linear),
            ],
        )
        .await;
    assert_eq!(
        (refused.code, refused.stderr.as_str()),
        (1, "error: Linear refused the query: Account disabled\n")
    );

    let env = [
        ("LINEAR_API_KEY", "lin_api_test"),
        ("LINEAR_API_URL", linear.as_str()),
    ];
    let mut filters = time_filters();
    filters.push((r"\d+(\.\d+)?s\b|\d+m \d+s", "[elapsed]"));
    let first = pl.run_with(&args, &env).await;
    assert_eq!((first.code, first.stderr.as_str()), (0, ""));
    insta::with_settings!({filters => filters.clone()}, {
        insta::assert_snapshot!(first.stdout, @r"
        Read KAT/Punchlist from Linear: 3 issues, 2 comments, 2 milestones in [elapsed].
        Import 1 done.
                     Created  Updated
        Issues             3        0
        Comments           2        0
        Projects           1        0
        Milestones         2        0
        People             1        0
        Elapsed [elapsed].
        ");
    });
    let again = pl.run_with(&args, &env).await;
    assert_eq!(again.code, 0);
    insta::with_settings!({filters => filters.clone()}, {
        insta::assert_snapshot!(again.stdout, @r"
        Read KAT/Punchlist from Linear: 3 issues, 2 comments, 2 milestones in [elapsed].
        Import 2 done.
                     Created  Updated
        Issues             0        0
        Comments           0        0
        Projects           0        0
        Milestones         0        0
        People             0        0
        Elapsed [elapsed].
        ");
    });
    let list = pl.run(&["issue", "list"]).await;
    assert_eq!(list.stdout.lines().count(), 3, "{}", list.stdout);

    let status = pl.run(&["import", "status"]).await;
    assert_eq!(status.code, 0);
    insta::with_settings!({filters => filters}, {
        insta::assert_snapshot!(status.stdout, @r"
        Imported from Linear
          Issues      3
          Comments    2
          Projects    1
          Milestones  2
          People      1
        Last import 2  done  [time]
                     Created  Updated
        Issues             0        0
        Comments           0        0
        Projects           0        0
        Milestones         0        0
        People             0        0
        ");
    });

    let show = pl.run(&["issue", "show", "PL-1"]).await;
    let head: Vec<&str> = show.stdout.lines().take(5).collect();
    assert_eq!(
        head,
        [
            "PL-1  Slice 10: the web app shows the issue page from the approved prototype",
            "Status: Done",
            "Project: Punchlist",
            "Milestone: Gate 1: Groundwork ships from Punchlist",
            "Linear: KAT-3743  https://linear.app/kata-sh/issue/KAT-3743/slice-10-the-web-app-shows-the-issue-page-from-the-approved-prototype",
        ]
    );
    assert!(
        show.stdout.contains("Imported agent (agent, imported)"),
        "{}",
        show.stdout
    );
}
