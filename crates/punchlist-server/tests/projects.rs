//! Editing projects and milestones: renames, `pass_when`, the `project` events a rename writes,
//! and what is refused. HTTP requests against the router, then the rows.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use punchlist_server::{AppState, Bootstrap, Bootstrapped, bootstrap, router};
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
    let request =
        (request.body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))).unwrap();
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

async fn post(app: &Router, token: &str, uri: &str, body: Value) -> Value {
    let (status, created) = call(app, Some(token), "POST", uri, Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    created
}

/// The `Punchlist` project with `Gate 1` (PASS when `Old`) and `Gate 2`, and PL-1 to PL-3 in
/// `Gate 1`. Returns the ids of the project and the two milestones.
async fn placed(app: &Router, token: &str) -> (String, String, String) {
    let project = post(app, token, "/api/projects", json!({"name": "Punchlist"})).await;
    let project = project["id"].as_str().unwrap().to_string();
    let milestones = format!("/api/projects/{project}/milestones");
    let gate1 = post(
        app,
        token,
        &milestones,
        json!({"name": "Gate 1", "pass_when": "Old"}),
    )
    .await;
    let gate1 = gate1["id"].as_str().unwrap().to_string();
    let gate2 = post(app, token, &milestones, json!({"name": "Gate 2"})).await;
    let gate2 = gate2["id"].as_str().unwrap().to_string();
    for n in 1..=3 {
        let issue = post(
            app,
            token,
            "/api/issues",
            json!({"title": format!("Issue {n}")}),
        )
        .await;
        let uri = format!(
            "/api/issues/{}",
            issue["row"]["issue"]["id"].as_str().unwrap()
        );
        let edit = json!({"project": project, "milestone": gate1});
        let (status, row) = call(app, Some(token), "PATCH", &uri, Some(edit)).await;
        assert_eq!(status, StatusCode::OK, "{row}");
    }
    (project, gate1, gate2)
}

/// The `project` events, each as `<issue> by <actor>`.
async fn project_events(pool: &PgPool) -> Vec<String> {
    let rows = sqlx::query!(
        "SELECT e.issue_id, a.name FROM event e JOIN actor a ON a.id = e.actor_id
         WHERE e.kind = 'project' ORDER BY e.seq"
    )
    .fetch_all(pool)
    .await
    .unwrap();
    rows.iter()
        .map(|r| format!("{} by {}", r.issue_id, r.name))
        .collect()
}

#[sqlx::test]
async fn a_rename_writes_one_project_event_per_issue_and_pass_when_none(pool: PgPool) {
    let who = workspace(&pool, "PL").await;
    let app = router(AppState::new(pool.clone()));
    let token = Some(who.token.as_str());
    let (project, gate1, gate2) = placed(&app, &who.token).await;
    let gate1_uri = format!("/api/projects/{project}/milestones/{gate1}");

    let edit = json!({"pass_when": " New text "});
    let (status, milestone) = call(&app, token, "PATCH", &gate1_uri, Some(edit)).await;
    assert_eq!(status, StatusCode::OK, "{milestone}");
    assert_eq!(
        milestone,
        json!({"id": gate1, "name": "Gate 1", "pass_when": "New text"})
    );
    assert_eq!(project_events(&pool).await, Vec::<String>::new());

    let uri = format!("/api/projects/{project}");
    let (status, renamed) = call(&app, token, "PATCH", &uri, Some(json!({"name": "Docs"}))).await;
    assert_eq!(status, StatusCode::OK, "{renamed}");
    assert_eq!(
        renamed,
        json!({"id": project, "name": "Docs", "milestones": [
            {"id": gate1, "name": "Gate 1", "pass_when": "New text"},
            {"id": gate2, "name": "Gate 2", "pass_when": ""},
        ]})
    );
    let events = ["PL-1 by Gannon", "PL-2 by Gannon", "PL-3 by Gannon"];
    assert_eq!(project_events(&pool).await, events);
    // The same name again changes nothing and writes nothing.
    let (status, _) = call(&app, token, "PATCH", &uri, Some(json!({"name": "Docs"}))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(project_events(&pool).await, events);

    let (status, row) = call(&app, token, "GET", "/api/issues", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        row["issues"][0]["issue"]["project"],
        json!({"id": project, "name": "Docs"})
    );

    let edit = json!({"name": "Launch", "pass_when": ""});
    let (status, milestone) = call(&app, token, "PATCH", &gate1_uri, Some(edit)).await;
    assert_eq!(status, StatusCode::OK, "{milestone}");
    assert_eq!(
        milestone,
        json!({"id": gate1, "name": "Launch", "pass_when": ""})
    );
    assert_eq!(project_events(&pool).await.len(), 6);
    let (_, row) = call(&app, token, "GET", "/api/issues", None).await;
    assert_eq!(
        row["issues"][0]["issue"]["milestone"],
        json!({"id": gate1, "name": "Launch"})
    );
    // A timeline leaves the `project` events out.
    let (_, timeline) = call(&app, token, "GET", "/api/issues/PL-1/events", None).await;
    let kinds: Vec<&str> = (timeline.as_array().unwrap().iter())
        .map(|e| e["detail"]["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["created", "edited"]);
    let edited = sqlx::query_scalar!("SELECT pass_when_edited FROM milestone ORDER BY created_at")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(edited, [true, false]);
}

#[sqlx::test]
async fn bad_edits_are_refused_and_change_nothing(pool: PgPool) {
    let who = workspace(&pool, "PL").await;
    let other = workspace(&pool, "OT").await;
    let app = router(AppState::new(pool.clone()));
    let token = Some(who.token.as_str());
    let (project, gate1, gate2) = placed(&app, &who.token).await;
    let groundwork = post(
        &app,
        &who.token,
        "/api/projects",
        json!({"name": "Groundwork"}),
    )
    .await;
    let groundwork = groundwork["id"].as_str().unwrap();
    let project_uri = format!("/api/projects/{project}");
    let gate2_uri = format!("/api/projects/{project}/milestones/{gate2}");

    let refused = [
        (
            &gate2_uri,
            json!({"name": "Gate 1"}),
            "project Punchlist already has a milestone named Gate 1",
        ),
        (
            &gate2_uri,
            json!({"name": "x".repeat(101)}),
            "a milestone name is at most 100 characters",
        ),
        (
            &gate2_uri,
            json!({"name": "  "}),
            "a milestone needs a name",
        ),
        (
            &gate2_uri,
            json!({}),
            "nothing to change: send name or pass_when",
        ),
        (
            &project_uri,
            json!({"name": "Groundwork"}),
            "a project named Groundwork already exists",
        ),
        (&project_uri, json!({"name": ""}), "a project needs a name"),
        (&project_uri, json!({}), "nothing to change: send name"),
    ];
    for (uri, body, message) in refused {
        let (status, answer) = call(&app, token, "PATCH", uri, Some(body)).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{answer}");
        assert_eq!(answer, json!({"code": "invalid", "message": message}));
    }

    let (status, created) = call(
        &app,
        token,
        "POST",
        "/api/actors",
        Some(json!({"kind": "codex"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let agent = created["token"].as_str().unwrap();
    let rename = || Some(json!({"name": "Mine"}));
    let random = Uuid::new_v4();
    let cases = [
        (Some(agent), project_uri.clone(), StatusCode::FORBIDDEN),
        (Some(agent), gate2_uri.clone(), StatusCode::FORBIDDEN),
        (None, project_uri.clone(), StatusCode::UNAUTHORIZED),
        (
            token,
            format!("/api/projects/{random}"),
            StatusCode::NOT_FOUND,
        ),
        (
            Some(other.token.as_str()),
            project_uri.clone(),
            StatusCode::NOT_FOUND,
        ),
        (
            token,
            format!("/api/projects/{groundwork}/milestones/{gate1}"),
            StatusCode::NOT_FOUND,
        ),
        (
            token,
            format!("/api/projects/{project}/milestones/{random}"),
            StatusCode::NOT_FOUND,
        ),
    ];
    for (token, uri, expected) in cases {
        let (status, answer) = call(&app, token, "PATCH", &uri, rename()).await;
        assert_eq!(status, expected, "{uri}: {answer}");
    }
    let (_, answer) = call(
        &app,
        token,
        "PATCH",
        &format!("/api/projects/{groundwork}/milestones/{gate1}"),
        rename(),
    )
    .await;
    assert_eq!(
        answer,
        json!({"code": "not_found", "message": format!("milestone {gate1} not found in this project")})
    );

    let names = sqlx::query_scalar!(
        "SELECT name FROM milestone UNION ALL SELECT name FROM project ORDER BY name"
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let names: Vec<String> = names.into_iter().flatten().collect();
    assert_eq!(names, ["Gate 1", "Gate 2", "Groundwork", "Punchlist"]);
    assert_eq!(project_events(&pool).await, Vec::<String>::new());
}
