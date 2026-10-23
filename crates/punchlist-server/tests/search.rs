//! `GET /api/search` over the live checks' seed: what it finds, in what order, with which
//! snippet, and for whom.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use punchlist_server::{AppState, Bootstrap, Bootstrapped, bootstrap, router};
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
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn create(app: &Router, token: &str, title: &str, body: &str) -> String {
    let (status, created) = call(
        app,
        token,
        "POST",
        "/api/issues",
        Some(json!({"title": title, "body": body})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    created["row"]["issue"]["id"].as_str().unwrap().to_string()
}

/// Comments on `id`; returns the comment event's seq.
async fn comment(app: &Router, token: &str, id: &str, body: &str) -> i64 {
    let (status, event) = call(
        app,
        token,
        "POST",
        &format!("/api/issues/{id}/comments"),
        Some(json!({"body": body})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{event}");
    event["seq"].as_i64().unwrap()
}

/// The live checks' seed; returns the seq of the comment that says `100%`.
async fn seed(app: &Router, token: &str) -> i64 {
    create(
        app,
        token,
        "Gate the transition",
        "The runner restarts and leaks ci_green on every retry.",
    )
    .await;
    create(app, token, "Reword ci_green", "Plain.").await;
    let docs = create(app, token, "Docs pass", "Nothing.").await;
    comment(app, token, &docs, "Looks fine.").await;
    let seq = comment(
        app,
        token,
        &docs,
        "We saw ciXgreen in the log and 100% of retries pass.",
    )
    .await;
    create(
        app,
        token,
        "Escape test",
        "<img src=x onerror=\"window.__x=1\"> ci_green",
    )
    .await;
    create(app, token, "Red", "\u{1b}[31mred\u{1b}[0m").await;
    seq
}

async fn search(app: &Router, token: &str, query: &str) -> (StatusCode, Value) {
    call(app, token, "GET", &format!("/api/search?{query}"), None).await
}

/// Each hit as `[issue_id, in_title, snippet.field]`.
fn summary(results: &Value) -> Value {
    let hits = results["hits"].as_array().unwrap();
    hits.iter()
        .map(|hit| json!([hit["issue_id"], hit["in_title"], hit["snippet"]["field"]]))
        .collect()
}

#[sqlx::test]
async fn finds_titles_first_then_bodies_with_literal_snippets(pool: PgPool) {
    let person = workspace(&pool, "PL").await;
    let app = router(AppState::new(pool));
    seed(&app, &person.token).await;

    for query in ["q=ci_green", "q=CI_GREEN", "q=%20%20ci_green%20"] {
        let (status, results) = search(&app, &person.token, query).await;
        assert_eq!(status, StatusCode::OK, "{results}");
        assert_eq!(
            summary(&results),
            json!([
                ["PL-2", true, null],
                ["PL-4", false, "body"],
                ["PL-1", false, "body"]
            ]),
            "{query}"
        );
        assert_eq!(results["more"], false);
        assert_eq!(
            results["hits"][2],
            json!({
                "issue_id": "PL-1",
                "title": "Gate the transition",
                "status_name": "Backlog",
                "in_title": false,
                "snippet": {
                    "field": "body",
                    "before": "The runner restarts and leaks ",
                    "hit": "ci_green",
                    "after": " on every retry.",
                    "comment_seq": null,
                },
            })
        );
        assert_eq!(
            results["hits"][1]["snippet"]["before"],
            "<img src=x onerror=\"window.__x=1\"> "
        );
    }

    let (_, results) = search(&app, &person.token, "q=pl-1").await;
    assert_eq!(summary(&results), json!([["PL-1", true, null]]));
}

#[sqlx::test]
async fn wildcards_match_themselves_and_a_comment_names_its_event(pool: PgPool) {
    let person = workspace(&pool, "PL").await;
    let app = router(AppState::new(pool));
    let seq = seed(&app, &person.token).await;

    let (status, results) = search(&app, &person.token, "q=100%25").await;
    assert_eq!(status, StatusCode::OK, "{results}");
    assert_eq!(
        results["hits"],
        json!([{
            "issue_id": "PL-3",
            "title": "Docs pass",
            "status_name": "Backlog",
            "in_title": false,
            "snippet": {
                "field": "comment",
                "before": "We saw ciXgreen in the log and ",
                "hit": "100%",
                "after": " of retries pass.",
                "comment_seq": seq,
            },
        }])
    );

    let (_, results) = search(&app, &person.token, "q=%25%25%25").await;
    assert_eq!(results, json!({"hits": [], "more": false}));

    let (_, results) = search(&app, &person.token, "q=red").await;
    assert_eq!(
        results["hits"][0]["snippet"],
        json!({"field": "body", "before": " [31m", "hit": "red", "after": " [0m",
               "comment_seq": null})
    );
}

#[sqlx::test]
async fn refuses_short_long_and_agent_queries(pool: PgPool) {
    let person = workspace(&pool, "PL").await;
    let app = router(AppState::new(pool));

    let (status, body) = search(&app, &person.token, "q=ci").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["message"], "search needs at least 3 characters");
    let (status, _) = search(&app, &person.token, "q=%20%20ci%09%20").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, body) = search(&app, &person.token, &format!("q={}", "a".repeat(201))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["message"], "search takes at most 200 characters");

    let (status, agent) = call(
        &app,
        &person.token,
        "POST",
        "/api/actors",
        Some(json!({"kind": "codex"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{agent}");
    let token = agent["token"].as_str().unwrap();
    let (status, _) = search(&app, token, "q=ci_green").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[sqlx::test]
async fn searches_only_the_callers_workspace(pool: PgPool) {
    let first = workspace(&pool, "PL").await;
    let other = workspace(&pool, "OT").await;
    let app = router(AppState::new(pool));
    seed(&app, &first.token).await;
    let elsewhere = create(&app, &other.token, "ci_green elsewhere", "").await;
    comment(&app, &other.token, &elsewhere, "ci_green again").await;

    let (_, results) = search(&app, &first.token, "q=ci_green").await;
    assert_eq!(
        summary(&results),
        json!([
            ["PL-2", true, null],
            ["PL-4", false, "body"],
            ["PL-1", false, "body"]
        ])
    );
    let (_, results) = search(&app, &other.token, "q=ci_green").await;
    assert_eq!(summary(&results), json!([["OT-1", true, "comment"]]));
}

#[sqlx::test]
async fn limit_cuts_the_hits_and_says_there_are_more(pool: PgPool) {
    let person = workspace(&pool, "PL").await;
    let app = router(AppState::new(pool));
    seed(&app, &person.token).await;

    let (_, results) = search(&app, &person.token, "q=ci_green&limit=1").await;
    assert_eq!(summary(&results), json!([["PL-2", true, null]]));
    assert_eq!(results["more"], true);
    let (_, results) = search(&app, &person.token, "q=ci_green&limit=3").await;
    assert_eq!(results["more"], false);
}
