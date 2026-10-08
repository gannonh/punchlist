//! The transition path end to end: HTTP requests against the router, then the rows they
//! wrote, read back from Postgres.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use punchlist_server::{AppState, Bootstrap, Bootstrapped, bootstrap, router};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

async fn setup(pool: &PgPool) -> (Router, Bootstrapped) {
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
    (router(AppState::new(pool.clone())), person)
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

#[sqlx::test]
async fn create_then_move_twice_records_transitions_and_events(pool: PgPool) {
    let (app, person) = setup(&pool).await;

    let (status, issue) = call(
        &app,
        &person.token,
        "POST",
        "/api/issues",
        Some(json!({"title": "First", "body": "Body"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(issue["id"], "PL-1");
    assert_eq!(issue["status"], "backlog");
    assert_eq!(issue["status_name"], "Backlog");
    assert_eq!(issue["body"], "Body");

    let mut returned = Vec::new();
    for to in ["todo", "start"] {
        let (status, moved) = call(
            &app,
            &person.token,
            "POST",
            "/api/issues/PL-1/transitions",
            Some(json!({"to": to})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{moved}");
        assert_eq!(moved["issue"]["status"], to);
        returned.push(moved["event"].clone());
    }
    let event = &returned[1];
    assert_eq!(event["seq"], 2);
    assert_eq!(event["issue_id"], "PL-1");
    assert_eq!(
        event["actor"],
        json!({"id": person.actor_id, "name": "Gannon", "role": "person"})
    );
    assert_eq!(
        event["detail"],
        json!({
            "kind": "transition",
            "from": "todo",
            "from_name": "Todo",
            "to": "start",
            "to_name": "Start",
            "workflow_version": punchlist_core::default_workflow().version(),
        })
    );

    let status = sqlx::query_scalar!("SELECT status FROM issue WHERE id = 'PL-1'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "start");

    let transitions = sqlx::query!(
        "SELECT from_status, to_status, actor_id, workflow_version FROM transition
         WHERE issue_id = 'PL-1' ORDER BY created_at"
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let moves: Vec<(&str, &str)> = transitions
        .iter()
        .map(|t| (t.from_status.as_str(), t.to_status.as_str()))
        .collect();
    assert_eq!(moves, [("backlog", "todo"), ("todo", "start")]);
    assert!(transitions.iter().all(|t| t.actor_id == person.actor_id));
    assert!(
        transitions
            .iter()
            .all(|t| t.workflow_version == punchlist_core::default_workflow().version())
    );

    let events =
        sqlx::query!("SELECT seq, kind, actor_id FROM event WHERE issue_id = 'PL-1' ORDER BY seq")
            .fetch_all(&pool)
            .await
            .unwrap();
    let events: Vec<(i64, &str, uuid::Uuid)> = events
        .iter()
        .map(|e| (e.seq, e.kind.as_str(), e.actor_id))
        .collect();
    assert_eq!(
        events,
        [
            (1, "transition", person.actor_id),
            (2, "transition", person.actor_id)
        ]
    );

    let (status, timeline) =
        call(&app, &person.token, "GET", "/api/issues/PL-1/events", None).await;
    assert_eq!(status, StatusCode::OK);
    // The events `move` returned are the ones the timeline reads back.
    assert_eq!(timeline.as_array().unwrap(), &returned);
    let timeline: Vec<(&str, &str, &str, &str)> = timeline
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["detail"]["from"].as_str().unwrap(),
                e["detail"]["to"].as_str().unwrap(),
                e["actor"]["name"].as_str().unwrap(),
                e["actor"]["role"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        timeline,
        [
            ("backlog", "todo", "Gannon", "person"),
            ("todo", "start", "Gannon", "person")
        ]
    );

    let (status, list) = call(&app, &person.token, "GET", "/api/issues", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["last_event_seq"], 2);
    assert_eq!(list["issues"][0]["id"], "PL-1");
}

#[sqlx::test]
async fn refused_moves_name_the_rule_and_write_nothing(pool: PgPool) {
    let (app, person) = setup(&pool).await;
    for title in ["First", "Second"] {
        call(
            &app,
            &person.token,
            "POST",
            "/api/issues",
            Some(json!({"title": title})),
        )
        .await;
    }
    for to in ["todo", "start"] {
        call(
            &app,
            &person.token,
            "POST",
            "/api/issues/PL-1/transitions",
            Some(json!({"to": to})),
        )
        .await;
    }

    let (status, body) = call(
        &app,
        &person.token,
        "POST",
        "/api/issues/PL-1/transitions",
        Some(json!({"to": "in_progress"})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(
        body,
        json!({"code": "role_not_allowed", "message": "only a runner makes start → in_progress, not a person"})
    );

    let (status, body) = call(
        &app,
        &person.token,
        "POST",
        "/api/issues/PL-2/transitions",
        Some(json!({"to": "done"})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(
        body,
        json!({"code": "no_transition", "message": "no transition backlog → done exists in the workflow"})
    );

    let counts = sqlx::query!(
        r#"SELECT (SELECT count(*) FROM transition) AS "transitions!",
                  (SELECT count(*) FROM event) AS "events!",
                  (SELECT status FROM issue WHERE id = 'PL-1') AS "pl1!",
                  (SELECT status FROM issue WHERE id = 'PL-2') AS "pl2!""#
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        (
            counts.transitions,
            counts.events,
            counts.pl1.as_str(),
            counts.pl2.as_str()
        ),
        (2, 2, "start", "backlog")
    );
}

#[sqlx::test]
async fn requests_need_a_known_token(pool: PgPool) {
    let (app, _) = setup(&pool).await;
    let (status, body) = call(&app, "plt_wrong", "GET", "/api/issues", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], "unauthorized");
}

#[sqlx::test]
async fn unknown_issue_is_not_found(pool: PgPool) {
    let (app, person) = setup(&pool).await;
    let (status, body) = call(&app, &person.token, "GET", "/api/issues/PL-9", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        body,
        json!({"code": "not_found", "message": "issue PL-9 not found"})
    );
}

#[sqlx::test]
async fn openapi_document_lists_the_routes(pool: PgPool) {
    let (app, person) = setup(&pool).await;
    let (status, doc) = call(&app, &person.token, "GET", "/api/openapi.json", None).await;
    assert_eq!(status, StatusCode::OK);
    let mut paths: Vec<&str> = doc["paths"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    paths.sort();
    assert_eq!(
        paths,
        [
            "/api/issues",
            "/api/issues/{id}",
            "/api/issues/{id}/events",
            "/api/issues/{id}/runs",
            "/api/issues/{id}/transitions",
            "/api/runners",
            "/api/runners/{id}/heartbeat",
            "/api/runs/claim",
            "/api/runs/{id}/finish",
            "/api/runs/{id}/log"
        ]
    );
}
