//! The workflow loaded from the repository, gated transitions, comments and agent actors,
//! end to end: requests against the router, a fake GitHub for `.punchlist/`, then the rows
//! read back from Postgres.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{Request, StatusCode};
use axum::routing::{get, post};
use hmac::{Hmac, KeyInit, Mac};
use http_body_util::BodyExt;
use punchlist_core::Workflow;
use punchlist_server::{
    AppState, Bootstrap, Bootstrapped, GithubClient, bootstrap, router, run_jobs_until_idle,
    warn_invalid_workflows,
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
/// requests and review threads GitHub currently reports; every pull request has the same
/// threads. `down` makes the pull request and review thread endpoints fail.
#[derive(Default)]
struct Repo {
    sha: String,
    files: HashMap<String, String>,
    pulls: Vec<Value>,
    threads: Vec<Value>,
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
        // GraphQL `reviewThreads`, two to a page; the cursor is the next thread's index.
        .route(
            "/graphql",
            post(
                |State(repo): State<FakeGithub>, axum::Json(query): axum::Json<Value>| async move {
                    let repo = repo.lock().unwrap();
                    if repo.down {
                        return (StatusCode::BAD_GATEWAY, axum::Json(json!({})));
                    }
                    let after = &query["variables"]["after"];
                    let from = after.as_str().map_or(0, |cursor| cursor.parse().unwrap());
                    let to = repo.threads.len().min(from + 2);
                    let threads = json!({
                        "nodes": repo.threads[from..to],
                        "pageInfo": {
                            "hasNextPage": to < repo.threads.len(),
                            "endCursor": to.to_string(),
                        },
                    });
                    let data = json!({"repository": {"pullRequest": {"reviewThreads": threads}}});
                    (StatusCode::OK, axum::Json(json!({"data": data})))
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

/// Every transition PL-1 made on a GitHub event: where from and to, who, and the delivery.
async fn event_transitions(pool: &PgPool) -> Vec<(String, String, String, Option<String>)> {
    sqlx::query!(
        "SELECT t.from_status, t.to_status, a.name, t.delivery_id FROM transition t
         JOIN actor a ON a.id = t.actor_id
         WHERE t.issue_id = 'PL-1' AND a.role = 'github' ORDER BY t.created_at"
    )
    .fetch_all(pool)
    .await
    .unwrap()
    .into_iter()
    .map(|r| (r.from_status, r.to_status, r.name, r.delivery_id))
    .collect()
}

fn moved(
    from: &str,
    to: &str,
    by: &str,
    delivery: Option<&str>,
) -> (String, String, String, Option<String>) {
    (
        from.into(),
        to.into(),
        by.into(),
        delivery.map(str::to_string),
    )
}

/// A webhook fixture as pull request `number`.
fn numbered(webhook: &str, number: i64) -> String {
    let mut event: Value = serde_json::from_str(webhook).unwrap();
    event["number"] = json!(number);
    event["pull_request"]["number"] = json!(number);
    event.to_string()
}

async fn issue_status(pool: &PgPool) -> String {
    sqlx::query_scalar!("SELECT status FROM issue WHERE id = 'PL-1'")
        .fetch_one(pool)
        .await
        .unwrap()
}

/// PL-1 in In Progress with pull request #12 recorded open and ready.
async fn issue_with_a_ready_pull_request(s: &Setup) -> String {
    let agent = claimed_issue(s).await;
    deliver(s, "pull_request", "d-opened", OPENED).await;
    deliver(s, "pull_request", "d-ready", READY).await;
    agent
}

const MERGED_REASON: &str = "no open pull request is linked to PL-1: pull request #12 is merged";

#[sqlx::test]
async fn a_pull_request_merged_on_github_fails_pr_open_before_its_webhook_arrives(pool: PgPool) {
    let s = setup(&pool).await;
    let agent = issue_with_a_ready_pull_request(&s).await;
    s.github.lock().unwrap().pulls = vec![pull_request_now(MERGED)];

    let (status, body) = request_agent_review(&s, &agent).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["gate"], "pr_open");
    assert_eq!(body["reason"], MERGED_REASON);
    assert_eq!(body["evidence"]["pull_requests"][0]["state"], "merged");

    // The PRD's workflow moves no issue on a merge from In Progress, read or delivered.
    deliver(&s, "pull_request", "d-merged", MERGED).await;
    let (_, body) = request_agent_review(&s, &agent).await;
    assert_eq!(body["reason"], MERGED_REASON);
    assert_eq!(issue_status(&pool).await, "in_progress");
    assert_eq!(event_transitions(&pool).await, []);
}

/// Loads the PRD's workflow with `in_progress → done` on `pr_merged` as well.
async fn load_a_workflow_that_finishes_on_merge(s: &Setup) {
    {
        let mut repo = s.github.lock().unwrap();
        repo.sha = "a".repeat(40);
        let workflow = PRD_WORKFLOW.replace(
            "from = \"merging\"\nto = \"done\"",
            "from = [\"in_progress\", \"merging\"]\nto = \"done\"",
        );
        assert_ne!(workflow, PRD_WORKFLOW);
        repo.files.insert("workflow.toml".into(), workflow);
        for (path, text) in [
            ("prompts/system.md", "Call request_transition.\n"),
            ("prompts/in_progress.md", "Build it.\n"),
            ("prompts/agent_review.md", "Review it.\n"),
        ] {
            repo.files.insert(path.into(), text.into());
        }
    }
    deliver(s, "push", "push-1", PUSH).await;
    assert_eq!(workflow_versions(&s.state.pool).await, 1);
}

#[sqlx::test]
async fn a_merge_read_from_github_moves_the_issue_once(pool: PgPool) {
    let s = setup(&pool).await;
    load_a_workflow_that_finishes_on_merge(&s).await;
    let agent = issue_with_a_ready_pull_request(&s).await;
    let mut merged = pull_request_now(MERGED);
    merged["merged_by"] = json!({"login": "octocat"});
    s.github.lock().unwrap().pulls = vec![merged];

    // The merge moves the issue first and ends the agent's run, as its webhook would have,
    // so the request is answered as it would be after that webhook.
    let (status, body) = request_agent_review(&s, &agent).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(
        body,
        json!({"code": "unauthorized", "message": "missing or unknown bearer token"})
    );
    let once = [moved("in_progress", "done", "octocat", None)];
    assert_eq!(event_transitions(&pool).await, once);

    // Its webhook then arrives, and is redelivered.
    deliver(&s, "pull_request", "d-merged", MERGED).await;
    deliver(&s, "pull_request", "d-merged-again", MERGED).await;
    assert_eq!(event_transitions(&pool).await, once);
    assert_eq!(issue_status(&pool).await, "done");
}

#[sqlx::test]
async fn a_merge_delivered_first_moves_the_issue_once(pool: PgPool) {
    let s = setup(&pool).await;
    load_a_workflow_that_finishes_on_merge(&s).await;
    let agent = issue_with_a_ready_pull_request(&s).await;
    s.github.lock().unwrap().pulls = vec![pull_request_now(MERGED)];

    deliver(&s, "pull_request", "d-merged", MERGED).await;
    let once = [moved("in_progress", "done", "octocat", Some("d-merged"))];
    assert_eq!(event_transitions(&pool).await, once);

    // The move ended the agent's run, so its request no longer authenticates; a redelivery
    // changes nothing either.
    let (status, _) = request_agent_review(&s, &agent).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    deliver(&s, "pull_request", "d-merged-again", MERGED).await;
    assert_eq!(event_transitions(&pool).await, once);
    assert_eq!(issue_status(&pool).await, "done");
}

#[sqlx::test]
async fn a_pull_request_closed_on_github_moves_the_issue_once_before_the_request(pool: PgPool) {
    let s = setup(&pool).await;
    let agent = issue_with_a_ready_pull_request(&s).await;
    s.github.lock().unwrap().pulls = vec![pull_request_now(CLOSED)];

    let (status, body) = request_agent_review(&s, &agent).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(
        body,
        json!({"code": "unauthorized", "message": "missing or unknown bearer token"})
    );

    // GitHub's pull request does not say who closed it, so nobody is credited.
    let once = [moved("in_progress", "todo", "GitHub", None)];
    assert_eq!(event_transitions(&pool).await, once);
    deliver(&s, "pull_request", "d-closed", CLOSED).await;
    assert_eq!(event_transitions(&pool).await, once);
    assert_eq!(issue_status(&pool).await, "todo");
    let comments = sqlx::query_scalar!("SELECT body FROM comment WHERE issue_id = 'PL-1'")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(
        comments,
        ["Pull request #12 was closed without merging.\n\n\
          https://github.com/gannonh/punchlist/pull/12"]
    );
}

#[sqlx::test]
async fn one_of_two_pull_requests_closed_on_github_answers_with_the_status_it_left(pool: PgPool) {
    let s = setup(&pool).await;
    let agent = issue_with_a_ready_pull_request(&s).await;
    deliver(&s, "pull_request", "d-ready-11", &numbered(READY, 11)).await;
    s.github.lock().unwrap().pulls = vec![
        pull_request_now(&numbered(CLOSED, 11)),
        pull_request_now(READY),
    ];

    // #12 alone would pass the gates, but closing #11 sent the issue back to Todo first and
    // ended the run whose token made the request.
    let (status, body) = request_agent_review(&s, &agent).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(
        body,
        json!({"code": "unauthorized", "message": "missing or unknown bearer token"})
    );
    assert_eq!(issue_status(&pool).await, "todo");
    assert_eq!(
        event_transitions(&pool).await,
        [moved("in_progress", "todo", "GitHub", None)]
    );
}

#[sqlx::test]
async fn a_pull_request_closed_before_it_was_ever_recorded_moves_nothing(pool: PgPool) {
    let s = setup(&pool).await;
    let agent = issue_with_a_ready_pull_request(&s).await;
    // #9 named PL-1 and was closed while no webhook reached the server.
    s.github.lock().unwrap().pulls = vec![
        pull_request_now(&numbered(CLOSED, 9)),
        pull_request_now(READY),
    ];

    let (status, body) = request_agent_review(&s, &agent).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["issue"]["status"], "agent_review");
    assert_eq!(issue_status(&pool).await, "agent_review");
    assert_eq!(event_transitions(&pool).await, []);
    let numbers = sqlx::query_scalar!("SELECT number FROM pull_request ORDER BY number")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(numbers, [12]);
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

/// A webhook fixture with `edit` applied to its JSON.
fn edited(fixture: &str, edit: impl FnOnce(&mut Value)) -> String {
    let mut value: Value = serde_json::from_str(fixture).unwrap();
    edit(&mut value);
    value.to_string()
}

/// #12's state and PL-1's status.
async fn pull_request_and_issue(pool: &PgPool) -> (String, String) {
    sqlx::query_as(
        "SELECT p.state, i.status FROM pull_request p JOIN issue i ON i.id = p.issue_id
         WHERE p.number = 12",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

#[sqlx::test]
async fn a_closed_delivered_after_the_reopened_of_the_same_second_leaves_it_open(pool: PgPool) {
    let s = setup(&pool).await;
    issue_with_a_ready_pull_request(&s).await;
    // Closed and reopened within 18:30:00. GitHub now reports it open, and delivers the
    // `reopened` first.
    let second = json!("2026-10-09T18:30:00Z");
    let reopened = edited(READY, |v| {
        v["action"] = json!("reopened");
        v["pull_request"]["updated_at"] = second.clone();
    });
    let closed: Value = serde_json::from_str(CLOSED).unwrap();
    assert_eq!(closed["pull_request"]["updated_at"], second);
    s.github.lock().unwrap().pulls = vec![pull_request_now(&reopened)];

    deliver(&s, "pull_request", "d-reopened", &reopened).await;
    deliver(&s, "pull_request", "d-closed", CLOSED).await;

    assert_eq!(
        pull_request_and_issue(&pool).await,
        ("open".into(), "in_progress".into())
    );
    assert_eq!(event_transitions(&pool).await, []);
    let jobs: Vec<(String, String)> = sqlx::query_as(
        "SELECT idempotency_key, state FROM job WHERE idempotency_key IN ('d-reopened', 'd-closed')
         ORDER BY id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        jobs,
        [
            ("d-reopened".into(), "done".into()),
            ("d-closed".into(), "done".into())
        ]
    );
}

#[sqlx::test]
async fn a_closed_delivery_is_retried_not_recorded_while_github_is_down(pool: PgPool) {
    let s = setup(&pool).await;
    issue_with_a_ready_pull_request(&s).await;
    {
        let mut repo = s.github.lock().unwrap();
        repo.pulls = vec![pull_request_now(CLOSED)];
        repo.down = true;
    }

    deliver(&s, "pull_request", "d-closed", CLOSED).await;
    let job: (String, i32, Option<String>) = sqlx::query_as(
        "SELECT state, attempts, last_error FROM job WHERE idempotency_key = 'd-closed'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        job,
        (
            "queued".into(),
            1,
            Some("GitHub /repos/gannonh/punchlist/pulls/12 answered 404 Not Found".into())
        )
    );
    assert_eq!(
        pull_request_and_issue(&pool).await,
        ("open".into(), "in_progress".into())
    );

    // GitHub answers again, and the retry records the close and moves the issue.
    s.github.lock().unwrap().down = false;
    sqlx::query("UPDATE job SET run_after = now()")
        .execute(&pool)
        .await
        .unwrap();
    run_jobs_until_idle(&s.state).await.unwrap();
    assert_eq!(
        pull_request_and_issue(&pool).await,
        ("closed".into(), "todo".into())
    );
    assert_eq!(
        event_transitions(&pool).await,
        [moved("in_progress", "todo", "octocat", Some("d-closed"))]
    );
}

#[sqlx::test]
async fn a_closed_delivery_for_a_pull_request_merged_since_records_the_merge_once(pool: PgPool) {
    let s = setup(&pool).await;
    load_a_workflow_that_finishes_on_merge(&s).await;
    issue_with_a_ready_pull_request(&s).await;
    // Closed, reopened and merged; the `closed` is the first of them to arrive.
    let mut merged = pull_request_now(MERGED);
    merged["merged_by"] = json!({"login": "hubot"});
    s.github.lock().unwrap().pulls = vec![merged];

    deliver(&s, "pull_request", "d-closed", CLOSED).await;
    let once = [moved("in_progress", "done", "hubot", Some("d-closed"))];
    assert_eq!(event_transitions(&pool).await, once);

    // Merged is final: its own delivery, and the `closed` again, change nothing.
    deliver(&s, "pull_request", "d-merged", MERGED).await;
    deliver(&s, "pull_request", "d-closed-again", CLOSED).await;
    assert_eq!(event_transitions(&pool).await, once);
    assert_eq!(
        pull_request_and_issue(&pool).await,
        ("merged".into(), "done".into())
    );
}

const THREAD: &str = include_str!("fixtures/github/pull_request_review_thread_resolved.json");

/// A review thread as GraphQL reports it.
fn thread(id: &str, resolved: bool) -> Value {
    json!({"id": id, "isResolved": resolved})
}

/// Every recorded review thread, and #12's count of open ones.
async fn recorded_threads(pool: &PgPool) -> (Vec<(String, bool)>, i32) {
    let threads = sqlx::query_as("SELECT node_id, resolved FROM review_thread ORDER BY node_id")
        .fetch_all(pool)
        .await
        .unwrap();
    let open = sqlx::query_scalar("SELECT open_threads FROM pull_request WHERE number = 12")
        .fetch_one(pool)
        .await
        .unwrap();
    (threads, open)
}

#[sqlx::test]
async fn a_resolved_delivered_after_the_newer_unresolved_leaves_the_thread_open(pool: PgPool) {
    let s = setup(&pool).await;
    issue_with_a_ready_pull_request(&s).await;
    // Resolved, then unresolved: GitHub reports it unresolved, and delivers that first.
    s.github.lock().unwrap().threads = vec![thread("PRRT_kwDOA1b2c84AbCdE", false)];
    let unresolved = edited(THREAD, |v| v["action"] = json!("unresolved"));

    deliver(
        &s,
        "pull_request_review_thread",
        "d-unresolved",
        &unresolved,
    )
    .await;
    deliver(&s, "pull_request_review_thread", "d-resolved", THREAD).await;

    assert_eq!(
        recorded_threads(&pool).await,
        (vec![("PRRT_kwDOA1b2c84AbCdE".into(), false)], 1)
    );
}

#[sqlx::test]
async fn a_thread_event_records_every_thread_github_has(pool: PgPool) {
    let s = setup(&pool).await;
    issue_with_a_ready_pull_request(&s).await;
    // Three threads, so two pages. No event has named the second or the third.
    s.github.lock().unwrap().threads = vec![
        thread("PRRT_kwDOA1b2c84AbCdE", true),
        thread("PRRT_kwDOA1b2c84ZyXwV", false),
        thread("PRRT_kwDOA1b2c84MnOpQ", false),
    ];
    deliver(&s, "pull_request_review_thread", "d-resolved", THREAD).await;
    assert_eq!(
        recorded_threads(&pool).await,
        (
            vec![
                ("PRRT_kwDOA1b2c84AbCdE".into(), true),
                ("PRRT_kwDOA1b2c84MnOpQ".into(), false),
                ("PRRT_kwDOA1b2c84ZyXwV".into(), false),
            ],
            2
        )
    );

    // A thread GitHub no longer has is no longer recorded.
    s.github.lock().unwrap().threads = vec![thread("PRRT_kwDOA1b2c84ZyXwV", false)];
    deliver(&s, "pull_request_review_thread", "d-resolved-2", THREAD).await;
    assert_eq!(
        recorded_threads(&pool).await,
        (vec![("PRRT_kwDOA1b2c84ZyXwV".into(), false)], 1)
    );
}

#[sqlx::test]
async fn a_thread_event_is_retried_not_recorded_while_github_is_down(pool: PgPool) {
    let s = setup(&pool).await;
    issue_with_a_ready_pull_request(&s).await;
    s.github.lock().unwrap().down = true;

    deliver(&s, "pull_request_review_thread", "d-resolved", THREAD).await;
    let job: (String, i32, Option<String>) = sqlx::query_as(
        "SELECT state, attempts, last_error FROM job WHERE idempotency_key = 'd-resolved'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        job,
        (
            "queued".into(),
            1,
            Some("GitHub /graphql answered 502 Bad Gateway".into())
        )
    );
    assert_eq!(recorded_threads(&pool).await, (vec![], 0));
}

#[sqlx::test]
async fn a_gated_request_reads_a_new_review_thread_from_github(pool: PgPool) {
    let s = setup(&pool).await;
    let agent = issue_with_a_ready_pull_request(&s).await;
    {
        let mut repo = s.github.lock().unwrap();
        repo.pulls = vec![pull_request_now(READY)];
        repo.threads = vec![thread("PRRT_kwDOA1b2c84AbCdE", false)];
    }

    // GitHub sends no event for a new thread; the request's read finds it.
    let (status, body) = request_agent_review(&s, &agent).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        recorded_threads(&pool).await,
        (vec![("PRRT_kwDOA1b2c84AbCdE".into(), false)], 1)
    );
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
    let (_, mut after) = call(&s.app, &token, "GET", "/api/workflow", None).await;
    let refused = after["refused"].take();
    assert_eq!(after, loaded);
    assert_eq!(refused["commit_sha"], "b".repeat(40));
    assert!(
        refused["reason"]
            .as_str()
            .unwrap()
            .contains("unknown gate `pr_redy`"),
        "{refused}"
    );
    assert!(refused["at"].is_string());

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
        // A `closed` payload is checked against GitHub, which agrees.
        repo.pulls = vec![pull_request_now(closed)];
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

/// PL-1's timeline rows, counted by kind.
async fn events_by_kind(pool: &PgPool) -> Vec<(String, i64)> {
    sqlx::query_as(
        "SELECT kind, count(*) FROM event WHERE issue_id = 'PL-1' GROUP BY kind ORDER BY kind",
    )
    .fetch_all(pool)
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
    // The actor is the refusal's, on the timeline.
    assert_eq!(rows_from_github(&pool).await, (0, 0, 1));
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
    assert_eq!(rows_from_github(&pool).await, (0, 0, 1));
    assert_eq!(events_by_kind(&pool).await, [("refusal".to_string(), 1)]);
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
    assert_eq!(rows_from_github(&pool).await, (0, 0, 1));
    let state: String = sqlx::query_scalar("SELECT state FROM pull_request WHERE number = 12")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(state, "merged");

    // One refusal on the timeline and no transition, after the three deliveries above and
    // after the first of them once more.
    assert_eq!(events_by_kind(&pool).await, [("refusal".to_string(), 1)]);
    deliver(&s, "pull_request", "closed-1", MERGED).await;
    assert_eq!(events_by_kind(&pool).await, [("refusal".to_string(), 1)]);
    assert_eq!(rows_from_github(&pool).await, (0, 0, 1));
    let token = s.person.token.clone();
    let (status, events) = call(&s.app, &token, "GET", "/api/issues/PL-1/events", None).await;
    assert_eq!(status, StatusCode::OK, "{events}");
    assert_eq!(events.as_array().unwrap().len(), 1);
    let detail = &events[0]["detail"];
    assert_eq!(events[0]["actor"]["name"], "octocat");
    assert_eq!(detail["kind"], "refusal");
    assert_eq!(detail["from"], "merging");
    assert_eq!(detail["to"], "done");
    assert_eq!(detail["gate"], "proof_attached");
    assert_eq!(
        detail["reason"],
        "Punchlist does not evaluate the proof_attached gate yet"
    );
    assert_eq!(detail["delivery_id"], "closed-1");

    // The PRD's way out of Merging: a person sends the issue back to Todo.
    let (status, body) = call(
        &s.app,
        &token,
        "POST",
        "/api/issues/PL-1/transitions",
        Some(json!({"to": "todo"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["issue"]["status"], "todo");
    assert_eq!(
        events_by_kind(&pool).await,
        [("refusal".to_string(), 1), ("transition".to_string(), 1)]
    );
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
async fn a_stored_version_without_the_dispatch_rule_stays_in_force_until_a_valid_push(
    pool: PgPool,
) {
    let s = setup(&pool).await;
    let token = s.person.token.clone();
    // A version stored before the runtime required a dispatch rule for `in_progress`.
    let old = PRD_WORKFLOW.replace(
        "[dispatch.status.in_progress]\nagent = \"claude-code\"\nprompt = \"prompts/in_progress.md\"\n",
        "",
    );
    assert_ne!(old, PRD_WORKFLOW);
    let prompts = BTreeMap::from([(
        "prompts/agent_review.md".to_string(),
        "Review it.".to_string(),
    )]);
    let (stored, _) = Workflow::from_stored(&old, prompts.clone()).unwrap();
    let hash = stored.version();
    sqlx::query!(
        "WITH v AS (
             INSERT INTO workflow_version (workspace_id, hash, commit_sha, workflow_toml, prompts)
             SELECT id, $1, $2, $3, $4 FROM workspace RETURNING id)
         UPDATE workspace SET workflow_version_id = (SELECT id FROM v)",
        hash,
        "c".repeat(40),
        old,
        json!(prompts),
    )
    .execute(&pool)
    .await
    .unwrap();
    let (status, body) = call(&s.app, &token, "GET", "/api/workflow", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["commit_sha"], "c".repeat(40));
    assert_eq!(body["version"], hash);
    assert_eq!(
        body["invalid"],
        "line 4, `statuses`: the runtime needs a dispatch rule for `in_progress`"
    );
    assert_eq!(warn_invalid_workflows(&pool).await.unwrap(), 1);
    let (status, _) = call(
        &s.app,
        &token,
        "POST",
        "/api/issues",
        Some(json!({"title": "X"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, body) = call(
        &s.app,
        &token,
        "POST",
        "/api/issues/PL-1/transitions",
        Some(json!({"to": "todo"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let issues = sqlx::query!("SELECT id, status FROM issue")
        .fetch_all(&pool)
        .await
        .unwrap();
    let issues: Vec<_> = issues.iter().map(|i| (&*i.id, &*i.status)).collect();
    assert_eq!(issues, [("PL-1", "todo")]);
    let moves = sqlx::query!("SELECT from_status, to_status, workflow_version FROM transition")
        .fetch_all(&pool)
        .await
        .unwrap();
    let moves: Vec<_> = moves
        .iter()
        .map(|t| (&*t.from_status, &*t.to_status, &*t.workflow_version))
        .collect();
    assert_eq!(moves, [("backlog", "todo", hash)]);
    assert_eq!(body.get("warning"), None);
    // A move to Start still succeeds, and says that no runner can claim the issue.
    let (status, body) = call(
        &s.app,
        &token,
        "POST",
        "/api/issues/PL-1/transitions",
        Some(json!({"to": "start"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["issue"]["status"], "start");
    assert_eq!(
        body["warning"],
        "line 4, `statuses`: the runtime needs a dispatch rule for `in_progress`"
    );

    // Pushing that same file is refused.
    {
        let mut repo = s.github.lock().unwrap();
        repo.sha = "d".repeat(40);
        repo.files.insert("workflow.toml".into(), old);
    }
    deliver(&s, "push", "push-1", PUSH).await;
    assert_eq!(workflow_versions(&pool).await, 1);
    let (_, after) = call(&s.app, &token, "GET", "/api/workflow", None).await;
    assert_eq!(after["commit_sha"], "c".repeat(40));

    // Pushing a valid file replaces it.
    {
        let mut repo = s.github.lock().unwrap();
        repo.sha = "e".repeat(40);
        repo.files
            .insert("workflow.toml".into(), PRD_WORKFLOW.into());
        repo.files
            .insert("prompts/in_progress.md".into(), "Build it.\n".into());
        repo.files
            .insert("prompts/agent_review.md".into(), "Review it.\n".into());
    }
    deliver(&s, "push", "push-2", PUSH).await;
    let (_, valid) = call(&s.app, &token, "GET", "/api/workflow", None).await;
    assert_eq!(valid["commit_sha"], "e".repeat(40));
    assert_ne!(valid["version"], hash);
    assert_eq!(valid["invalid"], Value::Null);
    assert_eq!(warn_invalid_workflows(&pool).await.unwrap(), 0);
    // Under the valid version a move to Start carries no warning.
    let (status, _) = call(
        &s.app,
        &token,
        "POST",
        "/api/issues",
        Some(json!({"title": "Y"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    for to in ["todo", "start"] {
        let (status, body) = call(
            &s.app,
            &token,
            "POST",
            "/api/issues/PL-2/transitions",
            Some(json!({"to": to})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body.get("warning"), None);
    }
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

#[sqlx::test]
async fn a_workflow_that_drops_a_status_in_use_is_refused(pool: PgPool) {
    let s = setup(&pool).await;
    let token = s.person.token.clone();
    let (_, issue) = call(
        &s.app,
        &token,
        "POST",
        "/api/issues",
        Some(json!({"title": "X"})),
    )
    .await;
    let path = format!("/api/issues/{}/transitions", issue["id"].as_str().unwrap());
    let (status, _) = call(&s.app, &token, "POST", &path, Some(json!({"to": "todo"}))).await;
    assert_eq!(status, StatusCode::OK);

    {
        let mut repo = s.github.lock().unwrap();
        repo.sha = "a".repeat(40);
        repo.files.insert(
            "workflow.toml".into(),
            PRD_WORKFLOW.replace("todo", "ready"),
        );
        repo.files
            .insert("prompts/system.md".into(), "System.\n".into());
        repo.files
            .insert("prompts/in_progress.md".into(), "Build it.\n".into());
        repo.files
            .insert("prompts/agent_review.md".into(), "Review it.\n".into());
    }
    deliver(&s, "push", "push-1", PUSH).await;
    assert_eq!(workflow_versions(&pool).await, 0);
    let (_, shown) = call(&s.app, &token, "GET", "/api/workflow", None).await;
    assert_eq!(shown["commit_sha"], Value::Null);
    assert_eq!(shown["refused"]["commit_sha"], "a".repeat(40));
    assert_eq!(
        shown["refused"]["reason"],
        "status in use is not in the workflow: `todo` by 1 issue(s)"
    );

    // The issue can still move; with it out of Todo the same file loads.
    let (status, _) = call(&s.app, &token, "POST", &path, Some(json!({"to": "start"}))).await;
    assert_eq!(status, StatusCode::OK);
    deliver(&s, "push", "push-2", PUSH).await;
    assert_eq!(workflow_versions(&pool).await, 1);
    let (_, shown) = call(&s.app, &token, "GET", "/api/workflow", None).await;
    assert_eq!(shown["commit_sha"], "a".repeat(40));
    assert_eq!(shown["refused"], Value::Null);
}

/// Waits until some backend waits on a lock, so a test needs no sleep as synchronization.
async fn wait_for_lock_waiter(pool: &PgPool) {
    for _ in 0..500 {
        let waiting = sqlx::query_scalar!(
            r#"SELECT count(*) AS "n!" FROM pg_stat_activity
               WHERE wait_event_type = 'Lock' AND datname = current_database()"#
        )
        .fetch_one(pool)
        .await
        .unwrap();
        if waiting > 0 {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("nothing waited on a lock");
}

fn put_workflow(s: &Setup, sha: &str, toml: String) {
    let mut repo = s.github.lock().unwrap();
    repo.sha = sha.repeat(40);
    repo.files.insert("workflow.toml".into(), toml);
    for (path, text) in [
        ("prompts/system.md", "System.\n"),
        ("prompts/in_progress.md", "Build it.\n"),
        ("prompts/agent_review.md", "Review it.\n"),
    ] {
        repo.files.insert(path.into(), text.into());
    }
}

#[sqlx::test]
async fn a_load_waits_for_a_move_in_flight_and_then_refuses(pool: PgPool) {
    let s = Arc::new(setup(&pool).await);
    let token = s.person.token.clone();
    let (_, issue) = call(
        &s.app,
        &token,
        "POST",
        "/api/issues",
        Some(json!({"title": "X"})),
    )
    .await;
    let id = issue["id"].as_str().unwrap().to_string();
    put_workflow(&s, "a", PRD_WORKFLOW.replace("todo", "ready"));

    // A move in flight: it holds the workspace lock and has put the issue in Todo.
    let mut mover = pool.begin().await.unwrap();
    sqlx::query!("SELECT id FROM workspace FOR NO KEY UPDATE")
        .fetch_one(&mut *mover)
        .await
        .unwrap();
    sqlx::query!("UPDATE issue SET status = 'todo' WHERE id = $1", id)
        .execute(&mut *mover)
        .await
        .unwrap();
    let load = tokio::spawn({
        let s = s.clone();
        async move { deliver(&s, "push", "push-1", PUSH).await }
    });
    wait_for_lock_waiter(&pool).await;
    assert!(!load.is_finished());
    mover.commit().await.unwrap();
    load.await.unwrap();
    assert_eq!(workflow_versions(&pool).await, 0);
    let (_, shown) = call(&s.app, &token, "GET", "/api/workflow", None).await;
    assert_eq!(shown["commit_sha"], Value::Null);
}

#[sqlx::test]
async fn a_move_waiting_behind_a_load_is_decided_by_the_new_workflow(pool: PgPool) {
    let s = Arc::new(setup(&pool).await);
    let token = s.person.token.clone();
    put_workflow(&s, "a", PRD_WORKFLOW.into());
    deliver(&s, "push", "push-1", PUSH).await;
    let (_, first) = call(&s.app, &token, "GET", "/api/workflow", None).await;
    let (_, issue) = call(
        &s.app,
        &token,
        "POST",
        "/api/issues",
        Some(json!({"title": "X"})),
    )
    .await;
    let path = format!("/api/issues/{}/transitions", issue["id"].as_str().unwrap());

    // A load in flight: it holds the workspace lock and has switched to a version with `ready`.
    let mut load = pool.begin().await.unwrap();
    sqlx::query!("SELECT id FROM workspace FOR NO KEY UPDATE")
        .fetch_one(&mut *load)
        .await
        .unwrap();
    let ready = sqlx::query_scalar!(
        "INSERT INTO workflow_version (workspace_id, hash, commit_sha, workflow_toml, prompts)
         SELECT workspace_id, 'sha256:ready', $1, replace(workflow_toml, 'todo', 'ready'), prompts
         FROM workflow_version RETURNING id",
        "b".repeat(40),
    )
    .fetch_one(&mut *load)
    .await
    .unwrap();
    sqlx::query!("UPDATE workspace SET workflow_version_id = $1", ready)
        .execute(&mut *load)
        .await
        .unwrap();
    let mover = tokio::spawn({
        let s = s.clone();
        async move { call(&s.app, &token, "POST", &path, Some(json!({"to": "ready"}))).await }
    });
    wait_for_lock_waiter(&pool).await;
    assert!(!mover.is_finished());
    load.commit().await.unwrap();
    let (status, moved) = mover.await.unwrap();
    // The built-in workflow would answer 409 unknown_status for `ready`.
    assert_eq!(status, StatusCode::OK);
    assert_ne!(
        moved["event"]["detail"]["workflow_version"],
        first["version"]
    );
}
