//! What the web app reads and writes: the issue rows, edits, the event stream, the
//! browser's session and the app's own files, end to end against the router.

use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use hmac::{Hmac, KeyInit, Mac};
use http_body_util::BodyExt;
use punchlist_server::{
    AppState, Bootstrap, Bootstrapped, bootstrap, router, run_jobs_until_idle, with_web_app,
    write_pull_request_events,
};
use serde_json::{Value, json};
use sha2::Sha256;
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

const SECRET: &str = "test-webhook-secret";
const OPENED: &str = include_str!("fixtures/github/pull_request_opened.json");
const READY: &str = include_str!("fixtures/github/pull_request_ready_for_review.json");
const CHECK_RUN: &str = include_str!("fixtures/github/check_run_completed.json");

async fn workspace(pool: &PgPool, prefix: &str, person: &str) -> Bootstrapped {
    bootstrap(
        pool,
        &Bootstrap {
            workspace_name: prefix.into(),
            issue_prefix: prefix.into(),
            repository_owner: "gannonh".into(),
            repository_name: format!("repo-{prefix}"),
            person_name: person.into(),
        },
    )
    .await
    .unwrap()
}

async fn setup(pool: &PgPool) -> (Router, Bootstrapped) {
    let person = workspace(pool, "PL", "Gannon").await;
    (router(AppState::new(pool.clone())), person)
}

fn request(token: &str, method: &str, uri: &str, body: Option<Value>) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
        .unwrap()
}

async fn send(app: &Router, request: Request<Body>) -> (StatusCode, HeaderMap, Value) {
    let response = app.clone().oneshot(request).await.unwrap();
    let (status, headers) = (response.status(), response.headers().clone());
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|_| json!(String::from_utf8_lossy(&bytes)))
    };
    (status, headers, json)
}

async fn call(
    app: &Router,
    token: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let (status, _, json) = send(app, request(token, method, uri, body)).await;
    (status, json)
}

async fn create(app: &Router, token: &str, title: &str) -> Value {
    let (status, body) = call(
        app,
        token,
        "POST",
        "/api/issues",
        Some(json!({"title": title})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body
}

async fn patch(app: &Router, token: &str, id: &str, body: Value) -> (StatusCode, Value) {
    call(
        app,
        token,
        "PATCH",
        &format!("/api/issues/{id}"),
        Some(body),
    )
    .await
}

async fn list(app: &Router, token: &str) -> Value {
    let (status, body) = call(app, token, "GET", "/api/issues", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

/// Each event's kind and edited fields, in sequence order.
async fn events(pool: &PgPool) -> Vec<(i64, String, Option<Vec<String>>)> {
    sqlx::query_as("SELECT seq, kind, edited_fields FROM event ORDER BY seq")
        .fetch_all(pool)
        .await
        .unwrap()
}

/// The workspace's event stream after `after`, or after `last_event_id` as a reconnecting
/// `EventSource` asks.
async fn open_stream(
    app: &Router,
    person: &Bootstrapped,
    after: i64,
    last_event_id: Option<i64>,
) -> Body {
    let uri = format!(
        "/api/workspaces/{}/events?after={after}",
        person.workspace_id
    );
    let mut request = request(&person.token, "GET", &uri, None);
    if let Some(id) = last_event_id {
        request
            .headers_mut()
            .insert("last-event-id", id.to_string().parse().unwrap());
    }
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response.into_body()
}

/// A fixture delivery for the `repo-PL` repository, `edit`ed, signed and run to idle.
async fn deliver(
    state: &AppState,
    event: &str,
    delivery: &str,
    fixture: &str,
    edit: impl FnOnce(&mut Value),
) {
    let mut payload: Value = serde_json::from_str(fixture).unwrap();
    payload["repository"]["name"] = json!("repo-PL");
    edit(&mut payload);
    let body = payload.to_string();
    let mut mac = Hmac::<Sha256>::new_from_slice(SECRET.as_bytes()).unwrap();
    mac.update(body.as_bytes());
    let hex: String = (mac.finalize().into_bytes().iter())
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let request = Request::builder()
        .method("POST")
        .uri("/api/github/webhook")
        .header("content-type", "application/json")
        .header("x-github-event", event)
        .header("x-github-delivery", delivery)
        .header("x-hub-signature-256", format!("sha256={hex}"))
        .body(Body::from(body))
        .unwrap();
    let response = router(state.clone()).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    run_jobs_until_idle(state).await.unwrap();
}

/// Each message's id, event kind, actor name and the row's `field`.
fn seen(messages: &[(i64, Value)], field: &str) -> Vec<(i64, Value, Value, Value)> {
    (messages.iter())
        .map(|(id, item)| {
            (
                *id,
                item["event"]["detail"]["kind"].clone(),
                item["event"]["actor"]["name"].clone(),
                item["row"][field].clone(),
            )
        })
        .collect()
}

/// Reads the stream until it has delivered `count` messages: each one's `id` and data.
async fn read_stream(body: &mut Body, count: usize) -> Vec<(i64, Value)> {
    let mut text = String::new();
    let mut messages = Vec::new();
    while messages.len() < count {
        let frame = tokio::time::timeout(Duration::from_secs(5), body.frame())
            .await
            .expect("the stream delivered the next event within 5 s")
            .unwrap()
            .unwrap();
        text.push_str(std::str::from_utf8(frame.data_ref().unwrap()).unwrap());
        while let Some(end) = text.find("\n\n") {
            let message: String = text.drain(..end + 2).collect();
            let field = |name: &str| {
                (message.lines()).find_map(|line| line.strip_prefix(&format!("{name}: ")))
            };
            if let (Some(id), Some(data)) = (field("id"), field("data")) {
                assert_eq!(field("event"), Some("issue"));
                messages.push((id.parse().unwrap(), serde_json::from_str(data).unwrap()));
            }
        }
    }
    messages
}

#[sqlx::test]
async fn the_list_and_the_stream_agree_over_a_concurrent_write(pool: PgPool) {
    let (app, person) = setup(&pool).await;
    let created = create(&app, &person.token, "First").await;
    assert_eq!(created["seq"], 1);
    assert_eq!(created["row"]["issue"]["id"], "PL-1");

    // A write that has taken sequence number 2 and not committed.
    let mut held = pool.begin().await.unwrap();
    let seq: i64 = sqlx::query_scalar(
        "UPDATE workspace SET last_event_seq = last_event_seq + 1 WHERE id = $1
         RETURNING last_event_seq",
    )
    .bind(person.workspace_id)
    .fetch_one(&mut *held)
    .await
    .unwrap();
    assert_eq!(seq, 2);
    sqlx::query("UPDATE issue SET title = 'Renamed' WHERE id = 'PL-1'")
        .execute(&mut *held)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO event (workspace_id, seq, issue_id, kind, actor_id, edited_fields)
         VALUES ($1, 2, 'PL-1', 'edited', $2, '{title}')",
    )
    .bind(person.workspace_id)
    .bind(person.actor_id)
    .execute(&mut *held)
    .await
    .unwrap();

    let snapshot = list(&app, &person.token).await;
    assert_eq!(snapshot["last_event_seq"], 1);
    assert_eq!(snapshot["issues"][0]["issue"]["title"], "First");

    let uri = format!(
        "/api/workspaces/{}/events?after={}",
        person.workspace_id, snapshot["last_event_seq"]
    );
    let response = app
        .clone()
        .oneshot(request(&person.token, "GET", &uri, None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    let mut body = response.into_body();
    let early = tokio::time::timeout(Duration::from_millis(1200), body.frame()).await;
    assert!(early.is_err(), "the stream sent an uncommitted event");

    held.commit().await.unwrap();
    let (status, edited) = patch(&app, &person.token, "PL-1", json!({"title": "Final"})).await;
    assert_eq!(status, StatusCode::OK, "{edited}");
    assert_eq!(edited["seq"], 3);

    let messages = read_stream(&mut body, 2).await;
    let seqs: Vec<i64> = messages.iter().map(|(id, _)| *id).collect();
    assert_eq!(seqs, [2, 3]);
    for (id, item) in &messages {
        assert_eq!(item["event"]["seq"], *id);
    }
    assert_eq!(
        messages[0].1["event"]["detail"],
        json!({"kind": "edited", "fields": ["title"]})
    );
    assert_eq!(messages[1].1["row"]["issue"]["title"], "Final");
    assert_eq!(messages[1].1["row"]["run"], json!({"state": "idle"}));
}

#[sqlx::test]
async fn an_open_stream_ends_when_the_server_shuts_down(pool: PgPool) {
    let person = workspace(&pool, "PL", "Gannon").await;
    let (stop, stopping) = tokio::sync::watch::channel(false);
    let app = router(AppState::new(pool.clone()).with_shutdown(stopping));
    create(&app, &person.token, "First").await;
    let uri = format!("/api/workspaces/{}/events?after=0", person.workspace_id);
    let response = app
        .oneshot(request(&person.token, "GET", &uri, None))
        .await
        .unwrap();
    let mut body = response.into_body();
    assert_eq!(read_stream(&mut body, 1).await[0].0, 1);
    stop.send(true).unwrap();
    let end = loop {
        let frame = tokio::time::timeout(Duration::from_secs(2), body.frame())
            .await
            .expect("the stream stayed open after shutdown");
        match frame {
            Some(Ok(frame)) if frame.is_data() => continue,
            other => break other.map(|frame| frame.is_ok()),
        }
    };
    assert_eq!(end, None);
}

#[sqlx::test]
async fn the_stream_is_only_for_the_tokens_workspace(pool: PgPool) {
    let (app, person) = setup(&pool).await;
    let other = workspace(&pool, "OT", "Ola").await;
    let uri = format!("/api/workspaces/{}/events?after=0", other.workspace_id);
    let (status, body) = call(&app, &person.token, "GET", &uri, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");
}

#[sqlx::test]
async fn an_edit_writes_one_event_naming_the_fields_it_changed(pool: PgPool) {
    let (app, person) = setup(&pool).await;
    create(&app, &person.token, "First").await;

    let (status, edited) = patch(
        &app,
        &person.token,
        "PL-1",
        json!({"title": "  Renamed ", "body": "", "labels": ["ui", " ui", "bug"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{edited}");
    assert_eq!(edited["seq"], 2);
    assert_eq!(edited["row"]["issue"]["title"], "Renamed");
    assert_eq!(edited["row"]["issue"]["labels"], json!(["ui", "bug"]));
    assert_eq!(edited["row"]["issue"]["assignee"], Value::Null);

    let (status, same) = patch(
        &app,
        &person.token,
        "PL-1",
        json!({"title": "Renamed", "labels": ["ui", "bug"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{same}");
    assert_eq!(same["seq"], 2);

    let (_, assigned) = patch(
        &app,
        &person.token,
        "PL-1",
        json!({"assignee": person.actor_id}),
    )
    .await;
    assert_eq!(
        assigned["row"]["issue"]["assignee"],
        json!({"id": person.actor_id, "name": "Gannon", "role": "person"})
    );
    let (_, cleared) = patch(&app, &person.token, "PL-1", json!({"assignee": null})).await;
    assert_eq!(cleared["seq"], 4);
    assert_eq!(cleared["row"]["issue"]["assignee"], Value::Null);
    assert_eq!(cleared["row"]["issue"]["labels"], json!(["ui", "bug"]));

    let fields = |f: &[&str]| Some(f.iter().map(|f| f.to_string()).collect());
    assert_eq!(
        events(&pool).await,
        [
            (1, "created".to_string(), None),
            (2, "edited".to_string(), fields(&["title", "labels"])),
            (3, "edited".to_string(), fields(&["assignee"])),
            (4, "edited".to_string(), fields(&["assignee"])),
        ]
    );
    let (_, timeline) = call(&app, &person.token, "GET", "/api/issues/PL-1/events", None).await;
    assert_eq!(timeline[0]["detail"], json!({"kind": "created"}));
    assert_eq!(
        timeline[1]["detail"],
        json!({"kind": "edited", "fields": ["title", "labels"]})
    );
}

#[sqlx::test]
async fn bad_edits_are_refused_and_write_nothing(pool: PgPool) {
    let (app, person) = setup(&pool).await;
    let other = workspace(&pool, "OT", "Ola").await;
    create(&app, &person.token, "First").await;
    let (status, runner) = call(
        &app,
        &person.token,
        "POST",
        "/api/runners",
        Some(json!({"name": "sartre", "agents": ["claude-code"]})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{runner}");
    let runner_actor: Uuid = sqlx::query_scalar("SELECT actor_id FROM runner")
        .fetch_one(&pool)
        .await
        .unwrap();

    let refused = [
        (json!({"title": "  "}), "an issue needs a title".to_string()),
        (
            json!({"assignee": runner_actor}),
            format!("{runner_actor} is not a person or an agent of this workspace"),
        ),
        (
            json!({"assignee": other.actor_id}),
            format!(
                "{} is not a person or an agent of this workspace",
                other.actor_id
            ),
        ),
    ];
    for (body, message) in refused {
        let (status, answer) = patch(&app, &person.token, "PL-1", body).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{answer}");
        assert_eq!(answer, json!({"code": "invalid", "message": message}));
    }

    let (status, agent) = call(
        &app,
        &person.token,
        "POST",
        "/api/actors",
        Some(json!({"kind": "codex"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{agent}");
    sqlx::query("UPDATE issue SET status = 'human_review' WHERE id = 'PL-1'")
        .execute(&pool)
        .await
        .unwrap();
    let agent_token = agent["token"].as_str().unwrap();
    let (status, answer) = patch(&app, agent_token, "PL-1", json!({"title": "Mine"})).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(
        answer,
        json!({"code": "locked", "message": "human_review is locked: an agent cannot act on an issue in it"})
    );
    assert_eq!(events(&pool).await, [(1, "created".to_string(), None)]);
}

async fn finish_failed(app: &Router, runner: &str, claim: &Value, reason: &str) {
    let (status, body) = call(
        app,
        runner,
        "POST",
        &format!("/api/runs/{}/finish", claim["run_id"].as_str().unwrap()),
        Some(json!({
            "attempt": claim["attempt"], "outcome": "failed", "reason": reason, "duration_ms": 10
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[sqlx::test]
async fn each_row_shows_its_run_idle_running_queued_or_failed(pool: PgPool) {
    let (app, person) = setup(&pool).await;
    create(&app, &person.token, "First").await;
    let run_of = |list: &Value| list["issues"][0]["run"].clone();
    assert_eq!(
        run_of(&list(&app, &person.token).await),
        json!({"state": "idle"})
    );

    let (_, runner) = call(
        &app,
        &person.token,
        "POST",
        "/api/runners",
        Some(json!({"name": "sartre", "agents": ["claude-code"]})),
    )
    .await;
    let runner = runner["token"].as_str().unwrap().to_string();
    for to in ["todo", "start"] {
        let path = "/api/issues/PL-1/transitions";
        let (status, _) = call(&app, &person.token, "POST", path, Some(json!({"to": to}))).await;
        assert_eq!(status, StatusCode::OK);
    }
    let claim = || call(&app, &runner, "POST", "/api/runs/claim", Some(json!({})));

    let (status, first) = claim().await;
    assert_eq!(status, StatusCode::OK, "{first}");
    let started: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT started_at FROM run WHERE outcome = 'running'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        run_of(&list(&app, &person.token).await),
        json!({"state": "running", "agent": "claude-code", "since": started})
    );

    finish_failed(&app, &runner, &first, "boom 1").await;
    assert_eq!(
        run_of(&list(&app, &person.token).await),
        json!({"state": "queued", "agent": "claude-code"})
    );

    for n in [2, 3] {
        let (status, next) = claim().await;
        assert_eq!(status, StatusCode::OK, "{next}");
        finish_failed(&app, &runner, &next, &format!("boom {n}")).await;
    }
    let finished: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT max(finished_at) FROM run")
            .fetch_one(&pool)
            .await
            .unwrap();
    let rows = list(&app, &person.token).await;
    assert_eq!(
        run_of(&rows),
        json!({"state": "failed", "agent": "claude-code", "reason": "boom 3", "at": finished})
    );
    assert_eq!(rows["issues"][0]["issue"]["status"], "in_progress");
    assert_eq!(rows["issues"][0]["pull_request"], Value::Null);
    assert_eq!(
        rows["issues"][0]["gates"],
        json!({"to": "agent_review", "to_name": "Agent Review", "passed": 0, "total": 3,
               "first_failed": "pr_open"})
    );
}

#[sqlx::test]
async fn a_run_ending_and_the_claim_of_the_next_dispatch_reach_the_open_stream(pool: PgPool) {
    let (app, person) = setup(&pool).await;
    create(&app, &person.token, "First").await;
    let (_, runner) = call(
        &app,
        &person.token,
        "POST",
        "/api/runners",
        Some(json!({"name": "sartre", "agents": ["claude-code"]})),
    )
    .await;
    let runner = runner["token"].as_str().unwrap().to_string();
    for to in ["todo", "start"] {
        let path = "/api/issues/PL-1/transitions";
        let (status, _) = call(&app, &person.token, "POST", path, Some(json!({"to": to}))).await;
        assert_eq!(status, StatusCode::OK);
    }
    let claim = || call(&app, &runner, "POST", "/api/runs/claim", Some(json!({})));
    let (_, first) = claim().await;
    // Created, two moves, and the claim's move to In Progress and its `run`.
    let snapshot = list(&app, &person.token).await;
    assert_eq!(snapshot["last_event_seq"], 5);
    let mut body = open_stream(&app, &person, 5, None).await;

    finish_failed(&app, &runner, &first, "boom").await;
    assert_eq!(
        seen(&read_stream(&mut body, 1).await, "run"),
        [(
            6,
            json!("run"),
            json!("sartre"),
            json!({"state": "queued", "agent": "claude-code"})
        )]
    );
    let (status, second) = claim().await;
    assert_eq!(status, StatusCode::OK, "{second}");
    let started: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT started_at FROM run WHERE outcome = 'running'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        seen(&read_stream(&mut body, 1).await, "run"),
        [(
            7,
            json!("run"),
            json!("sartre"),
            json!({"state": "running", "agent": "claude-code", "since": started})
        )]
    );
    // The second claim moved nothing, and the timeline shows no run.
    let (_, timeline) = call(&app, &person.token, "GET", "/api/issues/PL-1/events", None).await;
    let kinds: Vec<&str> = (timeline.as_array().unwrap().iter())
        .map(|event| event["detail"]["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["created", "transition", "transition", "transition"]);
}

#[sqlx::test]
async fn a_pull_request_opening_turning_ready_and_its_checks_reach_the_open_stream(pool: PgPool) {
    let person = workspace(&pool, "PL", "Gannon").await;
    let state = AppState::new(pool.clone()).with_github_webhook_secret(SECRET);
    let app = router(state.clone());
    create(&app, &person.token, "Fix the thing").await;
    sqlx::query("UPDATE issue SET status = 'in_progress' WHERE id = 'PL-1'")
        .execute(&pool)
        .await
        .unwrap();
    let mut body = open_stream(&app, &person, 1, None).await;

    let url = "https://github.com/gannonh/punchlist/pull/12";
    let gates = |passed: u32, first_failed: Option<&str>| {
        json!({"to": "agent_review", "to_name": "Agent Review", "passed": passed, "total": 3,
               "first_failed": first_failed})
    };
    deliver(&state, "pull_request", "d-opened", OPENED, |_| {}).await;
    assert_eq!(write_pull_request_events(&pool).await.unwrap(), 1);
    let opened = read_stream(&mut body, 1).await;
    assert_eq!(
        seen(&opened, "pull_request"),
        [(
            2,
            json!("pull_request"),
            json!("GitHub"),
            json!({"number": 12, "state": "draft", "checks": "none", "url": url})
        )]
    );
    assert_eq!(opened[0].1["row"]["gates"], gates(2, Some("pr_ready")));

    // Recorded before one sweep, the ready pull request and its check are one event.
    deliver(&state, "pull_request", "d-ready", READY, |_| {}).await;
    deliver(&state, "check_run", "d-check", CHECK_RUN, |_| {}).await;
    assert_eq!(write_pull_request_events(&pool).await.unwrap(), 1);
    assert_eq!(write_pull_request_events(&pool).await.unwrap(), 0);
    let ready = read_stream(&mut body, 1).await;
    assert_eq!(
        seen(&ready, "pull_request"),
        [(
            3,
            json!("pull_request"),
            json!("GitHub"),
            json!({"number": 12, "state": "open", "checks": "passing", "url": url})
        )]
    );
    assert_eq!(ready[0].1["row"]["gates"], gates(3, None));
    let marks: i64 = sqlx::query_scalar("SELECT count(*) FROM pull_request_change")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(marks, 0);

    // A browser that reconnects after the snapshot gets both again, each with the row as it is.
    let mut replay = open_stream(&app, &person, 0, Some(1)).await;
    let replayed = read_stream(&mut replay, 2).await;
    assert_eq!(
        seen(&replayed, "pull_request")
            .iter()
            .map(|(id, _, _, pr)| (*id, pr["state"].clone()))
            .collect::<Vec<_>>(),
        [(2, json!("open")), (3, json!("open"))]
    );
    let (_, timeline) = call(&app, &person.token, "GET", "/api/issues/PL-1/events", None).await;
    assert_eq!(timeline.as_array().unwrap().len(), 1);
}

/// How many `pull_request` events the log holds, after a sweep.
async fn pull_request_events(pool: &PgPool) -> usize {
    write_pull_request_events(pool).await.unwrap();
    events(pool)
        .await
        .iter()
        .filter(|(_, kind, _)| kind == "pull_request")
        .count()
}

#[sqlx::test]
async fn the_same_pull_request_delivered_twice_writes_one_event(pool: PgPool) {
    let person = workspace(&pool, "PL", "Gannon").await;
    let state = AppState::new(pool.clone()).with_github_webhook_secret(SECRET);
    create(&router(state.clone()), &person.token, "Fix the thing").await;

    deliver(&state, "pull_request", "d-1", OPENED, |_| {}).await;
    assert_eq!(pull_request_events(&pool).await, 1);
    deliver(&state, "pull_request", "d-2", OPENED, |_| {}).await;
    assert_eq!(pull_request_events(&pool).await, 1);
    deliver(&state, "pull_request", "d-3", READY, |_| {}).await;
    assert_eq!(pull_request_events(&pool).await, 2);
}

#[sqlx::test]
async fn the_same_check_run_delivered_twice_writes_one_event(pool: PgPool) {
    let person = workspace(&pool, "PL", "Gannon").await;
    let state = AppState::new(pool.clone()).with_github_webhook_secret(SECRET);
    create(&router(state.clone()), &person.token, "Fix the thing").await;
    deliver(&state, "pull_request", "d-pr", OPENED, |_| {}).await;
    assert_eq!(pull_request_events(&pool).await, 1);

    deliver(&state, "check_run", "d-1", CHECK_RUN, |_| {}).await;
    assert_eq!(pull_request_events(&pool).await, 2);
    deliver(&state, "check_run", "d-2", CHECK_RUN, |_| {}).await;
    assert_eq!(pull_request_events(&pool).await, 2);
    // A conclusion that changes the row's checks is one more.
    let failed = |payload: &mut Value| payload["check_run"]["conclusion"] = json!("failure");
    deliver(&state, "check_run", "d-3", CHECK_RUN, failed).await;
    assert_eq!(pull_request_events(&pool).await, 3);
}

#[sqlx::test]
async fn a_pull_request_that_comes_to_name_another_issue_updates_both_rows(pool: PgPool) {
    let person = workspace(&pool, "PL", "Gannon").await;
    let state = AppState::new(pool.clone()).with_github_webhook_secret(SECRET);
    let app = router(state.clone());
    create(&app, &person.token, "First").await;
    create(&app, &person.token, "Second").await;
    let named = |title: &'static str, at: &'static str| {
        move |payload: &mut Value| {
            payload["pull_request"]["head"]["ref"] = json!("stray");
            payload["pull_request"]["title"] = json!(title);
            payload["pull_request"]["updated_at"] = json!(at);
        }
    };
    deliver(
        &state,
        "pull_request",
        "d-1",
        OPENED,
        named("Fix (PL-1)", "2026-10-09T18:00:00Z"),
    )
    .await;
    write_pull_request_events(&pool).await.unwrap();
    let mut body = open_stream(&app, &person, 3, None).await;

    deliver(
        &state,
        "pull_request",
        "d-2",
        OPENED,
        named("Fix (PL-2)", "2026-10-09T18:01:00Z"),
    )
    .await;
    assert_eq!(write_pull_request_events(&pool).await.unwrap(), 2);
    let messages = read_stream(&mut body, 2).await;
    let rows: Vec<(Value, Value)> = (messages.iter())
        .map(|(_, item)| {
            (
                item["row"]["issue"]["id"].clone(),
                item["row"]["pull_request"]["number"].clone(),
            )
        })
        .collect();
    assert_eq!(
        rows,
        [(json!("PL-1"), Value::Null), (json!("PL-2"), json!(12))]
    );
}

#[sqlx::test]
async fn a_person_signs_in_and_the_cookie_alone_authenticates(pool: PgPool) {
    let (app, person) = setup(&pool).await;
    let sign_in = |token: &str, https: bool| {
        let mut request = Request::builder()
            .method("POST")
            .uri("/api/session")
            .header("content-type", "application/json");
        if https {
            request = request.header("x-forwarded-proto", "https");
        }
        request
            .body(Body::from(json!({"token": token}).to_string()))
            .unwrap()
    };

    let (status, headers, session) = send(&app, sign_in(&person.token, false)).await;
    assert_eq!(status, StatusCode::OK, "{session}");
    assert_eq!(
        headers["set-cookie"],
        format!(
            "punchlist_session={}; HttpOnly; SameSite=Strict; Path=/",
            person.token
        )
    );
    let expected = json!({
        "actor": {"id": person.actor_id, "name": "Gannon", "role": "person"},
        "workspace_id": person.workspace_id,
        "account": {"id": person.account_id, "name": "Gannon"},
    });
    assert_eq!(session, expected);
    let (_, headers, _) = send(&app, sign_in(&person.token, true)).await;
    assert!(
        (headers["set-cookie"].to_str().unwrap()).ends_with("; Path=/; Secure"),
        "{headers:?}"
    );

    let with_cookie = |uri: &str, token: &str| {
        Request::builder()
            .uri(uri)
            .header("cookie", format!("theme=dark; punchlist_session={token}"))
            .body(Body::empty())
            .unwrap()
    };
    let (status, _, issues) = send(&app, with_cookie("/api/issues", &person.token)).await;
    assert_eq!(status, StatusCode::OK, "{issues}");
    assert_eq!(issues, json!({"issues": [], "last_event_seq": 0}));
    let (status, _, me) = send(&app, with_cookie("/api/session", &person.token)).await;
    assert_eq!((status, me), (StatusCode::OK, expected));

    let (_, runner) = call(
        &app,
        &person.token,
        "POST",
        "/api/runners",
        Some(json!({"name": "sartre", "agents": ["claude-code"]})),
    )
    .await;
    let runner = runner["token"].as_str().unwrap();
    let (status, _, body) = send(&app, sign_in(runner, false)).await;
    assert_eq!(
        (status, &body["code"]),
        (StatusCode::UNAUTHORIZED, &json!("unauthorized"))
    );
    let (status, _, _) = send(&app, with_cookie("/api/issues", runner)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _, _) = send(&app, with_cookie("/api/session", "plt_unknown")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let sign_out = Request::builder()
        .method("DELETE")
        .uri("/api/session")
        .body(Body::empty())
        .unwrap();
    let (status, headers, _) = send(&app, sign_out).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        headers["set-cookie"],
        "punchlist_session=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0"
    );
}

#[sqlx::test]
async fn the_web_app_is_served_beside_the_api(pool: PgPool) {
    let (app, person) = setup(&pool).await;
    let dir = tempfile::tempdir().unwrap();
    let get = |app: Router, uri: &str| {
        let request = request(&person.token, "GET", uri, None);
        async move { send(&app, request).await }
    };
    // Without an `index.html` the server is the API alone.
    let api_only = with_web_app(app.clone(), dir.path());
    assert_eq!(get(api_only, "/").await.0, StatusCode::NOT_FOUND);

    std::fs::write(dir.path().join("index.html"), "<p>app</p>").unwrap();
    std::fs::create_dir(dir.path().join("assets")).unwrap();
    std::fs::write(dir.path().join("assets/app.js"), "go()").unwrap();
    let served = with_web_app(app, dir.path());
    for uri in ["/", "/board", "/issues/PL-1"] {
        let (status, _, body) = get(served.clone(), uri).await;
        assert_eq!(
            (status, body),
            (StatusCode::OK, json!("<p>app</p>")),
            "{uri}"
        );
    }
    let (status, headers, body) = get(served.clone(), "/assets/app.js").await;
    assert_eq!((status, body), (StatusCode::OK, json!("go()")));
    assert_eq!(headers["content-type"], "text/javascript");
    assert_eq!(
        get(served.clone(), "/api/nothing").await.0,
        StatusCode::NOT_FOUND
    );
    let (status, _, issues) = get(served, "/api/issues").await;
    assert_eq!(
        (status, issues["last_event_seq"].clone()),
        (StatusCode::OK, json!(0))
    );
}

#[sqlx::test]
async fn each_status_names_its_dispatch_and_whether_it_is_closed(pool: PgPool) {
    let (app, person) = setup(&pool).await;
    let (status, body) = call(&app, &person.token, "GET", "/api/workflow", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let statuses: Vec<_> = (body["statuses"].as_array().unwrap().iter())
        .map(|s| {
            (
                s["key"].as_str().unwrap(),
                s["dispatch"].clone(),
                s["closed"].clone(),
            )
        })
        .collect();
    assert_eq!(
        statuses,
        [
            ("backlog", Value::Null, json!(false)),
            ("todo", Value::Null, json!(false)),
            ("start", Value::Null, json!(false)),
            ("in_progress", json!("claude-code"), json!(false)),
            ("agent_review", json!("codex"), json!(false)),
            ("human_review", Value::Null, json!(false)),
            ("merging", Value::Null, json!(false)),
            ("done", Value::Null, json!(true)),
            ("canceled", Value::Null, json!(true)),
        ]
    );
}

/// Creates a project, or with `project` a milestone in it, and returns its id.
async fn create_placement(app: &Router, token: &str, project: Option<&str>, body: Value) -> String {
    let uri = match project {
        Some(id) => format!("/api/projects/{id}/milestones"),
        None => "/api/projects".to_string(),
    };
    let (status, created) = call(app, token, "POST", &uri, Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    created["id"].as_str().unwrap().to_string()
}

#[sqlx::test]
async fn projects_and_milestones_are_created_and_listed(pool: PgPool) {
    let (app, person) = setup(&pool).await;
    let other = workspace(&pool, "OT", "Ola").await;
    let punchlist =
        create_placement(&app, &person.token, None, json!({"name": " Punchlist "})).await;
    let groundwork =
        create_placement(&app, &person.token, None, json!({"name": "Groundwork"})).await;
    let gate0 = create_placement(
        &app,
        &person.token,
        Some(&punchlist),
        json!({"name": "Gate 0", "pass_when": "pl creates and moves an issue "}),
    )
    .await;
    let gate1 = create_placement(
        &app,
        &person.token,
        Some(&punchlist),
        json!({"name": "Gate 1"}),
    )
    .await;
    create_placement(&app, &other.token, None, json!({"name": "Theirs"})).await;

    let (status, list) = call(&app, &person.token, "GET", "/api/projects", None).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(
        list,
        json!({"projects": [
            {"id": punchlist, "name": "Punchlist", "milestones": [
                {"id": gate0, "name": "Gate 0", "pass_when": "pl creates and moves an issue"},
                {"id": gate1, "name": "Gate 1", "pass_when": ""},
            ]},
            {"id": groundwork, "name": "Groundwork", "milestones": []},
        ]})
    );

    let refused = [
        (
            "/api/projects".to_string(),
            json!({"name": "Punchlist"}),
            "a project named Punchlist already exists".to_string(),
        ),
        (
            "/api/projects".to_string(),
            json!({"name": " "}),
            "a project needs a name".to_string(),
        ),
        (
            "/api/projects".to_string(),
            json!({"name": "x".repeat(101)}),
            "a project name is at most 100 characters".to_string(),
        ),
        (
            format!("/api/projects/{punchlist}/milestones"),
            json!({"name": "Gate 0"}),
            "project Punchlist already has a milestone named Gate 0".to_string(),
        ),
    ];
    for (uri, body, message) in refused {
        let (status, answer) = call(&app, &person.token, "POST", &uri, Some(body)).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{answer}");
        assert_eq!(answer, json!({"code": "invalid", "message": message}));
    }
    // A name one project's milestone has is free in another project.
    create_placement(
        &app,
        &person.token,
        Some(&groundwork),
        json!({"name": "Gate 0"}),
    )
    .await;
    let (status, answer) = call(
        &app,
        &other.token,
        "POST",
        &format!("/api/projects/{punchlist}/milestones"),
        Some(json!({"name": "Mine"})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{answer}");
    assert_eq!(
        answer,
        json!({"code": "not_found", "message": format!("project {punchlist} not found")})
    );
}

#[sqlx::test]
async fn an_edit_sets_and_clears_the_parent_project_and_milestone(pool: PgPool) {
    let (app, person) = setup(&pool).await;
    create(&app, &person.token, "Epic").await;
    create(&app, &person.token, "Slice").await;
    let punchlist = create_placement(&app, &person.token, None, json!({"name": "Punchlist"})).await;
    let groundwork =
        create_placement(&app, &person.token, None, json!({"name": "Groundwork"})).await;
    let gate0 = create_placement(
        &app,
        &person.token,
        Some(&punchlist),
        json!({"name": "Gate 0"}),
    )
    .await;

    let (status, placed) = patch(
        &app,
        &person.token,
        "PL-2",
        json!({"parent": " PL-1 ", "project": punchlist, "milestone": gate0}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{placed}");
    assert_eq!(placed["seq"], 3);
    let issue = &placed["row"]["issue"];
    assert_eq!(issue["parent"], "PL-1");
    assert_eq!(
        issue["project"],
        json!({"id": punchlist, "name": "Punchlist"})
    );
    assert_eq!(issue["milestone"], json!({"id": gate0, "name": "Gate 0"}));
    let (_, shown) = call(&app, &person.token, "GET", "/api/issues/PL-2", None).await;
    assert_eq!(
        (
            &shown["parent"],
            &shown["project"]["name"],
            &shown["milestone"]["name"]
        ),
        (&json!("PL-1"), &json!("Punchlist"), &json!("Gate 0"))
    );

    // Another project leaves no milestone, since a milestone is in its project.
    let (_, moved) = patch(&app, &person.token, "PL-2", json!({"project": groundwork})).await;
    assert_eq!(moved["row"]["issue"]["project"]["name"], "Groundwork");
    assert_eq!(moved["row"]["issue"]["milestone"], Value::Null);
    let (_, cleared) = patch(
        &app,
        &person.token,
        "PL-2",
        json!({"parent": null, "project": null, "milestone": null}),
    )
    .await;
    assert_eq!(cleared["seq"], 5);
    let issue = &cleared["row"]["issue"];
    assert_eq!(
        (&issue["parent"], &issue["project"], &issue["milestone"]),
        (&Value::Null, &Value::Null, &Value::Null)
    );
    let (_, same) = patch(&app, &person.token, "PL-2", json!({"parent": null})).await;
    assert_eq!(same["seq"], 5);

    let stored: (Option<String>, Option<Uuid>, Option<Uuid>) =
        sqlx::query_as("SELECT parent_id, project_id, milestone_id FROM issue WHERE id = 'PL-2'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(stored, (None, None, None));
    let fields = |f: &[&str]| Some(f.iter().map(|f| f.to_string()).collect());
    assert_eq!(
        events(&pool).await[2..],
        [
            (
                3,
                "edited".to_string(),
                fields(&["parent", "project", "milestone"])
            ),
            (4, "edited".to_string(), fields(&["project", "milestone"])),
            (5, "edited".to_string(), fields(&["parent", "project"])),
        ]
    );
}

#[sqlx::test]
async fn a_parent_cycle_and_anything_outside_the_workspace_or_project_are_refused(pool: PgPool) {
    let (app, person) = setup(&pool).await;
    let other = workspace(&pool, "OT", "Ola").await;
    for title in ["Top", "Middle", "Bottom"] {
        create(&app, &person.token, title).await;
    }
    for (child, parent) in [("PL-2", "PL-1"), ("PL-3", "PL-2")] {
        let (status, body) = patch(&app, &person.token, child, json!({"parent": parent})).await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let punchlist = create_placement(&app, &person.token, None, json!({"name": "Punchlist"})).await;
    let groundwork =
        create_placement(&app, &person.token, None, json!({"name": "Groundwork"})).await;
    let gate0 = create_placement(
        &app,
        &person.token,
        Some(&punchlist),
        json!({"name": "Gate 0"}),
    )
    .await;
    create(&app, &other.token, "Theirs").await;
    let theirs = create_placement(&app, &other.token, None, json!({"name": "Theirs"})).await;
    let their_gate =
        create_placement(&app, &other.token, Some(&theirs), json!({"name": "Gate 0"})).await;
    let before = events(&pool).await.len();

    let refused = [
        (
            json!({"parent": "PL-3"}),
            "PL-3 is under PL-1 (PL-1 → PL-2 → PL-3), so it cannot be PL-1's parent".to_string(),
        ),
        (
            json!({"parent": "PL-1"}),
            "PL-1 cannot be its own parent".to_string(),
        ),
        (
            json!({"parent": "OT-1"}),
            "no issue OT-1 in this workspace".to_string(),
        ),
        (
            json!({"parent": " "}),
            "a parent is an issue id such as PL-1, or null for none".to_string(),
        ),
        (
            json!({"project": theirs}),
            format!("no project {theirs} in this workspace"),
        ),
        (
            json!({"project": punchlist, "milestone": their_gate}),
            format!("no milestone {their_gate} in this workspace"),
        ),
        (
            json!({"milestone": gate0}),
            "milestone Gate 0 is in project Punchlist; give PL-1 that project to set it"
                .to_string(),
        ),
        (
            json!({"project": groundwork, "milestone": gate0}),
            "milestone Gate 0 is in project Punchlist; give PL-1 that project to set it"
                .to_string(),
        ),
    ];
    for (body, message) in refused {
        let (status, answer) = patch(&app, &person.token, "PL-1", body).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{answer}");
        assert_eq!(answer, json!({"code": "invalid", "message": message}));
    }
    let top: (Option<String>, Option<Uuid>) =
        sqlx::query_as("SELECT parent_id, project_id FROM issue WHERE id = 'PL-1'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(top, (None, None));
    assert_eq!(events(&pool).await.len(), before);
}
