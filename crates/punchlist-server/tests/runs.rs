//! Runners, claims, leases and run logs end to end: HTTP requests against the router, then
//! the rows they wrote, read back from Postgres.

use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use punchlist_server::{AppState, Bootstrap, Bootstrapped, bootstrap, expire_leases, router};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

async fn setup(pool: &PgPool, lease: Duration) -> (Router, Bootstrapped) {
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
    let app = router(AppState::new(pool.clone()).with_lease(lease));
    (app, person)
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

struct Reg {
    id: String,
    token: String,
}

async fn register(app: &Router, person: &str, name: &str, agents: &[&str]) -> Reg {
    let (status, body) = call(
        app,
        person,
        "POST",
        "/api/runners",
        Some(json!({"name": name, "agents": agents})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    Reg {
        id: body["runner"]["id"].as_str().unwrap().to_string(),
        token: body["token"].as_str().unwrap().to_string(),
    }
}

async fn create_and_move(app: &Router, person: &str, title: &str, to: &[&str]) -> String {
    let (status, issue) = call(
        app,
        person,
        "POST",
        "/api/issues",
        Some(json!({"title": title, "body": ""})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let id = issue["id"].as_str().unwrap().to_string();
    for to in to {
        let (status, _) = call(
            app,
            person,
            "POST",
            &format!("/api/issues/{id}/transitions"),
            Some(json!({"to": to})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }
    id
}

async fn claim(app: &Router, runner: &Reg, wait: u32) -> (StatusCode, Value) {
    call(
        app,
        &runner.token,
        "POST",
        "/api/runs/claim",
        Some(json!({"wait_seconds": wait})),
    )
    .await
}

async fn assert_one_attempt_and_winner(pool: &PgPool, winner: &str) {
    let attempts = sqlx::query_scalar!("SELECT count(*) FROM attempt WHERE issue_id = 'PL-1'")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(attempts, Some(1));
    let status = sqlx::query_scalar!("SELECT status FROM issue WHERE id = 'PL-1'")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(status, "in_progress");
    let last = sqlx::query!(
        "SELECT a.name, a.role, t.to_status FROM transition t JOIN actor a ON a.id = t.actor_id
         WHERE t.issue_id = 'PL-1' AND t.to_status = 'in_progress'"
    )
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(last.name, winner);
    assert_eq!(last.role, "runner");
    assert_eq!(last.to_status, "in_progress");
}

#[sqlx::test]
async fn two_runners_claim_one_started_issue_once_without_waiting(pool: PgPool) {
    two_runners_race(&pool, 0).await;
}

#[sqlx::test]
async fn two_runners_claim_one_started_issue_once_with_wait(pool: PgPool) {
    two_runners_race(&pool, 2).await;
}

async fn two_runners_race(pool: &PgPool, wait: u32) {
    let (app, person) = setup(pool, Duration::from_secs(30)).await;
    let r1 = register(&app, &person.token, "r1", &["claude-code"]).await;
    let r2 = register(&app, &person.token, "r2", &["claude-code"]).await;
    create_and_move(&app, &person.token, "Do the thing", &["todo", "start"]).await;

    let ((s1, b1), (s2, b2)) = tokio::join!(claim(&app, &r1, wait), claim(&app, &r2, wait));
    let outcomes = [(s1, &b1, "r1"), (s2, &b2, "r2")];
    let wins: Vec<_> = outcomes
        .iter()
        .filter(|(s, ..)| *s == StatusCode::OK)
        .collect();
    assert_eq!(wins.len(), 1, "{s1} {b1} / {s2} {b2}");
    let (_, claim_body, winner) = wins[0];
    assert_eq!(claim_body["issue"]["id"], "PL-1");
    assert_eq!(claim_body["issue"]["status"], "in_progress");
    assert_eq!(claim_body["attempt"], 1);
    assert_eq!(claim_body["agent"], "claude-code");
    assert_eq!(claim_body["branch"], "feature/pl-1-do-the-thing");
    assert_eq!(claim_body["repository"]["owner"], "gannonh");
    for (s, b, _) in outcomes.iter().filter(|(s, ..)| *s != StatusCode::OK) {
        assert!(
            *s == StatusCode::NO_CONTENT
                || (*s == StatusCode::CONFLICT && b["code"] == "claim_taken"),
            "{s} {b}"
        );
    }
    assert_one_attempt_and_winner(pool, winner).await;
}

#[sqlx::test]
async fn two_long_polling_runners_one_claims_the_other_gets_claim_taken(pool: PgPool) {
    let (app, person) = setup(&pool, Duration::from_secs(30)).await;
    let r1 = register(&app, &person.token, "r1", &["claude-code"]).await;
    let r2 = register(&app, &person.token, "r2", &["claude-code"]).await;
    let (a1, a2, a3) = (app.clone(), app.clone(), app.clone());
    let (t1, t2) = (
        Reg {
            id: r1.id.clone(),
            token: r1.token.clone(),
        },
        Reg {
            id: r2.id.clone(),
            token: r2.token.clone(),
        },
    );
    let h1 = tokio::spawn(async move { claim(&a1, &t1, 5).await });
    let h2 = tokio::spawn(async move { claim(&a2, &t2, 5).await });
    tokio::time::sleep(Duration::from_millis(500)).await;
    create_and_move(&a3, &person.token, "Wait for it", &["todo", "start"]).await;
    let (s1, b1) = h1.await.unwrap();
    let (s2, b2) = h2.await.unwrap();

    let (winner, loser_status, loser_body) = if s1 == StatusCode::OK {
        ("r1", s2, b2)
    } else {
        ("r2", s1, b1)
    };
    assert_eq!(loser_status, StatusCode::CONFLICT, "{loser_body}");
    assert_eq!(loser_body["code"], "claim_taken");
    assert_eq!(
        loser_body["message"],
        format!("PL-1 was claimed by runner {winner}")
    );
    assert_one_attempt_and_winner(&pool, winner).await;
}

#[sqlx::test]
async fn a_long_poll_with_nothing_started_times_out_with_204(pool: PgPool) {
    let (app, person) = setup(&pool, Duration::from_secs(30)).await;
    let r1 = register(&app, &person.token, "r1", &["claude-code"]).await;
    let (status, body) = claim(&app, &r1, 1).await;
    assert_eq!((status, body), (StatusCode::NO_CONTENT, Value::Null));
}

#[sqlx::test]
async fn a_runner_without_the_dispatch_agent_claims_nothing(pool: PgPool) {
    let (app, person) = setup(&pool, Duration::from_secs(30)).await;
    let codex = register(&app, &person.token, "codex-box", &["codex"]).await;
    create_and_move(&app, &person.token, "Work", &["todo", "start"]).await;
    let (status, _) = claim(&app, &codex, 0).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let status = sqlx::query_scalar!("SELECT status FROM issue WHERE id = 'PL-1'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "start");
}

#[sqlx::test]
async fn expired_lease_fails_the_attempt_and_a_heartbeat_renews_it(pool: PgPool) {
    let (app, person) = setup(&pool, Duration::from_secs(1)).await;
    let r1 = register(&app, &person.token, "r1", &["claude-code"]).await;
    create_and_move(&app, &person.token, "Lease", &["todo", "start"]).await;
    let (status, _) = claim(&app, &r1, 0).await;
    assert_eq!(status, StatusCode::OK);

    tokio::time::sleep(Duration::from_millis(700)).await;
    let (status, hb) = call(
        &app,
        &r1.token,
        "POST",
        &format!("/api/runners/{}/heartbeat", r1.id),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(hb["name"], "r1");
    tokio::time::sleep(Duration::from_millis(700)).await;
    // 1.4 s after the claim, but only 0.7 s after the heartbeat.
    assert_eq!(expire_leases(&pool).await.unwrap(), 0);
    let state = sqlx::query_scalar!("SELECT state FROM attempt WHERE issue_id = 'PL-1'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(state, "running");

    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(expire_leases(&pool).await.unwrap(), 1);
    let attempt = sqlx::query!(
        "SELECT state, failure_reason, finished_at FROM attempt WHERE issue_id = 'PL-1'"
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(attempt.state, "failed");
    assert_eq!(
        attempt.failure_reason.as_deref(),
        Some("lease expired: runner r1 stopped sending heartbeats")
    );
    assert!(attempt.finished_at.is_some());
    let outcome = sqlx::query_scalar!("SELECT outcome FROM run WHERE issue_id = 'PL-1'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(outcome, "failed");
    let issue = sqlx::query_scalar!("SELECT status FROM issue WHERE id = 'PL-1'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(issue, "in_progress");
}

#[sqlx::test]
async fn registering_needs_a_person_and_heartbeats_are_own_runner_only(pool: PgPool) {
    let (app, person) = setup(&pool, Duration::from_secs(30)).await;
    let r1 = register(&app, &person.token, "r1", &["claude-code"]).await;
    let r2 = register(&app, &person.token, "r2", &["claude-code"]).await;

    let (status, body) = call(
        &app,
        &r1.token,
        "POST",
        "/api/runners",
        Some(json!({"name": "r3", "agents": ["claude-code"]})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["code"], "forbidden");

    for bad in [
        json!({"name": "", "agents": ["claude-code"]}),
        json!({"name": "x", "agents": []}),
    ] {
        let (status, _) = call(&app, &person.token, "POST", "/api/runners", Some(bad)).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    }

    let (status, body) = call(
        &app,
        &r1.token,
        "POST",
        &format!("/api/runners/{}/heartbeat", r2.id),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, _) = call(
        &app,
        &r1.token,
        "POST",
        "/api/runners/00000000-0000-0000-0000-000000000000/heartbeat",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(
        &app,
        &person.token,
        "POST",
        "/api/runs/claim",
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, list) = call(&app, &person.token, "GET", "/api/runners", None).await;
    assert_eq!(status, StatusCode::OK);
    let names: Vec<_> = list["runners"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["r1", "r2"]);
}

#[sqlx::test]
async fn logs_are_numbered_finish_records_the_outcome_and_stale_attempts_are_refused(pool: PgPool) {
    let (app, person) = setup(&pool, Duration::from_secs(30)).await;
    let r1 = register(&app, &person.token, "r1", &["claude-code"]).await;
    create_and_move(&app, &person.token, "Log it", &["todo", "start"]).await;
    let (_, claimed) = claim(&app, &r1, 0).await;
    let run_id = claimed["run_id"].as_str().unwrap().to_string();

    let log_uri = format!("/api/runs/{run_id}/log");
    let (status, _) = call(
        &app,
        &r1.token,
        "POST",
        &log_uri,
        Some(json!({"attempt": 1, "lines": ["one", "two"]})),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let many: Vec<String> = (3..=30).map(|n| format!("line {n}")).collect();
    let (status, _) = call(
        &app,
        &r1.token,
        "POST",
        &log_uri,
        Some(json!({"attempt": 1, "lines": many})),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Wrong attempt, oversize line and too many lines are refused.
    let (status, body) = call(
        &app,
        &r1.token,
        "POST",
        &log_uri,
        Some(json!({"attempt": 2, "lines": ["x"]})),
    )
    .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::CONFLICT, Some("stale_attempt"))
    );
    let (status, _) = call(
        &app,
        &r1.token,
        "POST",
        &log_uri,
        Some(json!({"attempt": 1, "lines": ["x".repeat(64 * 1024 + 1)]})),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = call(
        &app,
        &r1.token,
        "POST",
        &log_uri,
        Some(json!({"attempt": 1, "lines": vec!["x"; 1001]})),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    let (status, log) = call(&app, &person.token, "GET", &log_uri, None).await;
    assert_eq!(status, StatusCode::OK);
    let lines = log["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 30);
    for (i, line) in lines.iter().enumerate() {
        assert_eq!(line["seq"], i as i64 + 1);
    }
    assert_eq!(lines[0]["line"], "one");
    assert_eq!(lines[1]["line"], "two");
    assert_eq!(lines[29]["line"], "line 30");

    // Finish validation.
    let finish_uri = format!("/api/runs/{run_id}/finish");
    for bad in [
        json!({"attempt": 1, "outcome": "running", "duration_ms": 1}),
        json!({"attempt": 1, "outcome": "failed", "duration_ms": 1}),
        json!({"attempt": 1, "outcome": "succeeded", "duration_ms": -1}),
        json!({"attempt": 1, "outcome": "succeeded", "duration_ms": 1, "input_tokens": -5}),
    ] {
        let (status, _) = call(&app, &r1.token, "POST", &finish_uri, Some(bad)).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    }
    let (status, run) = call(
        &app,
        &r1.token,
        "POST",
        &finish_uri,
        Some(
            json!({"attempt": 1, "outcome": "succeeded", "duration_ms": 4200,
                    "input_tokens": 1000, "output_tokens": 250}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{run}");
    assert_eq!(run["outcome"], "succeeded");
    assert_eq!(run["duration_ms"], 4200);
    assert_eq!(run["input_tokens"], 1000);
    assert_eq!(run["output_tokens"], 250);

    // After finish, the attempt is stale.
    let (status, body) = call(
        &app,
        &r1.token,
        "POST",
        &log_uri,
        Some(json!({"attempt": 1, "lines": ["late"]})),
    )
    .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::CONFLICT, Some("stale_attempt"))
    );
    let (status, body) = call(
        &app,
        &r1.token,
        "POST",
        &finish_uri,
        Some(json!({"attempt": 1, "outcome": "succeeded", "duration_ms": 1})),
    )
    .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::CONFLICT, Some("stale_attempt"))
    );

    let row = sqlx::query!("SELECT outcome, finished_at FROM run WHERE issue_id = 'PL-1'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row.outcome, "succeeded");
    assert!(row.finished_at.is_some());

    let (status, runs) = call(&app, &person.token, "GET", "/api/issues/PL-1/runs", None).await;
    assert_eq!(status, StatusCode::OK);
    let runs = runs.as_array().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0]["agent_name"], "Claude Code");
    assert_eq!(runs[0]["runner"]["name"], "r1");
    let tail = runs[0]["log_tail"].as_array().unwrap();
    assert_eq!(tail.len(), 20);
    assert_eq!(tail[0]["line"], "line 11");
    assert_eq!(tail[19]["line"], "line 30");

    let (status, _) = call(&app, &person.token, "GET", "/api/issues/PL-9/runs", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
