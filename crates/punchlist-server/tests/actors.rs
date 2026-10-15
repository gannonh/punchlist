//! Agent actors people mint, their reach, locks and workspaces: HTTP requests against the
//! router, then the rows they wrote, read back from Postgres.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use punchlist_server::{AppState, Bootstrap, Bootstrapped, bootstrap, router};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

async fn workspace(pool: &PgPool, prefix: &str, person: &str) -> Bootstrapped {
    bootstrap(
        pool,
        &Bootstrap {
            workspace_name: prefix.into(),
            issue_prefix: prefix.into(),
            repository_owner: "gannonh".into(),
            repository_name: prefix.to_lowercase(),
            person_name: person.into(),
        },
    )
    .await
    .unwrap()
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

async fn create_agent(app: &Router, person: &str, kind: &str) -> (String, String) {
    let (status, body) = call(
        app,
        person,
        "POST",
        "/api/actors",
        Some(json!({"kind": kind})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    (
        body["actor"]["id"].as_str().unwrap().to_string(),
        body["token"].as_str().unwrap().to_string(),
    )
}

/// Creates an issue and moves it through `to` as the person; returns its id.
async fn issue_in(app: &Router, person: &str, to: &[&str]) -> String {
    let (status, issue) = call(
        app,
        person,
        "POST",
        "/api/issues",
        Some(json!({"title": "Fix it", "body": ""})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{issue}");
    let id = issue["id"].as_str().unwrap().to_string();
    for to in to {
        let (status, body) = call(
            app,
            person,
            "POST",
            &format!("/api/issues/{id}/transitions"),
            Some(json!({"to": to})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    id
}

async fn comment(app: &Router, token: &str, id: &str) -> (StatusCode, Value) {
    call(
        app,
        token,
        "POST",
        &format!("/api/issues/{id}/comments"),
        Some(json!({"body": "On it"})),
    )
    .await
}

#[sqlx::test]
async fn a_person_mints_an_agent_that_comments_as_itself(pool: PgPool) {
    let person = workspace(&pool, "PL", "Gannon").await;
    let app = router(AppState::new(pool.clone()));
    let (status, created) = call(
        &app,
        &person.token,
        "POST",
        "/api/actors",
        Some(json!({"kind": "codex", "model": "gpt-6-luna"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["actor"]["name"], "Codex");
    assert_eq!(created["actor"]["role"], "agent");
    assert_eq!(created["actor"]["kind"], "codex");
    assert_eq!(created["actor"]["model"], "gpt-6-luna");
    assert_eq!(created["actor"]["owner"], "Gannon");
    let agent_id = created["actor"]["id"].as_str().unwrap();
    let token = created["token"].as_str().unwrap();
    let stored = sqlx::query!(
        "SELECT token_hash, run_id, owner_id FROM actor WHERE id = $1::text::uuid",
        agent_id,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(stored.token_hash, Some(punchlist_server::hash_token(token)));
    assert_eq!(stored.run_id, None);
    assert_eq!(stored.owner_id, Some(person.actor_id));

    let id = issue_in(&app, &person.token, &["todo"]).await;
    let (status, event) = comment(&app, token, &id).await;
    assert_eq!(status, StatusCode::CREATED, "{event}");
    assert_eq!(
        event["actor"],
        json!({"id": agent_id, "name": "Codex", "role": "agent", "kind": "codex"})
    );
    // The timeline names the agent's kind; a person has none.
    let (_, events) = call(
        &app,
        &person.token,
        "GET",
        &format!("/api/issues/{id}/events"),
        None,
    )
    .await;
    let actors: Vec<&Value> = events
        .as_array()
        .unwrap()
        .iter()
        .map(|e| &e["actor"])
        .collect();
    assert_eq!(actors[0].get("kind"), None);
    assert_eq!(actors.last().unwrap()["kind"], "codex");

    // A standalone agent reaches issues only.
    let (status, _) = call(&app, token, "GET", "/api/issues", None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = call(&app, token, "GET", "/api/runners", None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, list) = call(&app, &person.token, "GET", "/api/actors", None).await;
    assert_eq!(status, StatusCode::OK);
    let rows: Vec<(String, String, Value)> = list["actors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| {
            (
                a["name"].as_str().unwrap().to_string(),
                a["role"].as_str().unwrap().to_string(),
                a["owner"].clone(),
            )
        })
        .collect();
    assert_eq!(
        rows,
        [
            ("Gannon".into(), "person".into(), Value::Null),
            ("Codex".into(), "agent".into(), json!("Gannon")),
        ]
    );
}

#[sqlx::test]
async fn only_a_person_mints_agents_of_known_kinds(pool: PgPool) {
    let person = workspace(&pool, "PL", "Gannon").await;
    let app = router(AppState::new(pool.clone()));
    let (status, body) = call(
        &app,
        &person.token,
        "POST",
        "/api/actors",
        Some(json!({"kind": "aider"})),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        body["message"],
        "unknown agent kind `aider`; the kinds are claude-code, codex, opencode, pi, cursor"
    );
    let (status, body) = call(
        &app,
        &person.token,
        "POST",
        "/api/actors",
        Some(json!({"kind": "codex", "owner": "Nobody"})),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        body["message"],
        "no person named `Nobody` in this workspace"
    );

    let (_, agent) = create_agent(&app, &person.token, "codex").await;
    let (status, runner) = call(
        &app,
        &person.token,
        "POST",
        "/api/runners",
        Some(json!({"name": "r1", "agents": ["codex"]})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let runner = runner["token"].as_str().unwrap();
    for token in [agent.as_str(), runner] {
        let (status, _) = call(
            &app,
            token,
            "POST",
            "/api/actors",
            Some(json!({"kind": "codex"})),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }
    let actors = sqlx::query_scalar!(r#"SELECT count(*) AS "n!" FROM actor WHERE role = 'agent'"#)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(actors, 1);
}

#[sqlx::test]
async fn no_token_reaches_another_workspaces_issue(pool: PgPool) {
    let a = workspace(&pool, "PL", "Gannon").await;
    let b = workspace(&pool, "OT", "Other").await;
    let app = router(AppState::new(pool.clone()));
    let (_, agent) = create_agent(&app, &a.token, "codex").await;
    let other = issue_in(&app, &b.token, &["todo"]).await;
    assert_eq!(other, "OT-1");

    let (status, body) = call(&app, &agent, "GET", "/api/issues/OT-1", None).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["message"], "issue OT-1 is in another workspace");
    let (status, body) = call(&app, &agent, "GET", "/api/issues/OT%2D1", None).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["message"], "issue OT-1 is in another workspace");
    let (status, _) = comment(&app, &agent, "OT-1").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // A person's token too.
    let (status, _) = comment(&app, &a.token, "OT-1").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let comments =
        sqlx::query_scalar!(r#"SELECT count(*) AS "n!" FROM comment WHERE issue_id = 'OT-1'"#)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(comments, 0);
    // An issue that does not exist anywhere is not found.
    let (status, _) = call(&app, &agent, "GET", "/api/issues/PL-9", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

async fn rows_for(pool: &PgPool, id: &str) -> (i64, i64) {
    let comments = sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!" FROM comment WHERE issue_id = $1"#,
        id
    )
    .fetch_one(pool)
    .await
    .unwrap();
    let transitions = sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!" FROM transition WHERE issue_id = $1"#,
        id
    )
    .fetch_one(pool)
    .await
    .unwrap();
    (comments, transitions)
}

#[sqlx::test]
async fn a_locked_status_refuses_the_agent_and_lets_the_person_act(pool: PgPool) {
    let person = workspace(&pool, "PL", "Gannon").await;
    let app = router(AppState::new(pool.clone()));
    let (_, agent) = create_agent(&app, &person.token, "codex").await;
    let id = issue_in(&app, &person.token, &["todo"]).await;
    // Test-only: the path to Human Review needs pull request evidence from GitHub.
    sqlx::query!("UPDATE issue SET status = 'human_review' WHERE id = $1", id)
        .execute(&pool)
        .await
        .unwrap();
    let before = rows_for(&pool, &id).await;

    let (status, body) = comment(&app, &agent, &id).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "locked");
    assert_eq!(
        body["message"],
        "human_review is locked: an agent cannot act on an issue in it"
    );
    let (status, body) = call(
        &app,
        &agent,
        "POST",
        &format!("/api/issues/{id}/transitions"),
        Some(json!({"to": "merging"})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "locked");
    assert_eq!(rows_for(&pool, &id).await, before);

    let (status, _) = comment(&app, &person.token, &id).await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, body) = call(
        &app,
        &person.token,
        "POST",
        &format!("/api/issues/{id}/transitions"),
        Some(json!({"to": "merging"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let moved = sqlx::query!(
        "SELECT a.role, t.from_status, t.to_status FROM transition t
         JOIN actor a ON a.id = t.actor_id
         WHERE t.issue_id = $1 ORDER BY t.created_at DESC LIMIT 1",
        id,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        (moved.role, moved.from_status, moved.to_status),
        ("person".into(), "human_review".into(), "merging".into())
    );
    assert_eq!(rows_for(&pool, &id).await, (before.0 + 1, before.1 + 1));
}

#[sqlx::test]
async fn a_locked_status_is_never_claimed(pool: PgPool) {
    let person = workspace(&pool, "PL", "Gannon").await;
    let app = router(AppState::new(pool.clone()));
    let id = issue_in(&app, &person.token, &["todo"]).await;
    sqlx::query!("UPDATE issue SET status = 'human_review' WHERE id = $1", id)
        .execute(&pool)
        .await
        .unwrap();
    // A dispatch waiting in the locked status, as an older workflow could have left one.
    sqlx::query!(
        "INSERT INTO dispatch (issue_id, transition_id, agent)
         SELECT issue_id, id, 'codex' FROM transition WHERE issue_id = $1",
        id,
    )
    .execute(&pool)
    .await
    .unwrap();
    let (status, runner) = call(
        &app,
        &person.token,
        "POST",
        "/api/runners",
        Some(json!({"name": "r1", "agents": ["codex", "claude-code"]})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, body) = call(
        &app,
        runner["token"].as_str().unwrap(),
        "POST",
        "/api/runs/claim",
        Some(json!({"wait_seconds": 0})),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let attempts = sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!" FROM attempt WHERE issue_id = $1"#,
        id
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(attempts, 0);
}

#[sqlx::test]
async fn a_runs_agent_is_its_dispatch_kind_owned_by_the_runners_person(pool: PgPool) {
    let person = workspace(&pool, "PL", "Gannon").await;
    let app = router(AppState::new(pool.clone()));
    let (_, runner) = call(
        &app,
        &person.token,
        "POST",
        "/api/runners",
        Some(json!({"name": "r1", "agents": ["claude-code"]})),
    )
    .await;
    issue_in(&app, &person.token, &["todo", "start"]).await;
    let (status, claim) = call(
        &app,
        runner["token"].as_str().unwrap(),
        "POST",
        "/api/runs/claim",
        Some(json!({"wait_seconds": 0})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{claim}");
    let agent = sqlx::query!(
        "SELECT name, kind, model, owner_id FROM actor WHERE run_id = $1::text::uuid",
        claim["run_id"].as_str().unwrap(),
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(agent.name, "Claude Code");
    assert_eq!(agent.kind.as_deref(), Some("claude-code"));
    assert_eq!(agent.model, None);
    assert_eq!(agent.owner_id, Some(person.actor_id));
}
