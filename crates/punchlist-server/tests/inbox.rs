//! The inbox (PRD R16): HTTP requests against the router with two people and an agent, then
//! the rows they wrote, read back from Postgres.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use punchlist_server::{AppState, Bootstrap, Bootstrapped, bootstrap, hash_token, router};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

async fn workspace(pool: &PgPool, prefix: &str) -> Bootstrapped {
    bootstrap(
        pool,
        &Bootstrap {
            workspace_name: prefix.into(),
            issue_prefix: prefix.into(),
            repository_owner: "gannonh".into(),
            repository_name: prefix.to_lowercase(),
            person_name: "Gannon".into(),
        },
    )
    .await
    .unwrap()
}

async fn call(
    app: &Router,
    token: Option<&str>,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let request = request
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

/// The ids in a person's inbox, newest first, and its unread count.
async fn inbox(app: &Router, token: &str) -> (Vec<String>, i64) {
    let (status, body) = call(app, Some(token), "GET", "/api/inbox", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let ids = (body["items"].as_array().unwrap().iter())
        .map(|item| item["issue"]["id"].as_str().unwrap().to_string())
        .collect();
    (ids, body["unread"].as_i64().unwrap())
}

async fn item(app: &Router, token: &str, id: &str) -> Value {
    let (_, body) = call(app, Some(token), "GET", "/api/inbox", None).await;
    (body["items"].as_array().unwrap().iter())
        .find(|item| item["issue"]["id"] == id)
        .unwrap_or_else(|| panic!("{id} is not in {body}"))
        .clone()
}

async fn mark_read(app: &Router, token: &str, issue: &str, since_seq: i64) -> StatusCode {
    let body = json!({"issue": issue, "kind": "waiting", "since_seq": since_seq});
    call(app, Some(token), "POST", "/api/inbox/read", Some(body))
        .await
        .0
}

async fn patch(app: &Router, token: &str, id: &str, body: Value) {
    let uri = format!("/api/issues/{id}");
    let (status, body) = call(app, Some(token), "PATCH", &uri, Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

async fn move_to(app: &Router, token: &str, id: &str, to: &str) {
    let uri = format!("/api/issues/{id}/transitions");
    let (status, body) = call(app, Some(token), "POST", &uri, Some(json!({"to": to}))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// A second person in the workspace, with their own account and token.
async fn second_person(pool: &PgPool, workspace_id: Uuid) -> Uuid {
    sqlx::query_scalar(
        "WITH acc AS (INSERT INTO account (name, token_hash) VALUES ('Second', $2) RETURNING id)
         INSERT INTO actor (workspace_id, name, role, account_id)
         SELECT $1, 'Second', 'person', id FROM acc RETURNING id",
    )
    .bind(workspace_id)
    .bind(hash_token("plt_second"))
    .fetch_one(pool)
    .await
    .unwrap()
}

#[sqlx::test]
async fn a_persons_inbox_lists_what_waits_on_them(pool: PgPool) {
    let app = router(AppState::new(pool.clone()));
    let first = workspace(&pool, "PL").await;
    let a = first.token.as_str();
    let b = "plt_second";
    let second = second_person(&pool, first.workspace_id).await;
    let (status, agent) = call(
        &app,
        Some(a),
        "POST",
        "/api/actors",
        Some(json!({"kind": "claude-code"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{agent}");
    let agent_id = agent["actor"]["id"].as_str().unwrap();
    let agent_token = agent["token"].as_str().unwrap();

    for title in ["One", "Two", "Three", "Four"] {
        let body = json!({"title": title});
        let (status, body) = call(&app, Some(a), "POST", "/api/issues", Some(body)).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }
    move_to(&app, a, "PL-1", "todo").await;
    sqlx::query("UPDATE issue SET status = 'human_review' WHERE id = 'PL-3'")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE issue SET status = 'merging' WHERE id = 'PL-4'")
        .execute(&pool)
        .await
        .unwrap();

    // Todo, Human Review and Merging wait on a person; Backlog does not. PL-1's move is the
    // newest event; PL-3 and PL-4 were put in their status without one.
    let (_, body) = call(&app, Some(a), "GET", "/api/inbox", None).await;
    assert_eq!(body["unread"], 3);
    assert_eq!(
        body["items"][0],
        json!({
            "kind": "waiting",
            "issue": {"id": "PL-1", "title": "One", "status": "todo", "status_name": "Todo"},
            "since": body["items"][0]["since"],
            "since_seq": 5,
            "unread": true,
        })
    );
    assert_eq!(
        inbox(&app, a).await,
        (vec!["PL-1".into(), "PL-4".into(), "PL-3".into()], 3)
    );
    assert_eq!(
        inbox(&app, b).await,
        (vec!["PL-1".into(), "PL-4".into(), "PL-3".into()], 3)
    );

    // An assignee that is a person sees it alone; an agent's owner sees it alone.
    patch(&app, a, "PL-1", json!({"assignee": second})).await;
    patch(&app, a, "PL-3", json!({"assignee": agent_id})).await;
    patch(&app, a, "PL-4", json!({"assignee": first.actor_id})).await;
    assert_eq!(
        inbox(&app, a).await,
        (vec!["PL-4".into(), "PL-3".into()], 2)
    );
    assert_eq!(inbox(&app, b).await, (vec!["PL-1".into()], 1));

    // An agent, a runner and no token.
    for (token, code) in [(Some(agent_token), 403), (None, 401)] {
        let (status, _) = call(&app, token, "GET", "/api/inbox", None).await;
        assert_eq!(status.as_u16(), code);
        let body = json!({"issue": "PL-4", "kind": "waiting", "since_seq": 4});
        let (status, _) = call(&app, token, "POST", "/api/inbox/read", Some(body)).await;
        assert_eq!(status.as_u16(), code);
    }
    let (status, runner) = call(
        &app,
        Some(a),
        "POST",
        "/api/runners",
        Some(json!({"name": "r1", "agents": ["claude-code"]})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{runner}");
    let runner_token = runner["token"].as_str().unwrap();
    let (status, _) = call(&app, Some(runner_token), "GET", "/api/inbox", None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Opening an item reads it; an older `since_seq` never makes it unread again.
    assert_eq!(mark_read(&app, a, "PL-4", 4).await, StatusCode::NO_CONTENT);
    assert_eq!(
        inbox(&app, a).await,
        (vec!["PL-4".into(), "PL-3".into()], 1)
    );
    assert_eq!(item(&app, a, "PL-4").await["unread"], false);
    assert_eq!(mark_read(&app, a, "PL-4", 1).await, StatusCode::NO_CONTENT);
    assert_eq!(inbox(&app, a).await.1, 1);
    let stored: Vec<(String, i64)> =
        sqlx::query_as("SELECT issue_id, since_seq FROM inbox_read WHERE actor_id = $1")
            .bind(first.actor_id)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(stored, [("PL-4".to_string(), 4)]);
    // The other person's read state is their own.
    assert_eq!(inbox(&app, b).await.1, 1);

    // Leaving a waiting status removes the item; a new move into one makes it unread again.
    move_to(&app, b, "PL-1", "start").await;
    assert_eq!(inbox(&app, b).await, (vec![], 0));
    move_to(&app, a, "PL-4", "todo").await;
    let back = item(&app, a, "PL-4").await;
    assert_eq!(
        (
            &back["issue"]["status"],
            &back["since_seq"],
            &back["unread"]
        ),
        (&json!("todo"), &json!(10), &json!(true))
    );
    let newest: i64 = sqlx::query_scalar(
        "SELECT max(seq) FROM event WHERE issue_id = 'PL-4' AND kind = 'transition'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(newest, 10);
}

#[sqlx::test]
async fn reading_another_workspaces_issue_is_not_found(pool: PgPool) {
    let app = router(AppState::new(pool.clone()));
    let mine = workspace(&pool, "PL").await;
    let theirs = workspace(&pool, "OT").await;
    let body = json!({"title": "Theirs"});
    let (status, _) = call(&app, Some(&theirs.token), "POST", "/api/issues", Some(body)).await;
    assert_eq!(status, StatusCode::CREATED);

    assert_eq!(
        mark_read(&app, &mine.token, "OT-1", 1).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        mark_read(&app, &mine.token, "PL-9", 1).await,
        StatusCode::NOT_FOUND
    );
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM inbox_read")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(rows, 0);
}

/// Claims the next run as the runner and finishes it with `outcome`; returns its run id.
async fn run_once(app: &Router, runner: &str, id: &str, outcome: Value) -> String {
    let body = json!({"wait_seconds": 0});
    let (status, claimed) = call(app, Some(runner), "POST", "/api/runs/claim", Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{claimed}");
    assert_eq!(claimed["issue"]["id"], id);
    let run_id = claimed["run_id"].as_str().unwrap().to_string();
    let mut finish = json!({"attempt": 1, "duration_ms": 1});
    finish
        .as_object_mut()
        .unwrap()
        .extend(outcome.as_object().unwrap().clone());
    let uri = format!("/api/runs/{run_id}/finish");
    let (status, body) = call(app, Some(runner), "POST", &uri, Some(finish)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    run_id
}

fn succeeded() -> Value {
    json!({"outcome": "succeeded"})
}

fn crashed() -> Value {
    json!({"outcome": "failed", "reason": "agent crashed"})
}

async fn newest_run_event(pool: &PgPool, id: &str) -> i64 {
    sqlx::query_scalar("SELECT max(seq) FROM event WHERE issue_id = $1 AND kind = 'run'")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

fn runs_of(items: &(Vec<String>, i64), id: &str) -> usize {
    items.0.iter().filter(|item| *item == id).count()
}

#[sqlx::test]
async fn the_owner_of_a_runs_agent_sees_an_issue_nothing_dispatches_again(pool: PgPool) {
    let app = router(AppState::new(pool.clone()));
    let first = workspace(&pool, "PL").await;
    let a = first.token.as_str();
    let b = "plt_second";
    second_person(&pool, first.workspace_id).await;
    let (status, runner) = call(
        &app,
        Some(a),
        "POST",
        "/api/runners",
        Some(json!({"name": "r", "agents": ["claude-code"]})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{runner}");
    let r = runner["token"].as_str().unwrap();
    for title in ["Succeeds", "Crashes"] {
        let (status, body) = call(
            &app,
            Some(a),
            "POST",
            "/api/issues",
            Some(json!({"title": title})),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }

    // Three runs that end without moving PL-1: an item only after the third.
    move_to(&app, a, "PL-1", "todo").await;
    move_to(&app, a, "PL-1", "start").await;
    for _ in 0..2 {
        run_once(&app, r, "PL-1", succeeded()).await;
        assert_eq!(inbox(&app, a).await, (vec![], 0));
    }
    run_once(&app, r, "PL-1", succeeded()).await;
    let limit = item(&app, a, "PL-1").await;
    assert_eq!(
        limit,
        json!({
            "kind": "run",
            "issue": {"id": "PL-1", "title": "Succeeds", "status": "in_progress",
                      "status_name": "In Progress"},
            "agent": "claude-code",
            "runs": 3,
            "last_outcome": "succeeded",
            "reason": null,
            "waiting_reason": null,
            "limit_reached": true,
            "since": limit["since"],
            "since_seq": newest_run_event(&pool, "PL-1").await,
            "unread": true,
        })
    );
    let (_, issue) = call(&app, Some(a), "GET", "/api/issues/PL-1", None).await;
    assert_eq!(
        issue["waiting_on_person"],
        "3 runs in In Progress ended without moving it"
    );

    // A failed run with a retry queued, or running, is no item; the third failure is one.
    move_to(&app, a, "PL-2", "todo").await;
    move_to(&app, a, "PL-2", "start").await;
    run_once(&app, r, "PL-2", crashed()).await;
    let queued: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM dispatch WHERE issue_id = 'PL-2' AND run_id IS NULL",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(queued, 1);
    assert_eq!(runs_of(&inbox(&app, a).await, "PL-2"), 0);
    run_once(&app, r, "PL-2", crashed()).await;
    run_once(&app, r, "PL-2", crashed()).await;
    assert_eq!(runs_of(&inbox(&app, a).await, "PL-2"), 1);
    let failed = item(&app, a, "PL-2").await;
    assert_eq!(
        (
            &failed["last_outcome"],
            &failed["reason"],
            &failed["runs"],
            &failed["limit_reached"]
        ),
        (
            &json!("failed"),
            &json!("agent crashed"),
            &json!(3),
            &json!(true)
        )
    );
    let (_, issue) = call(&app, Some(a), "GET", "/api/issues/PL-2", None).await;
    assert_eq!(
        issue["waiting_on_person"],
        "3 runs in In Progress ended without moving it"
    );

    // The person who did not register the runner sees neither.
    assert_eq!(inbox(&app, b).await, (vec![], 0));
    assert_eq!(
        inbox(&app, a).await,
        (vec!["PL-2".into(), "PL-1".into()], 2)
    );

    // Opening a run item reads it, apart from a waiting item for the issue.
    let since_seq = failed["since_seq"].as_i64().unwrap();
    let body = json!({"issue": "PL-2", "kind": "run", "since_seq": since_seq});
    let (status, _) = call(&app, Some(a), "POST", "/api/inbox/read", Some(body)).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(inbox(&app, a).await.1, 1);
    let stored: Vec<(String, String, i64)> =
        sqlx::query_as("SELECT issue_id, kind, since_seq FROM inbox_read")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(stored, [("PL-2".to_string(), "run".to_string(), since_seq)]);

    // A run that failed with nothing queued after it is an item before the limit.
    sqlx::query("UPDATE issue SET status = 'todo' WHERE id = 'PL-2'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(runs_of(&inbox(&app, a).await, "PL-2"), 1);
    assert_eq!(item(&app, a, "PL-2").await["kind"], "waiting");
    move_to(&app, a, "PL-2", "start").await;
    run_once(&app, r, "PL-2", crashed()).await;
    sqlx::query("DELETE FROM dispatch WHERE issue_id = 'PL-2' AND run_id IS NULL")
        .execute(&pool)
        .await
        .unwrap();
    let early = item(&app, a, "PL-2").await;
    assert_eq!(
        (
            &early["kind"],
            &early["runs"],
            &early["limit_reached"],
            &early["reason"]
        ),
        (
            &json!("run"),
            &json!(1),
            &json!(false),
            &json!("agent crashed")
        )
    );
    let (_, issue) = call(&app, Some(a), "GET", "/api/issues/PL-2", None).await;
    assert_eq!(issue["waiting_on_person"], Value::Null);
}

#[sqlx::test]
async fn the_owner_of_a_refused_agent_sees_the_refusal_until_the_issue_moves(pool: PgPool) {
    let app = router(AppState::new(pool.clone()));
    let first = workspace(&pool, "PL").await;
    let a = first.token.as_str();
    let b = "plt_second";
    second_person(&pool, first.workspace_id).await;
    let (status, agent) = call(
        &app,
        Some(a),
        "POST",
        "/api/actors",
        Some(json!({"kind": "codex", "name": "Reviewer"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{agent}");
    let agent_id = agent["actor"]["id"].clone();
    let agent = agent["token"].as_str().unwrap();
    let (status, body) = call(
        &app,
        Some(a),
        "POST",
        "/api/issues",
        Some(json!({"title": "Built"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    sqlx::query("UPDATE issue SET status = 'in_progress' WHERE id = 'PL-1'")
        .execute(&pool)
        .await
        .unwrap();
    let request = json!({"to": "agent_review"});
    let uri = "/api/issues/PL-1/transitions";
    let (status, _) = call(&app, Some(agent), "POST", uri, Some(request.clone())).await;
    assert_eq!(status, StatusCode::CONFLICT);

    let since_seq: i64 =
        sqlx::query_scalar("SELECT seq FROM event WHERE issue_id = 'PL-1' AND kind = 'refusal'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let refused = item(&app, a, "PL-1").await;
    assert_eq!(
        refused,
        json!({
            "kind": "refusal",
            "issue": {"id": "PL-1", "title": "Built", "status": "in_progress",
                      "status_name": "In Progress"},
            "agent": {"id": agent_id, "name": "Reviewer", "role": "agent", "kind": "codex"},
            "from": "in_progress",
            "from_name": "In Progress",
            "to": "agent_review",
            "to_name": "Agent Review",
            "gate": "pr_open",
            "reason": "no open pull request is linked to PL-1",
            "since": refused["since"],
            "since_seq": since_seq,
            "unread": true,
        })
    );
    assert_eq!(inbox(&app, a).await, (vec!["PL-1".into()], 1));
    assert_eq!(inbox(&app, b).await, (vec![], 0));

    // Opening it reads it, as its own kind.
    let body = json!({"issue": "PL-1", "kind": "refusal", "since_seq": since_seq});
    let (status, _) = call(&app, Some(a), "POST", "/api/inbox/read", Some(body)).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(inbox(&app, a).await, (vec!["PL-1".into()], 0));

    // The agent's next request is allowed: the issue moved, and the item leaves.
    sqlx::query(
        "INSERT INTO pull_request (repository_id, number, github_id, title, branch, head_sha,
                                   url, author_login, state, draft, issue_id, github_updated_at)
         SELECT id, 1, 1, 'Built (PL-1)', 'feature/pl-1', 'abc', 'https://github.com/x/y/pull/1',
                'gannonh', 'open', false, 'PL-1', now() FROM repository",
    )
    .execute(&pool)
    .await
    .unwrap();
    let (status, body) = call(&app, Some(agent), "POST", uri, Some(request)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(inbox(&app, a).await, (vec![], 0));
}

/// What `get_issue` says PL-1 waits on a person for, asserting the inbox lists it just then.
async fn agree(app: &Router, token: &str, listed: bool) -> Option<String> {
    let (_, issue) = call(app, Some(token), "GET", "/api/issues/PL-1", None).await;
    let waiting = issue["waiting_on_person"].as_str().map(str::to_string);
    assert_eq!(
        runs_of(&inbox(app, token).await, "PL-1"),
        usize::from(listed)
    );
    assert_eq!(waiting.is_some(), listed, "{issue}");
    waiting
}

#[sqlx::test]
async fn run_again_takes_the_issue_out_of_the_inbox_until_its_next_round_runs_out(pool: PgPool) {
    let app = router(AppState::new(pool.clone()));
    let first = workspace(&pool, "PL").await;
    let a = first.token.as_str();
    let (status, runner) = call(
        &app,
        Some(a),
        "POST",
        "/api/runners",
        Some(json!({"name": "r", "agents": ["claude-code"]})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{runner}");
    let r = runner["token"].as_str().unwrap();
    let (status, body) = call(
        &app,
        Some(a),
        "POST",
        "/api/issues",
        Some(json!({"title": "Build"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    move_to(&app, a, "PL-1", "todo").await;
    move_to(&app, a, "PL-1", "start").await;
    for _ in 0..3 {
        agree(&app, a, false).await;
        run_once(&app, r, "PL-1", succeeded()).await;
    }
    assert_eq!(
        agree(&app, a, true).await.as_deref(),
        Some("3 runs in In Progress ended without moving it")
    );

    let (status, body) = call(&app, Some(a), "POST", "/api/issues/PL-1/retry", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    for _ in 0..3 {
        agree(&app, a, false).await;
        run_once(&app, r, "PL-1", succeeded()).await;
    }
    agree(&app, a, true).await;
    let again = item(&app, a, "PL-1").await;
    assert_eq!(
        (&again["runs"], &again["limit_reached"]),
        (&json!(3), &json!(true))
    );
}

/// Claims PL-`n`'s run, has its agent call `wait_for_person` with `reason`, and asserts the
/// owner's inbox lists nothing for the issue while the run runs; then finishes it with
/// `outcome`.
async fn wait_then_finish(
    app: &Router,
    a: &str,
    runner: &str,
    id: &str,
    reason: &str,
    outcome: Value,
) {
    let body = json!({"wait_seconds": 0});
    let (status, claimed) = call(app, Some(runner), "POST", "/api/runs/claim", Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{claimed}");
    assert_eq!(claimed["issue"]["id"], id);
    let run_id = claimed["run_id"].as_str().unwrap();
    let (status, body) = call(
        app,
        claimed["agent_token"].as_str(),
        "POST",
        &format!("/api/runs/{run_id}/waiting"),
        Some(json!({"reason": reason})),
    )
    .await;
    assert!(status.is_success(), "{body}");
    assert_eq!(runs_of(&inbox(app, a).await, id), 0);
    let mut finish = json!({"attempt": 1, "duration_ms": 1});
    finish
        .as_object_mut()
        .unwrap()
        .extend(outcome.as_object().unwrap().clone());
    let uri = format!("/api/runs/{run_id}/finish");
    let (status, body) = call(app, Some(runner), "POST", &uri, Some(finish)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[sqlx::test]
async fn the_owner_sees_a_run_whose_agent_waits_on_a_person_once_it_ends(pool: PgPool) {
    let app = router(AppState::new(pool.clone()));
    let first = workspace(&pool, "PL").await;
    let a = first.token.as_str();
    let b = "plt_second";
    second_person(&pool, first.workspace_id).await;
    let (status, runner) = call(
        &app,
        Some(a),
        "POST",
        "/api/runners",
        Some(json!({"name": "r", "agents": ["claude-code"]})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{runner}");
    let r = runner["token"].as_str().unwrap();
    for title in ["Succeeds", "Fails"] {
        let (status, body) = call(
            &app,
            Some(a),
            "POST",
            "/api/issues",
            Some(json!({"title": title})),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        let id = if title == "Succeeds" { "PL-1" } else { "PL-2" };
        move_to(&app, a, id, "todo").await;
        move_to(&app, a, id, "start").await;
    }

    let reason = "<b>bold</b> *star*";
    wait_then_finish(&app, a, r, "PL-1", reason, succeeded()).await;
    let waits = item(&app, a, "PL-1").await;
    assert_eq!(
        waits,
        json!({
            "kind": "run",
            "issue": {"id": "PL-1", "title": "Succeeds", "status": "in_progress",
                      "status_name": "In Progress"},
            "agent": "claude-code",
            "runs": 1,
            "last_outcome": "succeeded",
            "reason": null,
            "waiting_reason": "<b>bold</b> *star*",
            "limit_reached": false,
            "since": waits["since"],
            "since_seq": newest_run_event(&pool, "PL-1").await,
            "unread": true,
        })
    );
    let (_, issue) = call(&app, Some(a), "GET", "/api/issues/PL-1", None).await;
    assert_eq!(
        issue["waiting_on_person"],
        "Claude Code is waiting on a person: <b>bold</b> *star*"
    );

    wait_then_finish(&app, a, r, "PL-2", "Needs a key.", crashed()).await;
    let failed = item(&app, a, "PL-2").await;
    assert_eq!(
        (
            &failed["last_outcome"],
            &failed["reason"],
            &failed["waiting_reason"],
            &failed["runs"],
            &failed["limit_reached"]
        ),
        (
            &json!("failed"),
            &json!("agent crashed"),
            &json!("Needs a key."),
            &json!(1),
            &json!(false)
        )
    );
    assert_eq!(
        inbox(&app, a).await,
        (vec!["PL-2".into(), "PL-1".into()], 2)
    );
    assert_eq!(inbox(&app, b).await, (vec![], 0));

    // Run again queues a run and the item leaves.
    let (status, body) = call(&app, Some(a), "POST", "/api/issues/PL-1/retry", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(inbox(&app, a).await, (vec!["PL-2".into()], 1));
}

/// Each item in a person's inbox as its kind and issue id, newest first, and the unread count.
async fn kinds(app: &Router, token: &str) -> (Vec<(String, String)>, i64) {
    let (status, body) = call(app, Some(token), "GET", "/api/inbox", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let items = (body["items"].as_array().unwrap().iter())
        .map(|item| {
            let kind = item["kind"].as_str().unwrap().to_string();
            (kind, item["issue"]["id"].as_str().unwrap().to_string())
        })
        .collect();
    (items, body["unread"].as_i64().unwrap())
}

fn of(kind: &str, id: &str) -> (String, String) {
    (kind.to_string(), id.to_string())
}

/// A workspace with a runner for Claude Code and an issue in Start for each title, PL-1 first;
/// returns the router, the person's token and the runner's.
async fn one_runner(pool: &PgPool, titles: &[&str]) -> (Router, String, String) {
    let app = router(AppState::new(pool.clone()));
    let a = workspace(pool, "PL").await.token;
    let (status, runner) = call(
        &app,
        Some(&a),
        "POST",
        "/api/runners",
        Some(json!({"name": "r", "agents": ["claude-code"]})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{runner}");
    for (n, title) in titles.iter().enumerate() {
        let body = Some(json!({"title": title}));
        let (status, body) = call(&app, Some(&a), "POST", "/api/issues", body).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        let id = format!("PL-{}", n + 1);
        move_to(&app, &a, &id, "todo").await;
        move_to(&app, &a, &id, "start").await;
    }
    (app, a, runner["token"].as_str().unwrap().to_string())
}

/// Claims PL-`n`'s run and has its agent ask for Agent Review, which `pr_open` refuses;
/// returns the run's id and the agent's token.
async fn claim_and_refuse(app: &Router, runner: &str, id: &str) -> (String, String) {
    let body = json!({"wait_seconds": 0});
    let (status, claimed) = call(app, Some(runner), "POST", "/api/runs/claim", Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{claimed}");
    assert_eq!(claimed["issue"]["id"], id);
    let agent = claimed["agent_token"].as_str().unwrap().to_string();
    let uri = format!("/api/issues/{id}/transitions");
    let request = json!({"to": "agent_review"});
    let (status, body) = call(app, Some(&agent), "POST", &uri, Some(request)).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    (claimed["run_id"].as_str().unwrap().to_string(), agent)
}

async fn finish(app: &Router, runner: &str, run_id: &str) {
    let uri = format!("/api/runs/{run_id}/finish");
    let body = json!({"attempt": 1, "duration_ms": 1, "outcome": "succeeded"});
    let (status, body) = call(app, Some(runner), "POST", &uri, Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[sqlx::test]
async fn a_refusal_is_listed_only_once_the_run_has_ended_on_it(pool: PgPool) {
    let (app, a, r) = one_runner(&pool, &["Refused"]).await;
    let (a, r) = (a.as_str(), r.as_str());

    // Refused while its run runs: nothing, the count included. Its end queues the next run.
    for _ in 0..2 {
        let (run, _) = claim_and_refuse(&app, r, "PL-1").await;
        assert_eq!(kinds(&app, a).await, (vec![], 0));
        finish(&app, r, &run).await;
        assert_eq!(kinds(&app, a).await, (vec![], 0));
    }

    // The entry's last run ends on a refusal, with nothing queued: the refusal is listed.
    let (run, _) = claim_and_refuse(&app, r, "PL-1").await;
    assert_eq!(kinds(&app, a).await, (vec![], 0));
    finish(&app, r, &run).await;
    let refusal_seq: i64 = sqlx::query_scalar(
        "SELECT max(seq) FROM event WHERE issue_id = 'PL-1' AND kind = 'refusal'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let refused = item_of(&app, a, "refusal", "PL-1").await;
    assert_eq!(
        refused,
        json!({
            "kind": "refusal",
            "issue": {"id": "PL-1", "title": "Refused", "status": "in_progress",
                      "status_name": "In Progress"},
            "agent": {"id": refused["agent"]["id"], "name": "Claude Code", "role": "agent",
                      "kind": "claude-code"},
            "from": "in_progress",
            "from_name": "In Progress",
            "to": "agent_review",
            "to_name": "Agent Review",
            "gate": "pr_open",
            "reason": "no open pull request is linked to PL-1",
            "since": refused["since"],
            "since_seq": refusal_seq,
            "unread": true,
        })
    );
    assert_eq!(
        kinds(&app, a).await,
        (vec![of("run", "PL-1"), of("refusal", "PL-1")], 2)
    );

    // Run again queues a run: the refusal leaves with the run item.
    let (status, body) = call(&app, Some(a), "POST", "/api/issues/PL-1/retry", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(kinds(&app, a).await, (vec![], 0));
    let (run, _) = claim_and_refuse(&app, r, "PL-1").await;
    assert_eq!(kinds(&app, a).await, (vec![], 0));
    finish(&app, r, &run).await;
    assert_eq!(kinds(&app, a).await, (vec![], 0));
}

#[sqlx::test]
async fn an_issue_refused_then_waiting_on_a_person_is_listed_once_as_waiting(pool: PgPool) {
    let (app, a, r) = one_runner(&pool, &["Waits"]).await;
    let (run, agent) = claim_and_refuse(&app, &r, "PL-1").await;
    let uri = format!("/api/runs/{run}/waiting");
    let body = json!({"reason": "Needs a key."});
    let (status, body) = call(&app, Some(&agent), "POST", &uri, Some(body)).await;
    assert!(status.is_success(), "{body}");
    assert_eq!(kinds(&app, &a).await, (vec![], 0));
    finish(&app, &r, &run).await;
    assert_eq!(kinds(&app, &a).await, (vec![of("run", "PL-1")], 1));
    let waits = item_of(&app, &a, "run", "PL-1").await;
    assert_eq!(
        waits,
        json!({
            "kind": "run",
            "issue": {"id": "PL-1", "title": "Waits", "status": "in_progress",
                      "status_name": "In Progress"},
            "agent": "claude-code",
            "runs": 1,
            "last_outcome": "succeeded",
            "reason": null,
            "waiting_reason": "Needs a key.",
            "limit_reached": false,
            "since": waits["since"],
            "since_seq": newest_run_event(&pool, "PL-1").await,
            "unread": true,
        })
    );
}

async fn item_of(app: &Router, token: &str, kind: &str, id: &str) -> Value {
    let (_, body) = call(app, Some(token), "GET", "/api/inbox", None).await;
    (body["items"].as_array().unwrap().iter())
        .find(|item| item["kind"] == kind && item["issue"]["id"] == id)
        .unwrap_or_else(|| panic!("no {kind} item for {id} in {body}"))
        .clone()
}
