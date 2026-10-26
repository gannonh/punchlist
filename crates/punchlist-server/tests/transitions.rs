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
    assert_eq!(issue["row"]["issue"]["id"], "PL-1");
    assert_eq!(issue["row"]["issue"]["status"], "backlog");
    assert_eq!(issue["row"]["issue"]["status_name"], "Backlog");
    assert_eq!(issue["row"]["issue"]["body"], "Body");

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
    assert_eq!(event["seq"], 3);
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
            (1, "created", person.actor_id),
            (2, "transition", person.actor_id),
            (3, "transition", person.actor_id)
        ]
    );

    let (status, timeline) =
        call(&app, &person.token, "GET", "/api/issues/PL-1/events", None).await;
    assert_eq!(status, StatusCode::OK);
    // The events `move` returned are the ones the timeline reads back, after the creation.
    let timeline = timeline.as_array().unwrap();
    assert_eq!(timeline[0]["detail"], json!({"kind": "created"}));
    assert_eq!(&timeline[1..], &returned);
    let timeline: Vec<(&str, &str, &str, &str)> = timeline[1..]
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
    assert_eq!(list["last_event_seq"], 3);
    assert_eq!(list["issues"][0]["issue"]["id"], "PL-1");
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
        (2, 4, "start", "backlog")
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
            "/api/actors",
            "/api/github/webhook",
            "/api/inbox",
            "/api/inbox/read",
            "/api/issues",
            "/api/issues/{id}",
            "/api/issues/{id}/comments",
            "/api/issues/{id}/events",
            "/api/issues/{id}/gates",
            "/api/issues/{id}/proof",
            "/api/issues/{id}/pull-requests",
            "/api/issues/{id}/pull-requests/refresh",
            "/api/issues/{id}/retry",
            "/api/issues/{id}/review-threads",
            "/api/issues/{id}/runs",
            "/api/issues/{id}/transitions",
            "/api/projects",
            "/api/projects/{id}",
            "/api/projects/{id}/milestones",
            "/api/projects/{id}/milestones/{milestone_id}",
            "/api/proof/{id}",
            "/api/pull-requests",
            "/api/pull-requests/unlinked",
            "/api/runners",
            "/api/runners/{id}/heartbeat",
            "/api/runners/{id}/leases",
            "/api/runs/claim",
            "/api/runs/{id}/finish",
            "/api/runs/{id}/github-token",
            "/api/runs/{id}/log",
            "/api/runs/{id}/resume",
            "/api/runs/{id}/waiting",
            "/api/search",
            "/api/session",
            "/api/workflow",
            "/api/workflow/schema.json",
            "/api/workspaces",
            "/api/workspaces/{id}/actors",
            "/api/workspaces/{id}/events",
            "/api/workspaces/{id}/imports",
            "/api/workspaces/{id}/imports/linear",
            "/api/workspaces/{id}/imports/{run}",
            "/api/workspaces/{id}/inbox",
            "/api/workspaces/{id}/inbox/read",
            "/api/workspaces/{id}/issues",
            "/api/workspaces/{id}/projects",
            "/api/workspaces/{id}/pull-requests",
            "/api/workspaces/{id}/pull-requests/unlinked",
            "/api/workspaces/{id}/runners",
            "/api/workspaces/{id}/search",
            "/api/workspaces/{id}/workflow"
        ]
    );
}

/// The refusal rows of the issue, oldest first: gate, reason, actor, delivery.
async fn refusals(pool: &PgPool) -> Vec<(String, String, uuid::Uuid, Option<String>)> {
    sqlx::query_as("SELECT gate, reason, actor_id, delivery_id FROM refusal ORDER BY created_at")
        .fetch_all(pool)
        .await
        .unwrap()
}

async fn refusal_events(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM event WHERE kind = 'refusal'")
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test]
async fn an_agents_request_a_gate_refuses_is_recorded_once(pool: PgPool) {
    let (app, person) = setup(&pool).await;
    // A person may ask for In Progress → Agent Review too, so their gate refusal is tried.
    let built_in = punchlist_core::default_workflow();
    let text = built_in.source().replace(
        "by = [\"agent\"]\ngates = [\"pr_open\"",
        "by = [\"agent\", \"person\"]\ngates = [\"pr_open\"",
    );
    assert_ne!(text, built_in.source());
    let version = punchlist_core::Workflow::from_files(&text, built_in.prompts().clone())
        .unwrap()
        .version()
        .to_string();
    sqlx::query(
        "WITH v AS (
            INSERT INTO workflow_version (workspace_id, hash, commit_sha, workflow_toml, prompts)
            SELECT id, 'either', 'either', $1, $2 FROM workspace RETURNING id)
         UPDATE workspace SET workflow_version_id = (SELECT id FROM v)",
    )
    .bind(&text)
    .bind(serde_json::to_value(built_in.prompts()).unwrap())
    .execute(&pool)
    .await
    .unwrap();
    let (status, agent) = call(
        &app,
        &person.token,
        "POST",
        "/api/actors",
        Some(json!({"kind": "claude-code"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{agent}");
    let agent_id: uuid::Uuid = agent["actor"]["id"].as_str().unwrap().parse().unwrap();
    let agent = agent["token"].as_str().unwrap();
    for title in ["Built", "Approved", "Reviewed"] {
        call(
            &app,
            &person.token,
            "POST",
            "/api/issues",
            Some(json!({"title": title})),
        )
        .await;
    }
    sqlx::query(
        "UPDATE issue SET status = CASE id WHEN 'PL-1' THEN 'in_progress'
                                           WHEN 'PL-2' THEN 'todo' ELSE 'human_review' END",
    )
    .execute(&pool)
    .await
    .unwrap();
    let request = |token: &str, id: &str, to: &str| {
        let (app, token, uri) = (
            app.clone(),
            token.to_string(),
            format!("/api/issues/{id}/transitions"),
        );
        let to = to.to_string();
        async move { call(&app, &token, "POST", &uri, Some(json!({"to": to}))).await }
    };

    let (status, body) = request(agent, "PL-1", "agent_review").await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(
        (&body["code"], &body["gate"], &body["reason"]),
        (
            &json!("gate_failed"),
            &json!("pr_open"),
            &json!("no open pull request is linked to PL-1")
        )
    );
    let first = vec![(
        "pr_open".to_string(),
        "no open pull request is linked to PL-1".to_string(),
        agent_id,
        None,
    )];
    assert_eq!(refusals(&pool).await, first);
    assert_eq!(refusal_events(&pool).await, 1);
    let stored: (String, String, String) =
        sqlx::query_as("SELECT from_status, to_status, workflow_version FROM refusal")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        stored,
        ("in_progress".into(), "agent_review".into(), version.clone())
    );
    let (_, timeline) = call(&app, &person.token, "GET", "/api/issues/PL-1/events", None).await;
    let last = timeline.as_array().unwrap().last().unwrap().clone();
    assert_eq!(
        (&last["actor"]["name"], &last["detail"]),
        (
            &json!("Claude Code"),
            &json!({
                "kind": "refusal",
                "from": "in_progress",
                "from_name": "In Progress",
                "to": "agent_review",
                "to_name": "Agent Review",
                "gate": "pr_open",
                "reason": "no open pull request is linked to PL-1",
                "workflow_version": version,
            })
        )
    );

    // The same answer again writes nothing.
    let (status, _) = request(agent, "PL-1", "agent_review").await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(refusals(&pool).await, first);
    assert_eq!(refusal_events(&pool).await, 1);

    // A person's gate refusal, and an agent's for its role, a lock or no transition: none.
    let (status, body) = request(&person.token, "PL-1", "agent_review").await;
    assert_eq!(
        (status, &body["code"]),
        (StatusCode::CONFLICT, &json!("gate_failed"))
    );
    let (status, body) = request(agent, "PL-2", "start").await;
    assert_eq!(
        (status, &body["code"]),
        (StatusCode::CONFLICT, &json!("role_not_allowed"))
    );
    let (status, body) = request(agent, "PL-3", "merging").await;
    assert_eq!(
        (status, &body["code"]),
        (StatusCode::CONFLICT, &json!("locked"))
    );
    let (status, body) = request(agent, "PL-1", "human_review").await;
    assert_eq!(
        (status, &body["code"]),
        (StatusCode::CONFLICT, &json!("no_transition"))
    );
    assert_eq!(refusals(&pool).await, first);
    assert_eq!(refusal_events(&pool).await, 1);

    // A changed reason is a new refusal, and so is the same one after a move.
    sqlx::query(
        "INSERT INTO pull_request (repository_id, number, github_id, title, branch, head_sha,
                                   url, author_login, state, draft, issue_id, github_updated_at)
         SELECT id, 1, 1, 'Built (PL-1)', 'feature/pl-1', 'abc', 'https://github.com/x/y/pull/1',
                'gannonh', 'open', true, 'PL-1', now() FROM repository",
    )
    .execute(&pool)
    .await
    .unwrap();
    let (_, body) = request(agent, "PL-1", "agent_review").await;
    assert_eq!(body["gate"], "pr_ready");
    let gates: Vec<String> = refusals(&pool).await.into_iter().map(|r| r.0).collect();
    assert_eq!(gates, ["pr_open", "pr_ready"]);
    assert_eq!(refusal_events(&pool).await, 2);
}
