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
