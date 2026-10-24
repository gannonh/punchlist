//! `children_done`: a parent moves when a child's move leaves all its children Done or
//! Canceled, one at least Done, and so on up the tree.

use axum::Router;
use axum::body::Body;
use axum::http::Request;
use http_body_util::BodyExt;
use punchlist_server::{AppState, Bootstrap, Bootstrapped, bootstrap, router};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

/// The built-in workflow, where a person moves Todo to Done or Canceled, and with
/// `backlog → done` on `children_done` when `on` is true.
async fn setup(pool: &PgPool, on: bool) -> (Router, Bootstrapped) {
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
    let built_in = punchlist_core::default_workflow();
    let mut added = "[[transition]]\nfrom = \"todo\"\nto = \"done\"\nby = [\"person\"]\n\
         [[transition]]\nfrom = \"todo\"\nto = \"canceled\"\nby = [\"person\"]\n"
        .to_string();
    if on {
        added.push_str(
            "[[transition]]\nfrom = \"backlog\"\nto = \"done\"\non = \"children_done\"\n",
        );
    }
    let text = built_in
        .source()
        .replace("[lock]", &format!("{added}[lock]"));
    punchlist_core::Workflow::from_toml(&text).expect("the test workflow parses");
    sqlx::query(
        "WITH v AS (
            INSERT INTO workflow_version (workspace_id, hash, commit_sha, workflow_toml, prompts)
            SELECT id, $1, $1, $2, $3 FROM workspace RETURNING id)
         UPDATE workspace SET workflow_version_id = (SELECT id FROM v)",
    )
    .bind(format!("children-done-{on}"))
    .bind(text)
    .bind(serde_json::to_value(built_in.prompts()).unwrap())
    .execute(pool)
    .await
    .unwrap();
    (router(AppState::new(pool.clone())), person)
}

async fn call(app: &Router, token: &str, method: &str, uri: &str, body: Value) -> Value {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(status.is_success(), "{method} {uri}: {status} {json}");
    json
}

/// Creates an issue under `parent`; returns its id.
async fn issue(app: &Router, token: &str, title: &str, parent: Option<&str>) -> String {
    let created = call(app, token, "POST", "/api/issues", json!({"title": title})).await;
    let id = created["row"]["issue"]["id"].as_str().unwrap().to_string();
    if let Some(parent) = parent {
        let uri = format!("/api/issues/{id}");
        call(app, token, "PATCH", &uri, json!({"parent": parent})).await;
    }
    id
}

async fn moves(app: &Router, token: &str, id: &str, to: &[&str]) {
    for to in to {
        let uri = format!("/api/issues/{id}/transitions");
        call(app, token, "POST", &uri, json!({"to": to})).await;
    }
}

async fn status(pool: &PgPool, id: &str) -> String {
    sqlx::query_scalar("SELECT status FROM issue WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Each transition of `id`, oldest first, with its actor's name and role.
async fn transitions(pool: &PgPool, id: &str) -> Vec<(String, String, String, String)> {
    sqlx::query_as(
        "SELECT t.from_status, t.to_status, a.name, a.role FROM transition t
         JOIN actor a ON a.id = t.actor_id WHERE t.issue_id = $1 ORDER BY t.created_at",
    )
    .bind(id)
    .fetch_all(pool)
    .await
    .unwrap()
}

fn by_punchlist(from: &str, to: &str) -> (String, String, String, String) {
    (from.into(), to.into(), "Punchlist".into(), "github".into())
}

#[sqlx::test]
async fn an_epic_moves_to_done_when_its_second_child_does(pool: PgPool) {
    let (app, person) = setup(&pool, true).await;
    let t = &person.token;
    let epic = issue(&app, t, "Epic", None).await;
    let first = issue(&app, t, "First", Some(&epic)).await;
    let second = issue(&app, t, "Second", Some(&epic)).await;

    moves(&app, t, &first, &["todo", "done"]).await;
    assert_eq!(status(&pool, &epic).await, "backlog");
    assert_eq!(transitions(&pool, &epic).await, []);

    moves(&app, t, &second, &["todo", "done"]).await;
    assert_eq!(status(&pool, &epic).await, "done");
    assert_eq!(
        transitions(&pool, &epic).await,
        [by_punchlist("backlog", "done")]
    );
    let events: Vec<(i64, String, String)> =
        sqlx::query_as("SELECT e.seq, e.issue_id, e.kind FROM event e ORDER BY e.seq DESC LIMIT 2")
            .fetch_all(&pool)
            .await
            .unwrap();
    let kinds: Vec<_> = events
        .iter()
        .map(|(_, id, kind)| (id.as_str(), kind.as_str()))
        .collect();
    assert_eq!(
        kinds,
        [
            (epic.as_str(), "transition"),
            (second.as_str(), "transition")
        ]
    );
}

#[sqlx::test]
async fn an_epic_moves_to_done_when_its_last_open_child_is_canceled(pool: PgPool) {
    let (app, person) = setup(&pool, true).await;
    let t = &person.token;
    let epic = issue(&app, t, "Epic", None).await;
    let first = issue(&app, t, "First", Some(&epic)).await;
    let second = issue(&app, t, "Second", Some(&epic)).await;

    moves(&app, t, &first, &["todo", "done"]).await;
    moves(&app, t, &second, &["todo", "canceled"]).await;
    assert_eq!(status(&pool, &epic).await, "done");
    assert_eq!(
        transitions(&pool, &epic).await,
        [by_punchlist("backlog", "done")]
    );
}

#[sqlx::test]
async fn an_epic_with_an_open_child_or_none_done_stays(pool: PgPool) {
    let (app, person) = setup(&pool, true).await;
    let t = &person.token;
    let epic = issue(&app, t, "Epic", None).await;
    let first = issue(&app, t, "First", Some(&epic)).await;
    let second = issue(&app, t, "Second", Some(&epic)).await;
    moves(&app, t, &first, &["todo", "done"]).await;
    moves(&app, t, &second, &["todo"]).await;
    assert_eq!(status(&pool, &epic).await, "backlog");

    let dropped = issue(&app, t, "Dropped", None).await;
    let only = issue(&app, t, "Only", Some(&dropped)).await;
    moves(&app, t, &only, &["todo", "canceled"]).await;
    assert_eq!(status(&pool, &dropped).await, "backlog");

    // A parent with no children never gets the event: an issue without one moves alone.
    let alone = issue(&app, t, "Alone", None).await;
    moves(&app, t, &alone, &["todo", "done"]).await;
    for id in [&epic, &dropped] {
        assert_eq!(transitions(&pool, id).await, [], "{id}");
    }
}

#[sqlx::test]
async fn a_grandparent_moves_when_its_last_child_epic_does(pool: PgPool) {
    let (app, person) = setup(&pool, true).await;
    let t = &person.token;
    let top = issue(&app, t, "Top", None).await;
    let epic = issue(&app, t, "Epic", Some(&top)).await;
    let child = issue(&app, t, "Child", Some(&epic)).await;

    moves(&app, t, &child, &["todo", "done"]).await;
    assert_eq!(status(&pool, &epic).await, "done");
    assert_eq!(status(&pool, &top).await, "done");
    assert_eq!(
        transitions(&pool, &top).await,
        [by_punchlist("backlog", "done")]
    );
}

#[sqlx::test]
async fn a_parent_moves_only_from_the_statuses_the_transition_names(pool: PgPool) {
    let (app, person) = setup(&pool, true).await;
    let t = &person.token;
    let epic = issue(&app, t, "Epic", None).await;
    let child = issue(&app, t, "Child", Some(&epic)).await;
    moves(&app, t, &epic, &["todo"]).await;
    moves(&app, t, &child, &["todo", "done"]).await;
    assert_eq!(status(&pool, &epic).await, "todo");
}

#[sqlx::test]
async fn a_workflow_without_children_done_moves_nothing(pool: PgPool) {
    let (app, person) = setup(&pool, false).await;
    let t = &person.token;
    let epic = issue(&app, t, "Epic", None).await;
    let child = issue(&app, t, "Child", Some(&epic)).await;
    moves(&app, t, &child, &["todo", "done"]).await;
    assert_eq!(status(&pool, &epic).await, "backlog");
    assert_eq!(transitions(&pool, &epic).await, []);
}

/// Sets statuses with SQL, as children closed before `children_done` existed were.
async fn closed_before(pool: &PgPool, ids: &[(&str, &str)]) {
    for (id, status) in ids {
        sqlx::query("UPDATE issue SET status = $2 WHERE id = $1")
            .bind(id)
            .bind(status)
            .execute(pool)
            .await
            .unwrap();
    }
}

async fn transition_count(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM transition")
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test]
async fn a_start_moves_epics_whose_children_were_already_done(pool: PgPool) {
    let (app, person) = setup(&pool, true).await;
    let t = &person.token;
    let epic = issue(&app, t, "Epic", None).await;
    let first = issue(&app, t, "First", Some(&epic)).await;
    let second = issue(&app, t, "Second", Some(&epic)).await;
    let open = issue(&app, t, "Open", None).await;
    let closed = issue(&app, t, "Closed", Some(&open)).await;
    let todo = issue(&app, t, "Todo", Some(&open)).await;
    let top = issue(&app, t, "Top", None).await;
    let middle = issue(&app, t, "Middle", Some(&top)).await;
    let leaf = issue(&app, t, "Leaf", Some(&middle)).await;
    closed_before(
        &pool,
        &[
            (&first, "done"),
            (&second, "canceled"),
            (&closed, "done"),
            (&todo, "todo"),
            (&leaf, "done"),
        ],
    )
    .await;

    punchlist_server::catch_up_at_start(&pool).await.unwrap();
    assert_eq!(status(&pool, &epic).await, "done");
    assert_eq!(
        transitions(&pool, &epic).await,
        [by_punchlist("backlog", "done")]
    );
    assert_eq!(status(&pool, &open).await, "backlog");
    assert_eq!(transitions(&pool, &open).await, []);
    // The top comes before its child epic, and moves once that epic does.
    assert_eq!(status(&pool, &middle).await, "done");
    assert_eq!(status(&pool, &top).await, "done");
    assert_eq!(
        transitions(&pool, &top).await,
        [by_punchlist("backlog", "done")]
    );

    assert_eq!(transition_count(&pool).await, 3);
    punchlist_server::catch_up_at_start(&pool).await.unwrap();
    assert_eq!(transition_count(&pool).await, 3);
}

#[sqlx::test]
async fn a_start_without_children_done_in_the_workflow_moves_nothing(pool: PgPool) {
    let (app, person) = setup(&pool, false).await;
    let t = &person.token;
    let epic = issue(&app, t, "Epic", None).await;
    let child = issue(&app, t, "Child", Some(&epic)).await;
    closed_before(&pool, &[(&child, "done")]).await;
    punchlist_server::catch_up_at_start(&pool).await.unwrap();
    assert_eq!(status(&pool, &epic).await, "backlog");
    assert_eq!(transition_count(&pool).await, 0);
}
