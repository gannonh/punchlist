//! The workflow loaded from the repository, gated transitions, comments and agent actors,
//! end to end: requests against the router, a fake GitHub for `.punchlist/`, then the rows
//! read back from Postgres.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{Request, StatusCode};
use axum::routing::get;
use hmac::{Hmac, KeyInit, Mac};
use http_body_util::BodyExt;
use punchlist_server::{
    AppState, Bootstrap, Bootstrapped, GithubClient, bootstrap, router, run_jobs_until_idle,
};
use serde_json::{Value, json};
use sha2::Sha256;
use sqlx::PgPool;
use tower::ServiceExt;

const SECRET: &str = "test-webhook-secret";
const OPENED: &str = include_str!("fixtures/github/pull_request_opened.json");
const READY: &str = include_str!("fixtures/github/pull_request_ready_for_review.json");
const MERGED: &str = include_str!("fixtures/github/pull_request_closed_merged.json");
const CLOSED: &str = include_str!("fixtures/github/pull_request_closed_unmerged.json");
const PUSH: &str = include_str!("fixtures/github/push.json");
const PRD_WORKFLOW: &str = include_str!("../../punchlist-core/src/default/workflow.toml");

/// The default branch's head commit, the files under `.punchlist/` there, and the pull
/// requests GitHub currently reports. `down` makes the pull request endpoints fail.
#[derive(Default)]
struct Repo {
    sha: String,
    files: HashMap<String, String>,
    pulls: Vec<Value>,
    down: bool,
}

type FakeGithub = Arc<Mutex<Repo>>;

/// A fake of the GitHub endpoints the workflow load and the pull request refresh call, on a
/// free port.
async fn fake_github() -> (FakeGithub, String) {
    let repo: FakeGithub = Arc::default();
    let app = Router::new()
        .route(
            "/repos/{owner}/{name}",
            get(|| async { axum::Json(json!({"default_branch": "main"})) }),
        )
        .route(
            "/repos/{owner}/{name}/commits/{branch}",
            get(|State(repo): State<FakeGithub>| async move {
                axum::Json(json!({"sha": repo.lock().unwrap().sha}))
            }),
        )
        .route(
            "/repos/{owner}/{name}/pulls",
            get(|State(repo): State<FakeGithub>| async move {
                let repo = repo.lock().unwrap();
                match repo.down {
                    true => (StatusCode::INTERNAL_SERVER_ERROR, axum::Json(json!([]))),
                    false => (StatusCode::OK, axum::Json(Value::Array(repo.pulls.clone()))),
                }
            }),
        )
        .route(
            "/repos/{owner}/{name}/pulls/{number}",
            get(
                |State(repo): State<FakeGithub>,
                 Path((_, _, number)): Path<(String, String, i64)>| async move {
                    let repo = repo.lock().unwrap();
                    match repo.pulls.iter().find(|pr| pr["number"] == number) {
                        Some(pr) if !repo.down => (StatusCode::OK, axum::Json(pr.clone())),
                        _ => (StatusCode::NOT_FOUND, axum::Json(json!({}))),
                    }
                },
            ),
        )
        .route(
            "/repos/{owner}/{name}/contents/.punchlist/{*path}",
            get(
                |State(repo): State<FakeGithub>,
                 Path((_, _, path)): Path<(String, String, String)>| async move {
                    match repo.lock().unwrap().files.get(&path) {
                        Some(text) => (StatusCode::OK, text.clone()),
                        None => (StatusCode::NOT_FOUND, String::new()),
                    }
                },
            ),
        )
        .with_state(repo.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (repo, url)
}

struct Setup {
    app: Router,
    state: AppState,
    person: Bootstrapped,
    github: FakeGithub,
}

async fn setup(pool: &PgPool) -> Setup {
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
    let (github, url) = fake_github().await;
    let state = AppState::new(pool.clone())
        .with_github_webhook_secret(SECRET)
        .with_github(GithubClient::with_token(&url, "test-token"));
    Setup {
        app: router(state.clone()),
        state,
        person,
        github,
    }
}

async fn call(
    app: &Router,
    token: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, json)
}

/// A signed delivery, then the worker run to idle.
async fn deliver(s: &Setup, event: &str, delivery: &str, body: &str) {
    let mut mac = Hmac::<Sha256>::new_from_slice(SECRET.as_bytes()).unwrap();
    mac.update(body.as_bytes());
    let hex: String = mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let request = Request::builder()
        .method("POST")
        .uri("/api/github/webhook")
        .header("content-type", "application/json")
        .header("x-github-event", event)
        .header("x-github-delivery", delivery)
        .header("x-hub-signature-256", format!("sha256={hex}"))
        .body(Body::from(body.to_string()))
        .unwrap();
    let status = s.app.clone().oneshot(request).await.unwrap().status();
    assert_eq!(status, StatusCode::ACCEPTED);
    run_jobs_until_idle(&s.state).await.unwrap();
}

/// Creates PL-1, moves it to Start, and claims it with a runner. Returns the run's agent
/// token.
async fn claimed_issue(s: &Setup) -> String {
    let token = &s.person.token;
    let (status, _) = call(
        &s.app,
        token,
        "POST",
        "/api/issues",
        Some(json!({"title": "Fix the thing"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    for to in ["todo", "start"] {
        let (status, _) = call(
            &s.app,
            token,
            "POST",
            "/api/issues/PL-1/transitions",
            Some(json!({"to": to})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }
    let (status, runner) = call(
        &s.app,
        token,
        "POST",
        "/api/runners",
        Some(json!({"name": "r1", "agents": ["claude-code"]})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let runner_token = runner["token"].as_str().unwrap();
    let (status, claim) = call(
        &s.app,
        runner_token,
        "POST",
        "/api/runs/claim",
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{claim}");
    assert!(
        claim["prompt"]
            .as_str()
            .unwrap()
            .contains("request_transition")
    );
    claim["agent_token"].as_str().unwrap().to_string()
}

async fn request_agent_review(s: &Setup, token: &str) -> (StatusCode, Value) {
    call(
        &s.app,
        token,
        "POST",
        "/api/issues/PL-1/transitions",
        Some(json!({"to": "agent_review"})),
    )
    .await
}

#[sqlx::test]
async fn a_failing_gate_refuses_and_writes_no_transition(pool: PgPool) {
    let s = setup(&pool).await;
    let agent = claimed_issue(&s).await;

    let (status, body) = request_agent_review(&s, &agent).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "gate_failed");
    assert_eq!(body["gate"], "pr_open");
    assert_eq!(body["reason"], "no open pull request is linked to PL-1");

    deliver(&s, "pull_request", "d-opened", OPENED).await;
    let (status, body) = request_agent_review(&s, &agent).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(
        body,
        json!({
            "code": "gate_failed",
            "message": "in_progress → agent_review needs gate pr_ready: pull request #12 is a draft",
            "gate": "pr_ready",
            "reason": "pull request #12 is a draft",
            "evidence": {
                "issue_id": "PL-1",
                "pull_requests": [{
                    "number": 12,
                    "title": "Fix the thing",
                    "branch": "feature/pl-1-fix-the-thing",
                    "state": "open",
                    "draft": true
                }]
            },
            "gates": [
                {"gate": "pr_open", "passed": true, "reason": "pull request #12 is open"},
                {"gate": "pr_ready", "passed": false, "reason": "pull request #12 is a draft"},
                {"gate": "pr_names_issue", "passed": true, "reason": "pull request #12's branch names PL-1"}
            ]
        })
    );
    let moved = sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!" FROM transition WHERE to_status = 'agent_review'"#
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(moved, 0);
    let status = sqlx::query_scalar!("SELECT status FROM issue WHERE id = 'PL-1'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "in_progress");
}

/// What GitHub's API returns for a pull request: the object a webhook carries.
fn pull_request_now(webhook: &str) -> Value {
    serde_json::from_str::<Value>(webhook).unwrap()["pull_request"].clone()
}

#[sqlx::test]
async fn a_pull_request_whose_webhook_has_not_arrived_is_read_from_github(pool: PgPool) {
    let s = setup(&pool).await;
    let agent = claimed_issue(&s).await;
    s.github.lock().unwrap().pulls = vec![pull_request_now(OPENED)];

    // No delivery yet: without the refresh this is pr_open, "no open pull request".
    let (status, body) = request_agent_review(&s, &agent).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["gate"], "pr_ready");
    assert_eq!(body["reason"], "pull request #12 is a draft");
}

#[sqlx::test]
async fn a_pull_request_made_ready_passes_before_its_webhook_arrives(pool: PgPool) {
    let s = setup(&pool).await;
    let agent = claimed_issue(&s).await;
    deliver(&s, "pull_request", "d-opened", OPENED).await;
    let (status, body) = request_agent_review(&s, &agent).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["reason"], "pull request #12 is a draft");

    // GitHub says ready; the ready_for_review delivery has not come.
    s.github.lock().unwrap().pulls = vec![pull_request_now(READY)];
    let (status, body) = request_agent_review(&s, &agent).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["issue"]["status"], "agent_review");
    let results = sqlx::query_scalar!(
        "SELECT g.result FROM transition_gate g JOIN transition t ON t.id = g.transition_id
         WHERE t.to_status = 'agent_review' ORDER BY g.position"
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(results, ["pass", "pass", "pass"]);

    // The late delivery changes nothing.
    deliver(&s, "pull_request", "d-ready", READY).await;
}

#[sqlx::test]
async fn recorded_pull_requests_stand_when_github_is_down(pool: PgPool) {
    let s = setup(&pool).await;
    let agent = claimed_issue(&s).await;
    deliver(&s, "pull_request", "d-opened", OPENED).await;
    deliver(&s, "pull_request", "d-ready", READY).await;
    s.github.lock().unwrap().down = true;

    let (status, body) = request_agent_review(&s, &agent).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[sqlx::test]
async fn passing_gates_are_recorded_on_the_transition(pool: PgPool) {
    let s = setup(&pool).await;
    let agent = claimed_issue(&s).await;
    deliver(&s, "pull_request", "d-opened", OPENED).await;
    deliver(&s, "pull_request", "d-ready", READY).await;

    let (status, body) = request_agent_review(&s, &agent).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["issue"]["status"], "agent_review");
    assert_eq!(body["event"]["actor"]["role"], "agent");
    assert_eq!(body["event"]["actor"]["name"], "Claude Code");

    let rows = sqlx::query!(
        "SELECT g.gate, g.result, g.reason FROM transition_gate g
         JOIN transition t ON t.id = g.transition_id
         WHERE t.to_status = 'agent_review' ORDER BY g.position"
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let rows: Vec<(String, String, String)> = rows
        .into_iter()
        .map(|r| (r.gate, r.result, r.reason))
        .collect();
    assert_eq!(
        rows,
        [
            (
                "pr_open".into(),
                "pass".into(),
                "pull request #12 is open".into()
            ),
            (
                "pr_ready".into(),
                "pass".into(),
                "pull request #12 is ready for review".into()
            ),
            (
                "pr_names_issue".into(),
                "pass".into(),
                "pull request #12's branch names PL-1".into()
            ),
        ]
    );
}

#[sqlx::test]
async fn agent_comments_end_with_agent_and_stay_on_their_issue(pool: PgPool) {
    let s = setup(&pool).await;
    let agent = claimed_issue(&s).await;
    let (status, event) = call(
        &s.app,
        &agent,
        "POST",
        "/api/issues/PL-1/comments",
        Some(json!({"body": "Opened #12."})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{event}");
    assert_eq!(
        event["detail"],
        json!({"kind": "comment", "body": "Opened #12.\n\n(agent)"})
    );

    let (_, events) = call(
        &s.app,
        &s.person.token,
        "GET",
        "/api/issues/PL-1/events",
        None,
    )
    .await;
    let last = events.as_array().unwrap().last().unwrap();
    assert_eq!(last["detail"]["body"], "Opened #12.\n\n(agent)");
    assert_eq!(last["actor"]["role"], "agent");

    // A person's comment is kept as written.
    let (status, event) = call(
        &s.app,
        &s.person.token,
        "POST",
        "/api/issues/PL-1/comments",
        Some(json!({"body": "Thanks"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(event["detail"]["body"], "Thanks");

    // An agent acts only on its run's issue.
    let (status, _) = call(
        &s.app,
        &s.person.token,
        "POST",
        "/api/issues",
        Some(json!({"title": "Other"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, body) = call(
        &s.app,
        &agent,
        "POST",
        "/api/issues/PL-2/comments",
        Some(json!({"body": "Hi"})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    // Its token stops working when the run ends.
    sqlx::query!("UPDATE run SET outcome = 'failed', finished_at = now()")
        .execute(&pool)
        .await
        .unwrap();
    let (status, _) = call(&s.app, &agent, "GET", "/api/issues/PL-1", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[sqlx::test]
async fn a_locked_status_refuses_agent_comments(pool: PgPool) {
    let s = setup(&pool).await;
    let agent = claimed_issue(&s).await;
    // Test-only: the path to Human Review needs gates that a later slice evaluates.
    sqlx::query!("UPDATE issue SET status = 'human_review' WHERE id = 'PL-1'")
        .execute(&pool)
        .await
        .unwrap();
    let (status, body) = call(
        &s.app,
        &agent,
        "POST",
        "/api/issues/PL-1/comments",
        Some(json!({"body": "Still here"})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "locked");
}

async fn workflow_versions(pool: &PgPool) -> i64 {
    sqlx::query_scalar!(r#"SELECT count(*) AS "n!" FROM workflow_version"#)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test]
async fn a_push_loads_the_workflow_and_an_invalid_one_is_refused(pool: PgPool) {
    let s = setup(&pool).await;
    let token = s.person.token.clone();
    let (_, before) = call(&s.app, &token, "GET", "/api/workflow", None).await;
    assert_eq!(before["commit_sha"], Value::Null);

    {
        let mut repo = s.github.lock().unwrap();
        repo.sha = "a".repeat(40);
        repo.files
            .insert("workflow.toml".into(), PRD_WORKFLOW.into());
        repo.files
            .insert("prompts/system.md".into(), "System.\n".into());
        repo.files
            .insert("prompts/in_progress.md".into(), "Build it.\n".into());
        repo.files
            .insert("prompts/agent_review.md".into(), "Review it.\n".into());
    }
    deliver(&s, "push", "push-1", PUSH).await;
    let (_, loaded) = call(&s.app, &token, "GET", "/api/workflow", None).await;
    assert_eq!(loaded["commit_sha"], "a".repeat(40));
    assert_eq!(loaded["statuses"].as_array().unwrap().len(), 9);
    assert_ne!(loaded["version"], before["version"]);
    let hash = sqlx::query_scalar!("SELECT hash FROM workflow_version")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(loaded["version"], hash);

    // Transitions record the loaded version.
    let (status, issue) = call(
        &s.app,
        &token,
        "POST",
        "/api/issues",
        Some(json!({"title": "X"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (_, moved) = call(
        &s.app,
        &token,
        "POST",
        &format!("/api/issues/{}/transitions", issue["id"].as_str().unwrap()),
        Some(json!({"to": "todo"})),
    )
    .await;
    assert_eq!(moved["event"]["detail"]["workflow_version"], hash);

    // An unknown gate is refused; the loaded version stays.
    {
        let mut repo = s.github.lock().unwrap();
        repo.sha = "b".repeat(40);
        repo.files.insert(
            "workflow.toml".into(),
            PRD_WORKFLOW.replace("\"pr_ready\"", "\"pr_redy\""),
        );
    }
    deliver(&s, "push", "push-2", PUSH).await;
    assert_eq!(workflow_versions(&pool).await, 1);
    let (_, after) = call(&s.app, &token, "GET", "/api/workflow", None).await;
    assert_eq!(after, loaded);

    // A push to another branch, or one that leaves .punchlist/ alone, loads nothing.
    s.github.lock().unwrap().files.insert(
        "workflow.toml".into(),
        PRD_WORKFLOW.replace("max_concurrent = 5", "max_concurrent = 4"),
    );
    let other_branch = PUSH.replace("refs/heads/main", "refs/heads/feature/pl-1-x");
    deliver(&s, "push", "push-3", &other_branch).await;
    let elsewhere = PUSH
        .replace(".punchlist/workflow.toml", "README.md")
        .replace(".punchlist/prompts/system.md", "src/lib.rs");
    deliver(&s, "push", "push-4", &elsewhere).await;
    assert_eq!(workflow_versions(&pool).await, 1);
    let jobs =
        sqlx::query_scalar!(r#"SELECT count(*) AS "n!" FROM job WHERE kind = 'workflow_load'"#)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(jobs, 2);
}

/// Loads the PRD workflow with `gate` on its `on = "<event>"` transition, puts PL-1 in
/// `from` and delivers `closed`, the pull request event that closes its pull request.
/// Returns PL-1's status.
async fn close_with_gate(s: &Setup, event: &str, gate: &str, from: &str, closed: &str) -> String {
    let on = format!("on = \"{event}\"");
    let gated = PRD_WORKFLOW.replace(&on, &format!("{on}\ngates = [\"{gate}\"]"));
    // The template still has the line this replaces, so the gate is in the file.
    assert_eq!(gated.matches(&format!("gates = [\"{gate}\"]")).count(), 1);
    {
        let mut repo = s.github.lock().unwrap();
        repo.sha = "a".repeat(40);
        repo.files.insert("workflow.toml".into(), gated);
        for prompt in ["system", "in_progress", "agent_review"] {
            repo.files
                .insert(format!("prompts/{prompt}.md"), "Prompt.\n".into());
        }
    }
    deliver(s, "push", "push-1", PUSH).await;
    assert_eq!(workflow_versions(&s.state.pool).await, 1);
    let (status, _) = call(
        &s.app,
        &s.person.token,
        "POST",
        "/api/issues",
        Some(json!({"title": "Fix the thing"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    // A person has no short path to these statuses, so the test puts the issue there.
    sqlx::query("UPDATE issue SET status = $1 WHERE id = 'PL-1'")
        .bind(from)
        .execute(&s.state.pool)
        .await
        .unwrap();
    deliver(s, "pull_request", "closed-1", closed).await;
    // Redelivery, and the same event under another delivery id, change nothing.
    deliver(s, "pull_request", "closed-1", closed).await;
    deliver(s, "pull_request", "closed-2", closed).await;
    sqlx::query_scalar!("SELECT status FROM issue WHERE id = 'PL-1'")
        .fetch_one(&s.state.pool)
        .await
        .unwrap()
}

/// How many transitions, comments and GitHub actors there are. A person made none of them
/// in these tests.
async fn rows_from_github(pool: &PgPool) -> (i64, i64, i64) {
    sqlx::query_as(
        "SELECT (SELECT count(*) FROM transition), (SELECT count(*) FROM comment),
                (SELECT count(*) FROM actor WHERE role = 'github')",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

#[sqlx::test]
async fn gates_on_pr_closed_unmerged_are_checked(pool: PgPool) {
    let s = setup(&pool).await;
    assert_eq!(
        close_with_gate(
            &s,
            "pr_closed_unmerged",
            "proof_attached",
            "in_progress",
            CLOSED
        )
        .await,
        "in_progress"
    );
    assert_eq!(rows_from_github(&pool).await, (0, 0, 0));
}

#[sqlx::test]
async fn a_failed_gate_is_final_when_the_close_arrives_again_and_would_now_pass(pool: PgPool) {
    let s = setup(&pool).await;
    assert_eq!(
        close_with_gate(&s, "pr_closed_unmerged", "pr_open", "in_progress", CLOSED).await,
        "in_progress"
    );
    // Another pull request opens for the issue, so `pr_open` would pass now.
    let mut opened: Value = serde_json::from_str(OPENED).unwrap();
    opened["number"] = json!(13);
    opened["pull_request"]["number"] = json!(13);
    opened["pull_request"]["id"] = json!(1934102999_i64);
    deliver(&s, "pull_request", "opened-13", &opened.to_string()).await;
    deliver(&s, "pull_request", "closed-3", CLOSED).await;
    let status = sqlx::query_scalar!("SELECT status FROM issue WHERE id = 'PL-1'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "in_progress");
    assert_eq!(rows_from_github(&pool).await, (0, 0, 0));
}

#[sqlx::test]
async fn a_passing_gate_on_pr_closed_unmerged_moves_the_issue_once(pool: PgPool) {
    let s = setup(&pool).await;
    assert_eq!(
        close_with_gate(
            &s,
            "pr_closed_unmerged",
            "pr_names_issue",
            "in_progress",
            CLOSED
        )
        .await,
        "todo"
    );
    assert_eq!(rows_from_github(&pool).await, (1, 1, 1));
}

#[sqlx::test]
async fn a_failing_gate_on_pr_merged_leaves_the_issue_in_merging(pool: PgPool) {
    let s = setup(&pool).await;
    assert_eq!(
        close_with_gate(&s, "pr_merged", "proof_attached", "merging", MERGED).await,
        "merging"
    );
    assert_eq!(rows_from_github(&pool).await, (0, 0, 0));
    let state: String = sqlx::query_scalar("SELECT state FROM pull_request WHERE number = 12")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(state, "merged");
}

#[sqlx::test]
async fn a_passing_gate_on_pr_merged_is_recorded_on_the_move(pool: PgPool) {
    let s = setup(&pool).await;
    assert_eq!(
        close_with_gate(&s, "pr_merged", "pr_names_issue", "merging", MERGED).await,
        "done"
    );
    let gates: Vec<(String, String, String, String, String)> = sqlx::query_as(
        "SELECT t.from_status, t.to_status, g.gate, g.result, g.reason
         FROM transition t LEFT JOIN transition_gate g ON g.transition_id = t.id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        gates,
        vec![(
            "merging".into(),
            "done".into(),
            "pr_names_issue".into(),
            "pass".into(),
            "pull request #12's branch names PL-1".into()
        )]
    );
}

#[sqlx::test]
async fn a_missing_prompt_is_refused(pool: PgPool) {
    let s = setup(&pool).await;
    {
        let mut repo = s.github.lock().unwrap();
        repo.sha = "a".repeat(40);
        repo.files
            .insert("workflow.toml".into(), PRD_WORKFLOW.into());
    }
    deliver(&s, "push", "push-1", PUSH).await;
    assert_eq!(workflow_versions(&pool).await, 0);
}

#[sqlx::test]
async fn the_schema_needs_no_token(pool: PgPool) {
    let s = setup(&pool).await;
    let request = Request::builder()
        .uri("/api/workflow/schema.json")
        .body(Body::empty())
        .unwrap();
    let response = s.app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let schema: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(schema["title"], "Punchlist workflow");
    assert!(schema.to_string().contains("pr_names_issue"));
}

#[sqlx::test]
async fn an_agent_token_reaches_only_its_runs_issue(pool: PgPool) {
    let s = setup(&pool).await;
    let agent = claimed_issue(&s).await;
    let (status, _) = call(
        &s.app,
        &s.person.token,
        "POST",
        "/api/issues",
        Some(json!({"title": "Another"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    for uri in [
        "/api/issues/PL-1",
        "/api/issues/PL-1/events",
        "/api/issues/PL-1/pull-requests",
    ] {
        let (status, body) = call(&s.app, &agent, "GET", uri, None).await;
        assert_eq!(status, StatusCode::OK, "{uri}: {body}");
    }
    for (method, uri, body) in [
        ("POST", "/api/issues", Some(json!({"title": "Mine now"}))),
        ("GET", "/api/issues", None),
        ("GET", "/api/issues/PL-2", None),
        ("GET", "/api/issues/PL-2/events", None),
        ("GET", "/api/issues/PL-10", None),
        ("GET", "/api/pull-requests/unlinked", None),
        ("GET", "/api/workflow", None),
    ] {
        let (status, error) = call(&s.app, &agent, method, uri, body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}: {error}");
        assert_eq!(
            error["message"],
            "an agent acts only on its run's issue, PL-1"
        );
    }
    let issues = sqlx::query_scalar!(r#"SELECT count(*) AS "n!" FROM issue"#)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(issues, 2);
}

#[sqlx::test]
async fn a_reverted_workflow_reports_the_commit_it_was_loaded_from(pool: PgPool) {
    let s = setup(&pool).await;
    let token = s.person.token.clone();
    {
        let mut repo = s.github.lock().unwrap();
        for (path, text) in [
            ("prompts/system.md", "System.\n"),
            ("prompts/in_progress.md", "Build it.\n"),
            ("prompts/agent_review.md", "Review it.\n"),
        ] {
            repo.files.insert(path.into(), text.into());
        }
    }
    let changed = PRD_WORKFLOW.replace("max_concurrent = 5", "max_concurrent = 4");
    let mut loaded = Vec::new();
    for (n, (sha, file)) in [
        ("a", PRD_WORKFLOW),
        ("b", changed.as_str()),
        ("c", PRD_WORKFLOW),
    ]
    .into_iter()
    .enumerate()
    {
        {
            let mut repo = s.github.lock().unwrap();
            repo.sha = sha.repeat(40);
            repo.files.insert("workflow.toml".into(), file.into());
        }
        deliver(&s, "push", &format!("push-{n}"), PUSH).await;
        let (_, now) = call(&s.app, &token, "GET", "/api/workflow", None).await;
        loaded.push((now["version"].clone(), now["commit_sha"].clone()));
    }
    assert_eq!(loaded[0].0, loaded[2].0);
    assert_ne!(loaded[0].0, loaded[1].0);
    assert_eq!(
        loaded.iter().map(|l| l.1.clone()).collect::<Vec<_>>(),
        ["a".repeat(40), "b".repeat(40), "c".repeat(40)]
    );
    assert_eq!(workflow_versions(&pool).await, 2);
}
