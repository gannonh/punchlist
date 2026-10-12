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
use tokio::sync::Barrier;
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
/// threads. `down` makes the pull request and review thread endpoints fail, `graphql_down` the
/// review threads alone. A pull request GitHub does not have is a 404. While `hold` is set,
/// each answer about one pull request or its review threads is kept back: it is what GitHub
/// had when it was asked, and it is sent after the test has met the barrier twice.
#[derive(Default)]
struct Repo {
    sha: String,
    files: HashMap<String, String>,
    pulls: Vec<Value>,
    threads: Vec<Value>,
    down: bool,
    graphql_down: bool,
    hold: Option<Arc<Barrier>>,
}

/// Waits out a `hold`: once to tell the test the answer is ready, once to be let go.
async fn held(hold: Option<Arc<Barrier>>) {
    if let Some(hold) = hold {
        hold.wait().await;
        hold.wait().await;
    }
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
                    let (answer, hold) = {
                        let repo = repo.lock().unwrap();
                        let answer = match repo.pulls.iter().find(|pr| pr["number"] == number) {
                            _ if repo.down => {
                                (StatusCode::INTERNAL_SERVER_ERROR, axum::Json(json!({})))
                            }
                            Some(pr) => (StatusCode::OK, axum::Json(pr.clone())),
                            None => (StatusCode::NOT_FOUND, axum::Json(json!({}))),
                        };
                        (answer, repo.hold.clone())
                    };
                    held(hold).await;
                    answer
                },
            ),
        )
        // GraphQL `reviewThreads`, two to a page; the cursor is the next thread's index.
        .route(
            "/graphql",
            post(
                |State(repo): State<FakeGithub>, axum::Json(query): axum::Json<Value>| async move {
                    let (data, hold) = {
                        let repo = repo.lock().unwrap();
                        if repo.down || repo.graphql_down {
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
                        let data =
                            json!({"repository": {"pullRequest": {"reviewThreads": threads}}});
                        (data, repo.hold.clone())
                    };
                    held(hold).await;
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
    claimed_by_runner(s).await.0
}

/// `claimed_issue`, also returning the runner's token and the claim.
async fn claimed_by_runner(s: &Setup) -> (String, String, Value) {
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
    let agent = claim["agent_token"].as_str().unwrap().to_string();
    (agent, runner_token.to_string(), claim)
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
                    "draft": true,
                    "head_sha": "6dcb09b5b57875f334f61aebed695e2e4193db5e",
                    "merge_state": "unknown",
                    "check_runs": [],
                    "review_threads": []
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
async fn a_gated_request_is_refused_when_github_cannot_be_read(pool: PgPool) {
    let s = setup(&pool).await;
    // Everything recorded would pass: a passed check, a clean merge state, no threads.
    let codex = issue_in_agent_review(&s).await;
    let passed = edited(CHECK_RUN, |_| {});
    deliver(&s, "check_run", "c-passed", &passed).await;
    sqlx::query("UPDATE pull_request SET merge_state = 'clean'")
        .execute(&pool)
        .await
        .unwrap();
    let refusal = json!({
        "code": "github_unreadable",
        "message": "GitHub could not be read, so the gates for PL-1 were not checked \
                    and it did not move; request again",
    });
    let gate_rows = || async {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM transition_gate g JOIN transition t ON t.id = g.transition_id
             WHERE t.to_status = 'human_review'",
        )
        .fetch_one(&pool)
        .await
        .unwrap()
    };

    // GitHub answers 500 to the pull request reads: the rows could be from before a push.
    s.github.lock().unwrap().down = true;
    let (status, body) = request_human_review(&s, &codex).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body, refusal);
    assert_eq!(issue_status(&pool).await, "agent_review");
    assert_eq!(gate_rows().await, 0);

    // The pull request reads, but not its threads.
    {
        let mut repo = s.github.lock().unwrap();
        repo.down = false;
        repo.graphql_down = true;
        repo.pulls = vec![ready_with_merge_state("clean")];
    }
    let (status, body) = request_human_review(&s, &codex).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body, refusal);
    assert_eq!(issue_status(&pool).await, "agent_review");
    assert_eq!(gate_rows().await, 0);

    // GitHub is back: the same request moves the issue.
    s.github.lock().unwrap().graphql_down = false;
    let (status, body) = request_human_review(&s, &codex).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(gate_rows().await, 4);
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
async fn deliveries_are_retried_not_recorded_while_github_is_down(pool: PgPool) {
    let s = setup(&pool).await;
    issue_with_a_ready_pull_request(&s).await;
    {
        let mut repo = s.github.lock().unwrap();
        repo.pulls = vec![pull_request_now(CLOSED)];
        repo.threads = vec![thread("PRRT_kwDOA1b2c84AbCdE", true)];
        repo.down = true;
    }

    deliver(&s, "pull_request", "d-closed", CLOSED).await;
    deliver(&s, "pull_request_review_thread", "d-resolved", THREAD).await;
    let jobs: Vec<(String, i32, String)> = sqlx::query_as(
        "SELECT state, attempts, split_part(last_error, ':', 1) FROM job
         WHERE idempotency_key IN ('d-closed', 'd-resolved') ORDER BY id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let closed = "GitHub /repos/gannonh/punchlist/pulls/12 answered 500 Internal Server Error";
    let resolved = "GitHub /graphql answered 502 Bad Gateway";
    assert_eq!(
        jobs,
        [
            ("queued".into(), 1, closed.into()),
            ("queued".into(), 1, resolved.into())
        ]
    );
    assert_eq!(
        pull_request_and_issue(&pool).await,
        ("open".into(), "in_progress".into())
    );
    assert_eq!(recorded_threads(&pool).await, (vec![], 0));

    // GitHub answers again, and the retries record the close, move the issue and record the
    // thread.
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
    assert_eq!(
        recorded_threads(&pool).await,
        (vec![("PRRT_kwDOA1b2c84AbCdE".into(), true)], 0)
    );
}

#[sqlx::test]
async fn a_closed_delivery_for_a_pull_request_github_no_longer_has_is_recorded(pool: PgPool) {
    let s = setup(&pool).await;
    issue_with_a_ready_pull_request(&s).await;
    // GitHub answers 404 for #12: there is nothing newer to read, so the payload stands.
    assert!(s.github.lock().unwrap().pulls.is_empty());

    deliver(&s, "pull_request", "d-closed", CLOSED).await;
    let once = [moved("in_progress", "todo", "octocat", Some("d-closed"))];
    assert_eq!(event_transitions(&pool).await, once);
    assert_eq!(
        pull_request_and_issue(&pool).await,
        ("closed".into(), "todo".into())
    );

    // It is recorded closed, so the same payload again closes nothing and reads nothing:
    // its job is done though GitHub is down.
    s.github.lock().unwrap().down = true;
    deliver(&s, "pull_request", "d-closed-again", CLOSED).await;
    let state: String =
        sqlx::query_scalar("SELECT state FROM job WHERE idempotency_key = 'd-closed-again'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(state, "done");
    assert_eq!(event_transitions(&pool).await, once);
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

#[sqlx::test]
async fn a_late_closed_delivery_does_not_name_who_closed_it_again(pool: PgPool) {
    let s = setup(&pool).await;
    issue_with_a_ready_pull_request(&s).await;
    // Closed by octocat, reopened, and closed again later by someone GitHub does not name;
    // octocat's `closed` is the first to arrive.
    let mut again = pull_request_now(CLOSED);
    again["closed_at"] = json!("2026-10-09T19:00:00Z");
    s.github.lock().unwrap().pulls = vec![again];

    deliver(&s, "pull_request", "d-closed", CLOSED).await;
    assert_eq!(
        event_transitions(&pool).await,
        [moved("in_progress", "todo", "GitHub", Some("d-closed"))]
    );
}

#[sqlx::test]
async fn a_closed_delivery_github_confirms_credits_its_sender(pool: PgPool) {
    let s = setup(&pool).await;
    issue_with_a_ready_pull_request(&s).await;
    s.github.lock().unwrap().pulls = vec![pull_request_now(CLOSED)];

    deliver(&s, "pull_request", "d-closed", CLOSED).await;
    assert_eq!(
        event_transitions(&pool).await,
        [moved("in_progress", "todo", "octocat", Some("d-closed"))]
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

/// The job of delivery `id`: its state, its attempts and its last error.
async fn job(pool: &PgPool, id: &str) -> (String, i32, Option<String>) {
    sqlx::query_as("SELECT state, attempts, last_error FROM job WHERE idempotency_key = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn pull_request_version(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT version FROM pull_request WHERE number = 12")
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Holds GitHub's answers from now on. `caught` waits for the next one to be held and stops
/// holding; `let_go` sends it.
fn hold(s: &Setup) -> Arc<Barrier> {
    let hold = Arc::new(Barrier::new(2));
    s.github.lock().unwrap().hold = Some(hold.clone());
    hold
}

async fn caught(s: &Setup, hold: &Barrier) {
    hold.wait().await;
    s.github.lock().unwrap().hold = None;
}

async fn let_go(hold: &Barrier) {
    hold.wait().await;
}

#[sqlx::test]
async fn a_jobs_older_read_of_the_threads_does_not_replace_a_gated_requests(pool: PgPool) {
    let s = setup(&pool).await;
    let agent = issue_with_a_ready_pull_request(&s).await;
    {
        let mut repo = s.github.lock().unwrap();
        repo.pulls = vec![pull_request_now(READY)];
        repo.threads = vec![
            thread("PRRT_kwDOA1b2c84AbCdE", false),
            thread("PRRT_kwDOA1b2c84ZyXwV", false),
        ];
    }
    let hold = hold(&s);
    let newer = (
        vec![
            ("PRRT_kwDOA1b2c84AbCdE".into(), true),
            ("PRRT_kwDOA1b2c84ZyXwV".into(), false),
        ],
        1,
    );

    tokio::join!(
        deliver(&s, "pull_request_review_thread", "d-resolved", THREAD),
        async {
            // The job has read two open threads, and its answer is on the way. The first is
            // resolved, and a gated request reads and records that.
            caught(&s, &hold).await;
            s.github.lock().unwrap().threads[0] = thread("PRRT_kwDOA1b2c84AbCdE", true);
            let (status, body) = request_agent_review(&s, &agent).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(recorded_threads(&pool).await, newer);
            assert_eq!(pull_request_version(&pool).await, 1);
            let_go(&hold).await;
        }
    );

    // The job's answer came second and is the older one: it is dropped. The job read again
    // in the same attempt, and recorded that.
    assert_eq!(recorded_threads(&pool).await, newer);
    assert_eq!(job(&pool, "d-resolved").await, ("done".into(), 1, None));
    assert_eq!(pull_request_version(&pool).await, 2);
}

#[sqlx::test]
async fn a_jobs_older_read_of_the_state_does_not_replace_a_gated_requests(pool: PgPool) {
    let s = setup(&pool).await;
    let agent = issue_with_a_ready_pull_request(&s).await;
    // Closed and reopened within 18:30:00, so `updated_at` does not order the two states.
    let reopened = edited(READY, |v| {
        v["pull_request"]["updated_at"] = json!("2026-10-09T18:30:00Z");
    });
    s.github.lock().unwrap().pulls = vec![pull_request_now(CLOSED)];
    let hold = hold(&s);

    tokio::join!(deliver(&s, "pull_request", "d-closed", CLOSED), async {
        // The job has read it closed, and its answer is on the way. It is reopened, and a
        // gated request reads and records that.
        caught(&s, &hold).await;
        s.github.lock().unwrap().pulls = vec![pull_request_now(&reopened)];
        let (status, body) = request_agent_review(&s, &agent).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let_go(&hold).await;
    });

    // The job's closed answer came second and is the older one: nothing is closed or moved.
    // The job read again in the same attempt: GitHub says open.
    assert_eq!(
        pull_request_and_issue(&pool).await,
        ("open".into(), "agent_review".into())
    );
    assert_eq!(event_transitions(&pool).await, []);
    assert_eq!(job(&pool, "d-closed").await, ("done".into(), 1, None));
    assert_eq!(pull_request_version(&pool).await, 2);
}

#[sqlx::test]
async fn a_job_whose_read_is_dropped_three_times_fails_and_is_retried(pool: PgPool) {
    let s = setup(&pool).await;
    let agent = issue_with_a_ready_pull_request(&s).await;
    {
        let mut repo = s.github.lock().unwrap();
        repo.pulls = vec![pull_request_now(READY)];
        repo.threads = vec![thread("PRRT_kwDOA1b2c84AbCdE", false)];
    }
    let hold = hold(&s);

    tokio::join!(
        deliver(&s, "pull_request_review_thread", "d-resolved", THREAD),
        async {
            // Each of the job's three reads is held while a gated request reads and records:
            // the move to Agent Review, then two requests refused at a gate after their read.
            for to in ["agent_review", "human_review", "human_review"] {
                caught(&s, &hold).await;
                let uri = "/api/issues/PL-1/transitions";
                call(&s.app, &agent, "POST", uri, Some(json!({"to": to}))).await;
                s.github.lock().unwrap().hold = Some(hold.clone());
                let_go(&hold).await;
            }
            s.github.lock().unwrap().hold = None;
        }
    );

    assert_eq!(
        job(&pool, "d-resolved").await,
        (
            "queued".into(),
            1,
            Some("another read of the pull request was recorded first, 3 times".into())
        )
    );
    assert_eq!(pull_request_version(&pool).await, 3);
}

#[sqlx::test]
async fn a_gated_request_whose_write_is_dropped_reads_again(pool: PgPool) {
    let s = setup(&pool).await;
    let agent = issue_with_a_ready_pull_request(&s).await;
    {
        let mut repo = s.github.lock().unwrap();
        repo.pulls = vec![pull_request_now(READY)];
        repo.threads = vec![thread("PRRT_kwDOA1b2c84AbCdE", false)];
    }
    let hold = hold(&s);
    let resolved = (vec![("PRRT_kwDOA1b2c84AbCdE".into(), true)], 0);

    let ((status, body), ()) = tokio::join!(request_agent_review(&s, &agent), async {
        // The request has read the pull request, and that answer is on the way. The thread
        // is resolved, and its delivery's job reads and records that first.
        caught(&s, &hold).await;
        s.github.lock().unwrap().threads = vec![thread("PRRT_kwDOA1b2c84AbCdE", true)];
        deliver(&s, "pull_request_review_thread", "d-resolved", THREAD).await;
        assert_eq!(recorded_threads(&pool).await, resolved);
        assert_eq!(pull_request_version(&pool).await, 1);
        let_go(&hold).await;
    });

    // The request's first write was dropped; it read again, and its second was recorded.
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(recorded_threads(&pool).await, resolved);
    assert_eq!(pull_request_version(&pool).await, 2);
}

#[sqlx::test]
async fn a_gated_request_dropped_twice_is_judged_on_what_is_recorded(pool: PgPool) {
    let s = setup(&pool).await;
    let agent = issue_with_a_ready_pull_request(&s).await;
    {
        let mut repo = s.github.lock().unwrap();
        repo.pulls = vec![pull_request_now(READY)];
        repo.threads = vec![thread("PRRT_kwDOA1b2c84AbCdE", false)];
    }
    let hold = hold(&s);
    let resolved = (vec![("PRRT_kwDOA1b2c84AbCdE".into(), true)], 0);

    let ((status, body), ()) = tokio::join!(request_agent_review(&s, &agent), async {
        // The request's first read of the pull request is held while a job reads and records.
        caught(&s, &hold).await;
        deliver(&s, "pull_request_review_thread", "d-1", THREAD).await;
        s.github.lock().unwrap().hold = Some(hold.clone());
        let_go(&hold).await;
        // Its first read of the threads; that write is dropped.
        hold.wait().await;
        let_go(&hold).await;
        // Its second read of the pull request is held while another job reads and records
        // the thread resolved. The thread is opened again before the request reads it.
        caught(&s, &hold).await;
        s.github.lock().unwrap().threads = vec![thread("PRRT_kwDOA1b2c84AbCdE", true)];
        deliver(&s, "pull_request_review_thread", "d-2", THREAD).await;
        s.github.lock().unwrap().threads = vec![thread("PRRT_kwDOA1b2c84AbCdE", false)];
        let_go(&hold).await;
    });

    // The request's second write was dropped too. It is answered, not failed, and the rows
    // are the second job's.
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(recorded_threads(&pool).await, resolved);
    assert_eq!(pull_request_version(&pool).await, 2);
}

#[sqlx::test]
async fn a_payload_that_inserts_the_row_during_a_read_does_not_drop_the_read(pool: PgPool) {
    let s = setup(&pool).await;
    let agent = claimed_issue(&s).await;
    {
        let mut repo = s.github.lock().unwrap();
        repo.pulls = vec![pull_request_now(READY)];
        repo.threads = vec![thread("PRRT_kwDOA1b2c84AbCdE", false)];
    }
    let hold = hold(&s);

    let ((status, body), ()) = tokio::join!(request_agent_review(&s, &agent), async {
        // The request found no row and has read the pull request. Its `opened` delivery, a
        // payload recorded without a read, inserts the row first.
        caught(&s, &hold).await;
        deliver(&s, "pull_request", "d-opened", OPENED).await;
        assert_eq!(pull_request_version(&pool).await, 0);
        let_go(&hold).await;
    });

    // The request's one read was recorded: ready, with its thread.
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        recorded_threads(&pool).await,
        (vec![("PRRT_kwDOA1b2c84AbCdE".into(), false)], 1)
    );
    assert_eq!(pull_request_version(&pool).await, 1);
}

#[sqlx::test]
async fn a_gated_request_is_refused_when_an_open_pull_requests_threads_cannot_be_read(
    pool: PgPool,
) {
    let s = setup(&pool).await;
    let agent = claimed_issue(&s).await;
    deliver(&s, "pull_request", "d-opened", OPENED).await;
    {
        let mut repo = s.github.lock().unwrap();
        repo.pulls = vec![pull_request_now(READY)];
        repo.threads = vec![thread("PRRT_kwDOA1b2c84AbCdE", false)];
        repo.graphql_down = true;
    }

    // This transition's gates read no thread, but which gates it has is decided later,
    // under the issue's lock: the request is refused whichever they are.
    let (status, body) = request_agent_review(&s, &agent).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(body["code"], "github_unreadable");
    assert_eq!(issue_status(&pool).await, "in_progress");
    assert_eq!(recorded_threads(&pool).await, (vec![], 0));

    s.github.lock().unwrap().graphql_down = false;
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
        close_with_gate(&s, "pr_closed_unmerged", "mergeable", "in_progress", CLOSED).await,
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
        close_with_gate(&s, "pr_merged", "ci_green", "merging", MERGED).await,
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
    assert_eq!(detail["gate"], "ci_green");
    assert_eq!(detail["reason"], MERGED_REASON);
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

const CHECK_RUN: &str = include_str!("fixtures/github/check_run_completed.json");

/// Every dispatch of PL-1 that waits for a runner: the status its transition moved to, its
/// agent, and whether that transition is the issue's latest.
async fn waiting_dispatches(pool: &PgPool) -> Vec<(String, String, bool)> {
    sqlx::query_as(
        "SELECT t.to_status, d.agent, t.id = (SELECT id FROM transition
             WHERE issue_id = 'PL-1' ORDER BY created_at DESC LIMIT 1)
         FROM dispatch d JOIN transition t ON t.id = d.transition_id
         WHERE d.issue_id = 'PL-1' AND d.run_id IS NULL",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn register(s: &Setup, name: &str, agents: &[&str]) -> String {
    let (status, runner) = call(
        &s.app,
        &s.person.token,
        "POST",
        "/api/runners",
        Some(json!({"name": name, "agents": agents})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{runner}");
    runner["token"].as_str().unwrap().to_string()
}

async fn claim(s: &Setup, runner: &str) -> (StatusCode, Value) {
    call(&s.app, runner, "POST", "/api/runs/claim", Some(json!({}))).await
}

async fn finish(s: &Setup, runner: &str, claim: &Value) {
    let run = claim["run_id"].as_str().unwrap();
    let (status, body) = call(
        &s.app,
        runner,
        "POST",
        &format!("/api/runs/{run}/finish"),
        Some(json!({"attempt": 1, "outcome": "succeeded", "duration_ms": 1})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[sqlx::test]
async fn each_entry_into_agent_review_enqueues_one_dispatch_and_one_runner_claims_it(pool: PgPool) {
    let s = setup(&pool).await;
    // A person may move the issue in and out of Agent Review, so the test can do it quickly.
    let gated = "to = \"agent_review\"\nby = [\"agent\"]\ngates = [\"pr_open\", \"pr_ready\", \"pr_names_issue\"]";
    let workflow = PRD_WORKFLOW
        .replace(gated, "to = \"agent_review\"\nby = [\"agent\", \"person\"]")
        + "\n[[transition]]\nfrom = \"agent_review\"\nto = \"in_progress\"\nby = [\"person\"]\n";
    assert!(!workflow.contains(gated));
    put_workflow(&s, "b", workflow);
    let system = "Call request_transition.\n".to_string();
    (s.github.lock().unwrap().files).insert("prompts/system.md".into(), system);
    deliver(&s, "push", "push-1", PUSH).await;
    assert_eq!(workflow_versions(&pool).await, 1);

    let (_, claude_runner, first) = claimed_by_runner(&s).await;
    // The claim's own dispatch was claimed in its transaction.
    assert_eq!(waiting_dispatches(&pool).await, []);
    let codex = ("agent_review".to_string(), "codex".to_string(), true);
    let claude = ("in_progress".to_string(), "claude-code".to_string(), true);
    for (to, waiting) in [
        ("agent_review", &codex),
        ("in_progress", &claude),
        ("agent_review", &codex),
        ("in_progress", &claude),
        ("agent_review", &codex),
    ] {
        let (status, body) = call(
            &s.app,
            &s.person.token,
            "POST",
            "/api/issues/PL-1/transitions",
            Some(json!({"to": to})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        // One for the transition just made, and none left from the ones before it.
        assert_eq!(
            waiting_dispatches(&pool).await,
            std::slice::from_ref(waiting)
        );
    }

    // The run that was working on the issue is still running, so the dispatch waits; and a
    // runner that cannot run Codex is not offered it.
    let codex_runner = register(&s, "r2", &["codex"]).await;
    let other_codex_runner = register(&s, "r3", &["claude-code", "codex"]).await;
    assert_eq!(claim(&s, &codex_runner).await.0, StatusCode::NO_CONTENT);
    finish(&s, &claude_runner, &first).await;
    assert_eq!(claim(&s, &claude_runner).await.0, StatusCode::NO_CONTENT);

    // Two runners that can run Codex ask at once: one run.
    let (one, two) = tokio::join!(claim(&s, &codex_runner), claim(&s, &other_codex_runner));
    let mut statuses = [one.0, two.0];
    statuses.sort();
    assert_eq!(statuses, [StatusCode::OK, StatusCode::NO_CONTENT]);
    let second = if one.0 == StatusCode::OK {
        one.1
    } else {
        two.1
    };
    assert_eq!(second["agent"], "codex");
    assert_eq!(second["issue"]["status"], "agent_review");
    assert_eq!(second["branch"], first["branch"]);
    assert_eq!(second["branch"], "feature/pl-1-fix-the-thing");
    assert_eq!(second["prompt"], "Call request_transition.\n\nReview it.");
    assert_eq!(waiting_dispatches(&pool).await, []);
    assert_eq!(claim(&s, &codex_runner).await.0, StatusCode::NO_CONTENT);
    assert_eq!(
        claim(&s, &other_codex_runner).await.0,
        StatusCode::NO_CONTENT
    );

    let runs: Vec<(String, String, String)> =
        sqlx::query_as("SELECT status, agent, outcome FROM run ORDER BY started_at")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        runs,
        [
            (
                "in_progress".into(),
                "claude-code".into(),
                "succeeded".into()
            ),
            ("agent_review".into(), "codex".into(), "running".into()),
        ]
    );
    // The claim made no transition: the last one is still the person's move in.
    let moves: i64 = sqlx::query_scalar("SELECT count(*) FROM transition WHERE issue_id = 'PL-1'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(moves, 2 + 1 + 5);
    let claimed: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM dispatch d JOIN run r ON r.id = d.run_id AND r.agent = d.agent",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(claimed, 2);
}

#[sqlx::test]
async fn a_long_polling_codex_runner_claims_once_the_run_before_it_ends(pool: PgPool) {
    let s = setup(&pool).await;
    let (agent, runner, first) = claimed_by_runner(&s).await;
    deliver(&s, "pull_request", "d-opened", OPENED).await;
    deliver(&s, "pull_request", "d-ready", READY).await;
    let codex_runner = register(&s, "r2", &["codex"]).await;
    let waiting = {
        let (app, token) = (s.app.clone(), codex_runner.clone());
        let wait = Some(json!({"wait_seconds": 20}));
        tokio::spawn(async move { call(&app, &token, "POST", "/api/runs/claim", wait).await })
    };
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // The issue enters Agent Review while the run that asked for the move still runs.
    let (status, body) = request_agent_review(&s, &agent).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(!waiting.is_finished());

    finish(&s, &runner, &first).await;
    let (status, claim) = tokio::time::timeout(std::time::Duration::from_secs(5), waiting)
        .await
        .expect("the waiting runner is woken when the run ends")
        .unwrap();
    assert_eq!(status, StatusCode::OK, "{claim}");
    assert_eq!(claim["agent"], "codex");
    assert_eq!(claim["issue"]["status"], "agent_review");
}

/// PL-1 in Agent Review with pull request #12 open and ready, its Claude Code run ended, and
/// its dispatch for Codex waiting.
async fn waiting_in_agent_review(s: &Setup) {
    let (agent, runner, first) = claimed_by_runner(s).await;
    deliver(s, "pull_request", "d-opened", OPENED).await;
    deliver(s, "pull_request", "d-ready", READY).await;
    let (status, body) = request_agent_review(s, &agent).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    finish(s, &runner, &first).await;
}

#[sqlx::test]
async fn a_dispatch_keeps_the_agent_and_prompt_of_the_workflow_it_was_queued_under(pool: PgPool) {
    let s = setup(&pool).await;
    // Queued under the built-in workflow: Codex, with its review prompt.
    waiting_in_agent_review(&s).await;
    let queued_under = punchlist_core::default_workflow().prompt_for("agent_review");

    // A version loads that gives Agent Review to Claude Code, with other prompts.
    let reassigned = PRD_WORKFLOW.replace("agent = \"codex\"", "agent = \"claude-code\"");
    assert_ne!(reassigned, PRD_WORKFLOW);
    put_workflow(&s, "c", reassigned);
    deliver(&s, "push", "push-1", PUSH).await;
    assert_eq!(workflow_versions(&pool).await, 1);

    // The waiting dispatch is still Codex's, and its run gets the prompt that came with it.
    let claude_runner = register(&s, "r2", &["claude-code"]).await;
    assert_eq!(claim(&s, &claude_runner).await.0, StatusCode::NO_CONTENT);
    let codex_runner = register(&s, "r3", &["codex"]).await;
    let (status, claim) = claim(&s, &codex_runner).await;
    assert_eq!(status, StatusCode::OK, "{claim}");
    assert_eq!(claim["agent"], "codex");
    assert_eq!(claim["prompt"], queued_under);
    assert!(queued_under.contains("ready for a person to review"));
}

/// The run number of every dispatch of PL-1 into `status`, in order, and whether it waits.
async fn dispatches_into(pool: &PgPool, status: &str) -> Vec<(i32, bool)> {
    sqlx::query_as(
        "SELECT d.run_number, d.run_id IS NULL
         FROM dispatch d JOIN transition t ON t.id = d.transition_id
         WHERE d.issue_id = 'PL-1' AND t.to_status = $1 ORDER BY t.created_at, d.run_number",
    )
    .bind(status)
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn waiting_on_person(s: &Setup) -> Value {
    let (status, issue) = call(&s.app, &s.person.token, "GET", "/api/issues/PL-1", None).await;
    assert_eq!(status, StatusCode::OK, "{issue}");
    issue["waiting_on_person"].clone()
}

#[sqlx::test]
async fn a_run_that_ends_without_moving_its_issue_is_dispatched_again_up_to_the_limit(
    pool: PgPool,
) {
    let s = setup(&pool).await;
    waiting_in_agent_review(&s).await;
    // The Claude Code run moved its issue before it ended: nothing waits for In Progress.
    let codex = ("agent_review".to_string(), "codex".to_string(), true);
    assert_eq!(
        waiting_dispatches(&pool).await,
        std::slice::from_ref(&codex)
    );
    assert_eq!(dispatches_into(&pool, "in_progress").await, [(1, false)]);
    let queued_under = punchlist_core::default_workflow().prompt_for("agent_review");

    // A version loads that gives Agent Review to Claude Code: the runs of this entry stay
    // Codex's, with the prompt of the version the issue entered under.
    put_workflow(
        &s,
        "c",
        PRD_WORKFLOW.replace("agent = \"codex\"", "agent = \"claude-code\""),
    );
    deliver(&s, "push", "push-1", PUSH).await;
    assert_eq!(workflow_versions(&pool).await, 1);

    let runner = register(&s, "r2", &["claude-code", "codex"]).await;
    // Each run ends without a request, the second one failed. After the first and the second
    // exactly one more dispatch waits; after the third, none.
    let finishes = [
        json!({"attempt": 1, "outcome": "succeeded", "duration_ms": 1}),
        json!({"attempt": 1, "outcome": "failed", "reason": "gave up", "duration_ms": 1}),
        json!({"attempt": 1, "outcome": "succeeded", "duration_ms": 1}),
    ];
    let after: [&[(i32, bool)]; 3] = [
        &[(1, false), (2, true)],
        &[(1, false), (2, false), (3, true)],
        &[(1, false), (2, false), (3, false)],
    ];
    for (outcome, after) in finishes.into_iter().zip(after) {
        assert_eq!(waiting_on_person(&s).await, Value::Null);
        let (status, claimed) = claim(&s, &runner).await;
        assert_eq!(status, StatusCode::OK, "{claimed}");
        assert_eq!(claimed["agent"], "codex");
        assert_eq!(claimed["issue"]["status"], "agent_review");
        assert_eq!(claimed["prompt"], queued_under);
        assert_eq!(waiting_dispatches(&pool).await, []);
        // One run at a time: nothing is claimable while this one runs.
        assert_eq!(claim(&s, &runner).await.0, StatusCode::NO_CONTENT);

        let uri = format!("/api/runs/{}/finish", claimed["run_id"].as_str().unwrap());
        let (status, body) = call(&s.app, &runner, "POST", &uri, Some(outcome.clone())).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        // A second finish of the same attempt is refused and queues nothing.
        let (status, body) = call(&s.app, &runner, "POST", &uri, Some(outcome)).await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert_eq!(dispatches_into(&pool, "agent_review").await, after);
    }

    // Past the limit nothing is claimable, and the issue says why.
    assert_eq!(waiting_dispatches(&pool).await, []);
    assert_eq!(claim(&s, &runner).await.0, StatusCode::NO_CONTENT);
    assert_eq!(
        dispatches_into(&pool, "agent_review").await,
        [(1, false), (2, false), (3, false)]
    );
    assert_eq!(
        waiting_on_person(&s).await,
        "3 runs in Agent Review ended without moving it"
    );
    assert_eq!(issue_status(&pool).await, "agent_review");
    let runs: i64 = sqlx::query_scalar("SELECT count(*) FROM run WHERE status = 'agent_review'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(runs, 3);
}

/// The PRD's workflow, with Agent Review and In Progress open to a person in both
/// directions, loaded.
async fn load_a_workflow_a_person_can_move_through(s: &Setup, pool: &PgPool) {
    let gated = "to = \"agent_review\"\nby = [\"agent\"]\ngates = [\"pr_open\", \"pr_ready\", \"pr_names_issue\"]";
    let workflow = PRD_WORKFLOW
        .replace(gated, "to = \"agent_review\"\nby = [\"agent\", \"person\"]")
        + "\n[[transition]]\nfrom = \"agent_review\"\nto = \"in_progress\"\nby = [\"person\"]\n";
    assert!(!workflow.contains(gated));
    put_workflow(s, "b", workflow);
    let system = "Call request_transition.\n".to_string();
    (s.github.lock().unwrap().files).insert("prompts/system.md".into(), system);
    deliver(s, "push", "push-1", PUSH).await;
    assert_eq!(workflow_versions(pool).await, 1);
}

async fn person_moves(s: &Setup, to: &str) {
    let uri = "/api/issues/PL-1/transitions";
    let (status, body) = call(
        &s.app,
        &s.person.token,
        "POST",
        uri,
        Some(json!({"to": to})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[sqlx::test]
async fn a_new_transition_into_the_status_starts_the_count_again(pool: PgPool) {
    let s = setup(&pool).await;
    load_a_workflow_a_person_can_move_through(&s, &pool).await;
    let (_, claude_runner, first) = claimed_by_runner(&s).await;
    person_moves(&s, "agent_review").await;
    finish(&s, &claude_runner, &first).await;

    let codex_runner = register(&s, "r2", &["codex"]).await;
    for _ in 0..3 {
        let (status, claimed) = claim(&s, &codex_runner).await;
        assert_eq!(status, StatusCode::OK, "{claimed}");
        finish(&s, &codex_runner, &claimed).await;
    }
    assert_eq!(claim(&s, &codex_runner).await.0, StatusCode::NO_CONTENT);
    assert_eq!(
        waiting_on_person(&s).await,
        "3 runs in Agent Review ended without moving it"
    );

    // Out and in again: a new entry, with its own three runs.
    person_moves(&s, "in_progress").await;
    assert_eq!(waiting_on_person(&s).await, Value::Null);
    person_moves(&s, "agent_review").await;
    assert_eq!(waiting_on_person(&s).await, Value::Null);
    assert_eq!(
        dispatches_into(&pool, "agent_review").await,
        [(1, false), (2, false), (3, false), (1, true)]
    );
    let (status, claimed) = claim(&s, &codex_runner).await;
    assert_eq!(status, StatusCode::OK, "{claimed}");
    assert_eq!(claimed["agent"], "codex");
    finish(&s, &codex_runner, &claimed).await;
    assert_eq!(
        dispatches_into(&pool, "agent_review").await,
        [(1, false), (2, false), (3, false), (1, false), (2, true)]
    );
    assert_eq!(waiting_on_person(&s).await, Value::Null);
}

#[sqlx::test]
async fn a_run_that_ends_in_a_status_the_active_workflow_no_longer_dispatches_is_the_last(
    pool: PgPool,
) {
    let s = setup(&pool).await;
    waiting_in_agent_review(&s).await;
    let runner = register(&s, "r2", &["codex"]).await;

    // Locked to agents and runners by a version loaded while the run works.
    let (status, locked_run) = claim(&s, &runner).await;
    assert_eq!(status, StatusCode::OK, "{locked_run}");
    let locks = "[lock]\n";
    let locked = PRD_WORKFLOW.replace(locks, "[lock]\nagent_review = [\"agent\", \"runner\"]\n");
    assert_ne!(locked, PRD_WORKFLOW);
    put_workflow(&s, "c", locked);
    deliver(&s, "push", "push-1", PUSH).await;
    assert_eq!(workflow_versions(&pool).await, 1);
    finish(&s, &runner, &locked_run).await;
    assert_eq!(dispatches_into(&pool, "agent_review").await, [(1, false)]);
    assert_eq!(claim(&s, &runner).await.0, StatusCode::NO_CONTENT);
    assert_eq!(waiting_on_person(&s).await, Value::Null);
}

#[sqlx::test]
async fn a_run_claimed_from_start_that_ends_without_a_request_is_dispatched_again(pool: PgPool) {
    let s = setup(&pool).await;
    let (_, runner, first) = claimed_by_runner(&s).await;
    // The claim from Start made one run, on the dispatch its own transition queued.
    assert_eq!(dispatches_into(&pool, "in_progress").await, [(1, false)]);
    finish(&s, &runner, &first).await;
    assert_eq!(
        waiting_dispatches(&pool).await,
        [("in_progress".to_string(), "claude-code".to_string(), true)]
    );

    let (status, second) = claim(&s, &runner).await;
    assert_eq!(status, StatusCode::OK, "{second}");
    assert_ne!(second["run_id"], first["run_id"]);
    assert_eq!(second["agent"], "claude-code");
    assert_eq!(second["issue"]["status"], "in_progress");
    assert_eq!(second["branch"], first["branch"]);
    assert_eq!(second["prompt"], first["prompt"]);
    assert_eq!(
        dispatches_into(&pool, "in_progress").await,
        [(1, false), (2, false)]
    );
    // The second claim made no transition.
    let moves: i64 = sqlx::query_scalar("SELECT count(*) FROM transition WHERE issue_id = 'PL-1'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(moves, 3, "todo, start, in_progress");
}

#[sqlx::test]
async fn a_run_that_ends_after_its_issue_left_and_came_back_is_not_dispatched_again(pool: PgPool) {
    let s = setup(&pool).await;
    load_a_workflow_a_person_can_move_through(&s, &pool).await;

    let (_, runner, first) = claimed_by_runner(&s).await;
    person_moves(&s, "agent_review").await;
    person_moves(&s, "in_progress").await;
    // In Progress again, by a later transition, whose own dispatch waits behind the run.
    assert_eq!(
        dispatches_into(&pool, "in_progress").await,
        [(1, false), (1, true)]
    );
    finish(&s, &runner, &first).await;
    assert_eq!(
        dispatches_into(&pool, "in_progress").await,
        [(1, false), (1, true)]
    );
    assert_eq!(dispatches_into(&pool, "agent_review").await, []);
}

#[sqlx::test]
async fn a_waiting_dispatch_on_an_older_issue_is_claimed_before_a_started_one(pool: PgPool) {
    let s = setup(&pool).await;
    waiting_in_agent_review(&s).await;
    let token = &s.person.token;
    let (status, _) = call(
        &s.app,
        token,
        "POST",
        "/api/issues",
        Some(json!({"title": "Next"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    for to in ["todo", "start"] {
        let moved = Some(json!({"to": to}));
        let (status, _) = call(&s.app, token, "POST", "/api/issues/PL-2/transitions", moved).await;
        assert_eq!(status, StatusCode::OK);
    }

    // A runner that can run both takes them in issue order, whichever kind each is.
    let runner = register(&s, "r2", &["claude-code", "codex"]).await;
    let (_, first) = claim(&s, &runner).await;
    assert_eq!(
        (&first["issue"]["id"], &first["agent"]),
        (&json!("PL-1"), &json!("codex"))
    );
    let (_, second) = claim(&s, &runner).await;
    assert_eq!(
        (&second["issue"]["id"], &second["agent"]),
        (&json!("PL-2"), &json!("claude-code"))
    );
    assert_eq!(second["issue"]["status"], "in_progress");
    assert_eq!(claim(&s, &runner).await.0, StatusCode::NO_CONTENT);
}

/// PL-1 in Agent Review with pull request #12 open and ready, and a Codex run claimed for
/// it. Returns that run's agent token.
async fn issue_in_agent_review(s: &Setup) -> String {
    waiting_in_agent_review(s).await;
    let codex_runner = register(s, "r2", &["codex"]).await;
    let (status, claim) = claim(s, &codex_runner).await;
    assert_eq!(status, StatusCode::OK, "{claim}");
    assert_eq!(claim["agent"], "codex");
    claim["agent_token"].as_str().unwrap().to_string()
}

async fn request_human_review(s: &Setup, token: &str) -> (StatusCode, Value) {
    call(
        &s.app,
        token,
        "POST",
        "/api/issues/PL-1/transitions",
        Some(json!({"to": "human_review"})),
    )
    .await
}

/// What GitHub says of pull request #12 now: ready, with this merge state.
fn ready_with_merge_state(state: &str) -> Value {
    let mut pr = pull_request_now(READY);
    pr["mergeable_state"] = json!(state);
    pr
}

/// A check run delivery for #12's head commit.
fn check_run(id: i64, status: &str, conclusion: Option<&str>) -> String {
    edited(CHECK_RUN, |v| {
        v["action"] = json!(if status == "completed" {
            "completed"
        } else {
            "created"
        });
        v["check_run"]["id"] = json!(id);
        v["check_run"]["status"] = json!(status);
        v["check_run"]["conclusion"] = json!(conclusion);
    })
}

#[sqlx::test]
async fn human_review_is_refused_on_a_check_the_merge_state_and_a_thread_then_accepted(
    pool: PgPool,
) {
    let s = setup(&pool).await;
    let codex = issue_in_agent_review(&s).await;
    s.github.lock().unwrap().pulls = vec![ready_with_merge_state("clean")];
    let refused = |body: &Value| {
        assert_eq!(body["code"], "gate_failed", "{body}");
        assert_eq!(body["gates"].as_array().unwrap().len(), 4);
        (
            body["gate"].as_str().unwrap().to_string(),
            body["reason"].as_str().unwrap().to_string(),
        )
    };

    // No check run has reported on the head commit.
    let (status, body) = request_human_review(&s, &codex).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(
        refused(&body),
        (
            "ci_green".into(),
            "no check run has reported on commit 6dcb09b".into()
        )
    );

    // A failed check: the refusal names it and the head commit.
    deliver(
        &s,
        "check_run",
        "c-failed",
        &check_run(1, "completed", Some("failure")),
    )
    .await;
    let (status, body) = request_human_review(&s, &codex).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(
        refused(&body),
        (
            "ci_green".into(),
            "1 of 1 checks failed on 6dcb09b: `check` (failure)".into()
        )
    );
    assert_eq!(
        body["message"],
        "agent_review → human_review needs gate ci_green: \
         1 of 1 checks failed on 6dcb09b: `check` (failure)"
    );
    assert_eq!(
        body["evidence"]["pull_requests"][0]["check_runs"],
        json!([{"name": "check", "status": "completed", "conclusion": "failure"}])
    );

    // Its rerun is the newest run of that name: pending, then passed.
    deliver(
        &s,
        "check_run",
        "c-rerun",
        &check_run(2, "in_progress", None),
    )
    .await;
    let (_, body) = request_human_review(&s, &codex).await;
    assert_eq!(
        refused(&body),
        (
            "ci_green".into(),
            "1 of 1 checks pending on 6dcb09b: `check` (in_progress)".into()
        )
    );
    deliver(
        &s,
        "check_run",
        "c-passed",
        &check_run(2, "completed", Some("success")),
    )
    .await;

    // GitHub has not computed the merge state: the gate says so rather than guess.
    s.github.lock().unwrap().pulls = vec![ready_with_merge_state("unknown")];
    let (_, body) = request_human_review(&s, &codex).await;
    assert_eq!(
        refused(&body),
        (
            "mergeable".into(),
            "GitHub has not computed pull request #12's merge state yet; request again".into()
        )
    );

    // An unresolved thread, which GitHub sent no event for. Its file changed under it, so
    // GitHub reports only the line it was on.
    {
        let mut repo = s.github.lock().unwrap();
        repo.pulls = vec![ready_with_merge_state("clean")];
        repo.threads = vec![json!({
            "id": "PRRT_kwDOA1b2c84AbCdE", "isResolved": false,
            "path": "src/lib.rs", "line": null, "originalLine": 7,
        })];
    }
    let (status, body) = request_human_review(&s, &codex).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(
        refused(&body),
        (
            "no_open_threads".into(),
            "1 unresolved review thread on pull request #12: src/lib.rs:7".into()
        )
    );
    let open: i64 = sqlx::query_scalar("SELECT count(*) FROM review_thread WHERE resolved = false")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(open, 1);
    assert_eq!(issue_status(&pool).await, "agent_review");

    // The checklist `pl issue show` prints reads the same recorded evidence.
    let (status, checklist) = call(
        &s.app,
        &s.person.token,
        "GET",
        "/api/issues/PL-1/gates",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{checklist}");
    assert_eq!(checklist["from"], "agent_review");
    assert_eq!(checklist["to_name"], "Human Review");
    let passed: Vec<(&str, bool)> = (checklist["gates"].as_array().unwrap().iter())
        .map(|g| (g["gate"].as_str().unwrap(), g["passed"].as_bool().unwrap()))
        .collect();
    assert_eq!(
        passed,
        [
            ("ci_green", true),
            ("mergeable", true),
            ("no_open_threads", false),
            ("proof_attached", true)
        ]
    );

    // Resolved: the request is accepted, and the transition records the four results.
    s.github.lock().unwrap().threads[0]["isResolved"] = json!(true);
    let (status, body) = request_human_review(&s, &codex).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["issue"]["status"], "human_review");
    assert_eq!(body["event"]["actor"]["name"], "Codex");
    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT g.gate, g.result, g.reason FROM transition_gate g
         JOIN transition t ON t.id = g.transition_id
         WHERE t.to_status = 'human_review' ORDER BY g.position",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let row = |gate: &str, reason: &str| (gate.to_string(), "pass".to_string(), reason.to_string());
    assert_eq!(
        rows,
        [
            row("ci_green", "1 of 1 checks passed on 6dcb09b"),
            row(
                "mergeable",
                "pull request #12 has no conflicts and GitHub's merge state is clean"
            ),
            row("no_open_threads", "1 review thread, all resolved"),
            row(
                "proof_attached",
                "passes vacuously: Punchlist records no proof yet, so nothing was checked"
            ),
        ]
    );
    // Human Review has no dispatch rule and no gated transition.
    assert_eq!(waiting_dispatches(&pool).await, []);
    let (status, _) = call(
        &s.app,
        &s.person.token,
        "GET",
        "/api/issues/PL-1/gates",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}
