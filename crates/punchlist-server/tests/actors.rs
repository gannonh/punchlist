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
    let id = issue["row"]["issue"]["id"].as_str().unwrap().to_string();
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

/// The status of a request, without reading its body: an event stream's never ends.
async fn status_of(app: &Router, token: &str, uri: &str) -> StatusCode {
    let request = Request::builder()
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    app.clone().oneshot(request).await.unwrap().status()
}

async fn event_actors(pool: &PgPool, id: &str) -> Vec<(String, String, String)> {
    sqlx::query!(
        "SELECT e.kind, a.name, a.role FROM event e JOIN actor a ON a.id = e.actor_id
         WHERE e.issue_id = $1 ORDER BY e.seq",
        id,
    )
    .fetch_all(pool)
    .await
    .unwrap()
    .into_iter()
    .map(|row| (row.kind, row.name, row.role))
    .collect()
}

#[sqlx::test]
async fn a_standalone_agent_files_lists_and_watches_its_workspaces_issues(pool: PgPool) {
    let a = workspace(&pool, "PL", "Gannon").await;
    let b = workspace(&pool, "OT", "Other").await;
    let app = router(AppState::new(pool.clone()));
    let (status, created) = call(
        &app,
        &a.token,
        "POST",
        "/api/actors",
        Some(json!({"kind": "claude-code", "name": " Orchestrator "})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["actor"]["name"], "Orchestrator");
    let agent = created["token"].as_str().unwrap().to_string();
    let (status, project) = call(
        &app,
        &b.token,
        "POST",
        "/api/projects",
        Some(json!({"name": "Theirs"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{project}");
    issue_in(&app, &b.token, &[]).await;

    // It files an issue, edits it and comments on it as itself.
    let (status, filed) = call(
        &app,
        &agent,
        "POST",
        "/api/issues",
        Some(json!({"title": "Filed", "body": "Spec"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{filed}");
    assert_eq!(filed["row"]["issue"]["id"], "PL-1");
    let (status, body) = call(
        &app,
        &agent,
        "PATCH",
        "/api/issues/PL-1",
        Some(json!({"body": "Longer spec"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = comment(&app, &agent, "PL-1").await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let agent_row = |kind: &str| (kind.into(), "Orchestrator".into(), "agent".into());
    assert_eq!(
        event_actors(&pool, "PL-1").await,
        [
            agent_row("created"),
            agent_row("edited"),
            agent_row("comment")
        ]
    );
    let created_by = sqlx::query_scalar!(
        "SELECT a.name FROM issue i JOIN actor a ON a.id = i.created_by WHERE i.id = 'PL-1'"
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(created_by, "Orchestrator");

    // It reads its own workspace, never another's.
    let (status, list) = call(&app, &agent, "GET", "/api/issues", None).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let ids: Vec<&str> = (list["issues"].as_array().unwrap().iter())
        .map(|row| row["issue"]["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["PL-1"]);
    let (status, projects) = call(&app, &agent, "GET", "/api/projects", None).await;
    assert_eq!(
        (status, projects),
        (StatusCode::OK, json!({"projects": []}))
    );
    let (status, actors) = call(&app, &agent, "GET", "/api/actors", None).await;
    assert_eq!(status, StatusCode::OK, "{actors}");
    let names: Vec<&str> = (actors["actors"].as_array().unwrap().iter())
        .map(|actor| actor["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Gannon", "Orchestrator"]);
    let (status, workflow) = call(&app, &agent, "GET", "/api/workflow", None).await;
    assert_eq!(status, StatusCode::OK, "{workflow}");
    let own = format!("/api/workspaces/{}/events?after=0", a.workspace_id);
    assert_eq!(status_of(&app, &agent, &own).await, StatusCode::OK);
    let other = format!("/api/workspaces/{}/events?after=0", b.workspace_id);
    assert_eq!(status_of(&app, &agent, &other).await, StatusCode::NOT_FOUND);
    let (status, _) = call(&app, &agent, "GET", "/api/issues/OT-1", None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Everything else stays people's and runners'.
    let (_, runner) = call(
        &app,
        &a.token,
        "POST",
        "/api/runners",
        Some(json!({"name": "r1", "agents": ["claude-code"]})),
    )
    .await;
    let rows = || async {
        sqlx::query!(
            r#"SELECT (SELECT count(*) FROM actor) AS "actors!",
                      (SELECT count(*) FROM project) AS "projects!",
                      (SELECT count(*) FROM runner) AS "runners!""#
        )
        .fetch_one(&pool)
        .await
        .map(|r| (r.actors, r.projects, r.runners))
        .unwrap()
    };
    let before = rows().await;
    let project_id = project["id"].as_str().unwrap();
    for (method, uri, body) in [
        ("POST", "/api/actors".to_string(), json!({"kind": "codex"})),
        ("POST", "/api/projects".into(), json!({"name": "Mine"})),
        (
            "POST",
            format!("/api/projects/{project_id}/milestones"),
            json!({"name": "M"}),
        ),
        (
            "POST",
            "/api/runners".into(),
            json!({"name": "r2", "agents": ["codex"]}),
        ),
        ("GET", "/api/runners".into(), Value::Null),
        (
            "GET",
            format!("/api/workspaces/{}/imports", a.workspace_id),
            Value::Null,
        ),
        ("GET", "/api/pull-requests/unlinked".into(), Value::Null),
        ("POST", "/api/runs/claim".into(), json!({"wait_seconds": 0})),
    ] {
        let (status, answer) = call(&app, &agent, method, &uri, Some(body)).await;
        assert_eq!(
            (status, answer["message"].as_str()),
            (
                StatusCode::FORBIDDEN,
                Some("an agent acts only on the workspace's issues")
            ),
            "{method} {uri}"
        );
    }
    assert_eq!(rows().await, before);
    // The workflow's `by` rules keep a person's moves from it.
    let (status, body) = call(
        &app,
        &agent,
        "POST",
        "/api/issues/PL-1/transitions",
        Some(json!({"to": "todo"})),
    )
    .await;
    assert_eq!(
        (status, &body["code"]),
        (StatusCode::CONFLICT, &json!("role_not_allowed"))
    );
    let (_, transitions) = rows_for(&pool, "PL-1").await;
    assert_eq!(transitions, 0);

    // A run's agent still reaches only its run's issue.
    let id = issue_in(&app, &a.token, &["todo", "start"]).await;
    let (status, claim) = call(
        &app,
        runner["token"].as_str().unwrap(),
        "POST",
        "/api/runs/claim",
        Some(json!({"wait_seconds": 0})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{claim}");
    let run_agent = claim["agent_token"].as_str().unwrap();
    assert_eq!(id, "PL-2");
    let (status, _) = call(&app, run_agent, "GET", "/api/issues/PL-2", None).await;
    assert_eq!(status, StatusCode::OK);
    for uri in [
        "/api/issues/PL-1",
        "/api/issues",
        "/api/projects",
        "/api/session",
    ] {
        let (status, body) = call(&app, run_agent, "GET", uri, None).await;
        assert_eq!(
            (status, body["message"].as_str()),
            (
                StatusCode::FORBIDDEN,
                Some("an agent acts only on its run's issue, PL-2")
            ),
            "{uri}"
        );
    }
    let (status, _) = call(
        &app,
        run_agent,
        "POST",
        "/api/issues",
        Some(json!({"title": "Mine"})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}
