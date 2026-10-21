//! Importing a Linear project: the snapshot `pl import linear` posts, the job that writes it,
//! and a rerun that writes nothing twice. HTTP requests against the router, then the rows.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use punchlist_server::{AppState, Bootstrap, Bootstrapped, bootstrap, router, run_jobs_until_idle};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

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

fn user(id: &str, name: &str) -> Value {
    json!({"id": id, "name": name})
}

fn comment(id: &str, body: &str, user: Value, at: &str) -> Value {
    json!({"id": id, "body": body, "user": user, "created_at": at})
}

/// KAT-3 was created first, so it is PL-1; then KAT-1 (PL-2) and its child KAT-2 (PL-3).
fn snapshot() -> Value {
    let gannon = user("u-gannon", "Gannon Hall");
    let ada = user("u-ada", "Ada");
    json!({
        "team": "KAT",
        "project": {
            "id": "p-punchlist",
            "name": "Punchlist",
            "url": "https://linear.app/kata-sh/project/punchlist",
            "milestones": [
                {"id": "m-gate0", "name": "Gate 0", "description": "PASS when pl creates and moves an issue."},
                {"id": "m-phase1", "name": "Phase 1", "description": null}
            ]
        },
        "issues": [
            {
                "id": "i-1", "identifier": "KAT-1", "url": "https://linear.app/kata-sh/issue/KAT-1",
                "title": " Spec ", "description": "The spec.",
                "state": {"name": "Done", "type": "completed"},
                "labels": ["Feature", "spec", "Feature"], "parent_id": null, "milestone_id": "m-gate0",
                "creator": gannon,
                "created_at": "2026-09-01T10:00:00Z", "updated_at": "2026-09-05T10:00:00Z",
                "comments": [
                    comment("c-1", "Looks good.", gannon.clone(), "2026-09-02T10:00:00Z"),
                    comment("c-2", "Opened #3.\n\n(agent)", gannon.clone(), "2026-09-03T10:00:00Z"),
                    comment("c-3", "Linked a pull request.", Value::Null, "2026-09-04T10:00:00Z")
                ]
            },
            {
                "id": "i-2", "identifier": "KAT-2", "url": "https://linear.app/kata-sh/issue/KAT-2",
                "title": "Child", "description": null,
                "state": {"name": "In Progress", "type": "started"},
                "labels": [], "parent_id": "i-1", "milestone_id": "m-gate0",
                "creator": ada,
                "created_at": "2026-09-02T09:00:00Z", "updated_at": "2026-09-06T10:00:00Z",
                "comments": [comment("c-4", "  \n", ada.clone(), "2026-09-06T09:00:00Z")]
            },
            {
                "id": "i-3", "identifier": "KAT-3", "url": "https://linear.app/kata-sh/issue/KAT-3",
                "title": "Orphan", "description": "",
                "state": {"name": "Agent Review", "type": "started"},
                "labels": [], "parent_id": "i-outside", "milestone_id": null,
                "creator": null,
                "created_at": "2026-08-01T10:00:00Z", "updated_at": "2026-08-01T10:00:00Z",
                "comments": []
            }
        ]
    })
}

async fn import(state: &AppState, app: &Router, who: &Bootstrapped, snapshot: Value) -> Value {
    let uri = format!("/api/workspaces/{}/imports/linear", who.workspace_id);
    let (status, run) = call(app, &who.token, "POST", &uri, Some(snapshot)).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{run}");
    assert_eq!(run["state"], "queued");
    run_jobs_until_idle(state).await.unwrap();
    let uri = format!("/api/workspaces/{}/imports/{}", who.workspace_id, run["id"]);
    let (status, run) = call(app, &who.token, "GET", &uri, None).await;
    assert_eq!(status, StatusCode::OK, "{run}");
    assert_eq!(run["state"], "done", "{run}");
    run["report"].clone()
}

/// Rows per table: issues, comments, events, actors, projects, milestones, transitions.
async fn counts(pool: &PgPool) -> [i64; 7] {
    let row = sqlx::query!(
        r#"SELECT (SELECT count(*) FROM issue) AS "issues!",
                  (SELECT count(*) FROM comment) AS "comments!",
                  (SELECT count(*) FROM event) AS "events!",
                  (SELECT count(*) FROM actor) AS "actors!",
                  (SELECT count(*) FROM project) AS "projects!",
                  (SELECT count(*) FROM milestone) AS "milestones!",
                  (SELECT count(*) FROM transition) AS "transitions!""#
    )
    .fetch_one(pool)
    .await
    .unwrap();
    [
        row.issues,
        row.comments,
        row.events,
        row.actors,
        row.projects,
        row.milestones,
        row.transitions,
    ]
}

#[sqlx::test]
async fn a_project_imports_once_and_a_rerun_changes_nothing(pool: PgPool) {
    let who = workspace(&pool, "PL").await;
    let state = AppState::new(pool.clone());
    let app = router(state.clone());

    let report = import(&state, &app, &who, snapshot()).await;
    assert_eq!(
        report,
        json!({
            "created": {"issues": 3, "comments": 3, "projects": 1, "milestones": 2, "people": 2},
            "updated": {"issues": 0, "comments": 0, "projects": 0, "milestones": 0, "people": 0},
            "notes": [
                "KAT-3 (PL-1) has a parent outside Linear's project, so it has none here",
                "KAT-2 (PL-3): skipped an empty comment from 2026-09-06 09:00 UTC"
            ]
        })
    );
    assert_eq!(counts(&pool).await, [3, 3, 6, 4, 1, 2, 0]);

    let issues = sqlx::query!(
        r#"SELECT i.id, i.title, i.body, i.status, i.labels, i.parent_id, i.linear_identifier,
                  i.linear_url, a.name AS creator, m.name AS "milestone?", p.name AS "project?",
                  i.created_at
           FROM issue i JOIN actor a ON a.id = i.created_by
           LEFT JOIN milestone m ON m.id = i.milestone_id LEFT JOIN project p ON p.id = i.project_id
           ORDER BY i.number"#
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let shown: Vec<String> = issues
        .iter()
        .map(|i| {
            format!(
                "{} {} {:?} {} {:?} {:?} {} {} by {} in {:?}/{:?} at {}",
                i.id,
                i.linear_identifier.as_deref().unwrap(),
                i.title,
                i.status,
                i.labels,
                i.parent_id,
                i.linear_url.as_deref().unwrap(),
                i.body.len(),
                i.creator,
                i.project,
                i.milestone,
                i.created_at.to_rfc3339()
            )
        })
        .collect();
    assert_eq!(
        shown,
        [
            "PL-1 KAT-3 \"Orphan\" agent_review [] None https://linear.app/kata-sh/issue/KAT-3 0 by Gannon in Some(\"Punchlist\")/None at 2026-08-01T10:00:00+00:00",
            "PL-2 KAT-1 \"Spec\" done [\"Feature\", \"spec\"] None https://linear.app/kata-sh/issue/KAT-1 9 by Gannon Hall in Some(\"Punchlist\")/Some(\"Gate 0\") at 2026-09-01T10:00:00+00:00",
            "PL-3 KAT-2 \"Child\" in_progress [] Some(\"PL-2\") https://linear.app/kata-sh/issue/KAT-2 0 by Ada in Some(\"Punchlist\")/Some(\"Gate 0\") at 2026-09-02T09:00:00+00:00",
        ]
    );
    let milestones = sqlx::query!("SELECT name, pass_when FROM milestone ORDER BY name")
        .fetch_all(&pool)
        .await
        .unwrap();
    let milestones: Vec<(&str, &str)> = (milestones.iter())
        .map(|m| (m.name.as_str(), m.pass_when.as_str()))
        .collect();
    assert_eq!(
        milestones,
        [
            ("Gate 0", "PASS when pl creates and moves an issue."),
            ("Phase 1", "")
        ]
    );
    let comments = sqlx::query!(
        r#"SELECT c.issue_id, c.body, a.name, a.role, a.kind, a.token_hash, c.created_at
           FROM comment c JOIN actor a ON a.id = c.actor_id ORDER BY c.created_at"#
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let comments: Vec<String> = (comments.iter())
        .map(|c| {
            format!(
                "{} {:?} {} {} {:?} token {} at {}",
                c.issue_id,
                c.body,
                c.name,
                c.role,
                c.kind,
                c.token_hash.is_some(),
                c.created_at.to_rfc3339()
            )
        })
        .collect();
    assert_eq!(
        comments,
        [
            "PL-2 \"Looks good.\" Gannon Hall person None token false at 2026-09-02T10:00:00+00:00",
            "PL-2 \"Opened #3.\\n\\n(agent)\" Imported agent agent Some(\"imported\") token false at 2026-09-03T10:00:00+00:00",
            "PL-2 \"Linked a pull request.\" Imported agent agent Some(\"imported\") token false at 2026-09-04T10:00:00+00:00",
        ]
    );
    let events = sqlx::query!("SELECT seq, issue_id, kind FROM event ORDER BY seq")
        .fetch_all(&pool)
        .await
        .unwrap();
    let events: Vec<String> = (events.iter())
        .map(|e| format!("{} {} {}", e.seq, e.issue_id, e.kind))
        .collect();
    assert_eq!(
        events,
        [
            "1 PL-1 created",
            "2 PL-2 created",
            "3 PL-3 created",
            "4 PL-2 comment",
            "5 PL-2 comment",
            "6 PL-2 comment"
        ]
    );

    let (status, shown) = call(&app, &who.token, "GET", "/api/issues/PL-2", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        shown["linear"],
        json!({"identifier": "KAT-1", "url": "https://linear.app/kata-sh/issue/KAT-1"})
    );

    let again = import(&state, &app, &who, snapshot()).await;
    assert_eq!(
        again,
        json!({
            "created": {"issues": 0, "comments": 0, "projects": 0, "milestones": 0, "people": 0},
            "updated": {"issues": 0, "comments": 0, "projects": 0, "milestones": 0, "people": 0},
            "notes": ["KAT-2 (PL-3): skipped an empty comment from 2026-09-06 09:00 UTC"]
        })
    );
    assert_eq!(counts(&pool).await, [3, 3, 6, 4, 1, 2, 0]);

    let uri = format!("/api/workspaces/{}/imports", who.workspace_id);
    let (status, imports) = call(&app, &who.token, "GET", &uri, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        imports["totals"],
        json!({"issues": 3, "comments": 3, "projects": 1, "milestones": 2, "people": 2})
    );
    assert_eq!(imports["last_run"]["state"], "done");
    assert_eq!(imports["last_run"]["report"], again);
}

#[sqlx::test]
async fn a_rerun_updates_what_linear_changed_and_reports_what_it_dropped(pool: PgPool) {
    let who = workspace(&pool, "PL").await;
    let state = AppState::new(pool.clone());
    let app = router(state.clone());
    import(&state, &app, &who, snapshot()).await;
    // PL-3 waits on a dispatch for In Progress, which the move below ends.
    sqlx::query(
        "WITH t AS (INSERT INTO transition (issue_id, from_status, to_status, actor_id, workflow_version)
                    SELECT 'PL-3', 'todo', 'in_progress', id, 'v0' FROM actor WHERE name = 'Gannon'
                    RETURNING id)
         INSERT INTO dispatch (issue_id, transition_id, agent) SELECT 'PL-3', id, 'claude-code' FROM t",
    )
    .execute(&pool)
    .await
    .unwrap();

    let mut changed = snapshot();
    let issues = changed["issues"].as_array_mut().unwrap();
    issues.retain(|issue| issue["identifier"] != "KAT-3");
    let child = &mut issues[1];
    child["title"] = json!("Child, renamed");
    child["state"] = json!({"name": "Agent Review", "type": "started"});
    child["updated_at"] = json!("2026-09-07T10:00:00Z");
    // Linear changed nothing here since the last import, so the stored issue stays.
    issues[0]["title"] = json!("Ignored: updated_at did not move");

    let report = import(&state, &app, &who, changed).await;
    assert_eq!(
        report,
        json!({
            "created": {"issues": 0, "comments": 0, "projects": 0, "milestones": 0, "people": 0},
            "updated": {"issues": 1, "comments": 0, "projects": 0, "milestones": 0, "people": 0},
            "notes": [
                "KAT-2 (PL-3): skipped an empty comment from 2026-09-06 09:00 UTC",
                "KAT-3 (PL-1) is no longer in Linear's project"
            ]
        })
    );
    let issues = sqlx::query!("SELECT id, title, status FROM issue ORDER BY number")
        .fetch_all(&pool)
        .await
        .unwrap();
    let issues: Vec<(&str, &str, &str)> = (issues.iter())
        .map(|i| (i.id.as_str(), i.title.as_str(), i.status.as_str()))
        .collect();
    assert_eq!(
        issues,
        [
            ("PL-1", "Orphan", "agent_review"),
            ("PL-2", "Spec", "done"),
            ("PL-3", "Child, renamed", "agent_review"),
        ]
    );
    let events = sqlx::query!(
        "SELECT e.issue_id, e.kind, e.edited_fields, a.name FROM event e
         JOIN actor a ON a.id = e.actor_id WHERE e.seq > 6 ORDER BY e.seq"
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let events: Vec<String> = (events.iter())
        .map(|e| {
            format!(
                "{} {} {:?} by {}",
                e.issue_id, e.kind, e.edited_fields, e.name
            )
        })
        .collect();
    assert_eq!(
        events,
        [
            "PL-3 edited Some([\"title\"]) by Gannon",
            "PL-3 transition None by Gannon"
        ]
    );
    let transition = sqlx::query!(
        "SELECT issue_id, from_status, to_status FROM transition WHERE workflow_version <> 'v0'"
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        (
            transition.issue_id.as_str(),
            transition.from_status.as_str(),
            transition.to_status.as_str()
        ),
        ("PL-3", "in_progress", "agent_review")
    );
    let dispatches = sqlx::query_scalar!(r#"SELECT count(*) AS "n!" FROM dispatch"#)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(dispatches, 0);
}

#[sqlx::test]
async fn an_unmapped_state_imports_nothing_and_names_the_state(pool: PgPool) {
    let who = workspace(&pool, "PL").await;
    let app = router(AppState::new(pool.clone()));
    let mut triaged = snapshot();
    triaged["issues"][0]["state"] = json!({"name": "Triage", "type": "triage"});
    let uri = format!("/api/workspaces/{}/imports/linear", who.workspace_id);
    let (status, body) = call(&app, &who.token, "POST", &uri, Some(triaged)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        body["message"],
        "Linear state \"Triage\" maps to no status; map it under [import.linear.states] in .punchlist/workflow.toml"
    );
    let row = sqlx::query!(
        r#"SELECT (SELECT count(*) FROM issue) AS "issues!",
                  (SELECT count(*) FROM job) AS "jobs!""#
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((row.issues, row.jobs), (0, 0));
}

#[sqlx::test]
async fn a_repeated_issue_or_comment_is_refused_before_anything_is_queued(pool: PgPool) {
    let who = workspace(&pool, "PL").await;
    let app = router(AppState::new(pool.clone()));
    let uri = format!("/api/workspaces/{}/imports/linear", who.workspace_id);
    let mut twice = snapshot();
    let first = twice["issues"][0].clone();
    twice["issues"].as_array_mut().unwrap().push(first);
    let (status, body) = call(&app, &who.token, "POST", &uri, Some(twice)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["message"], "Linear issue KAT-1 appears twice");

    let mut twice = snapshot();
    let comment = twice["issues"][0]["comments"][0].clone();
    twice["issues"][0]["comments"]
        .as_array_mut()
        .unwrap()
        .push(comment);
    let (status, body) = call(&app, &who.token, "POST", &uri, Some(twice)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["message"], "Linear comment c-1 on KAT-1 appears twice");
    let jobs = sqlx::query_scalar!(r#"SELECT count(*) AS "n!" FROM job"#)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(jobs, 0);
}

#[sqlx::test]
async fn another_workspace_is_not_found_and_an_agent_is_refused(pool: PgPool) {
    let who = workspace(&pool, "PL").await;
    let other = workspace(&pool, "OT").await;
    let app = router(AppState::new(pool.clone()));
    for uri in [
        format!("/api/workspaces/{}/imports/linear", other.workspace_id),
        format!("/api/workspaces/{}/imports", other.workspace_id),
        format!("/api/workspaces/{}/imports/1", other.workspace_id),
    ] {
        let method = if uri.ends_with("linear") {
            "POST"
        } else {
            "GET"
        };
        let body = (method == "POST").then(snapshot);
        let (status, answer) = call(&app, &who.token, method, &uri, body).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}: {answer}");
        assert_eq!(
            answer["message"],
            format!("workspace {} not found", other.workspace_id)
        );
    }
    let (status, created) = call(
        &app,
        &who.token,
        "POST",
        "/api/actors",
        Some(json!({"kind": "codex"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let agent = created["token"].as_str().unwrap();
    let uri = format!("/api/workspaces/{}/imports", who.workspace_id);
    let (status, _) = call(&app, agent, "GET", &uri, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let jobs = sqlx::query_scalar!(r#"SELECT count(*) AS "n!" FROM job"#)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(jobs, 0);
}

#[sqlx::test]
async fn a_state_that_stops_mapping_before_the_job_runs_fails_it_at_once(pool: PgPool) {
    let who = workspace(&pool, "PL").await;
    let state = AppState::new(pool.clone());
    let app = router(state.clone());
    let uri = format!("/api/workspaces/{}/imports/linear", who.workspace_id);
    let (status, run) = call(&app, &who.token, "POST", &uri, Some(snapshot())).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    // As if the workflow changed after the request was accepted.
    sqlx::query!(
        "UPDATE job SET payload = jsonb_set(payload, '{snapshot,issues,0,state}',
                                            '{\"name\": \"Triage\", \"type\": \"triage\"}')"
    )
    .execute(&pool)
    .await
    .unwrap();
    run_jobs_until_idle(&state).await.unwrap();
    let uri = format!("/api/workspaces/{}/imports/{}", who.workspace_id, run["id"]);
    let (_, run) = call(&app, &who.token, "GET", &uri, None).await;
    assert_eq!(run["state"], "failed");
    assert_eq!(
        run["error"],
        "Linear state \"Triage\" maps to no status; map it under [import.linear.states] in .punchlist/workflow.toml"
    );
    let attempts = sqlx::query_scalar!("SELECT attempts FROM job")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(attempts, 1);
    assert_eq!(counts(&pool).await[0], 0);
}
