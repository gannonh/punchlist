//! GitHub webhook deliveries end to end: signed requests against the router, the jobs they
//! queue, and the rows the jobs write, read back from Postgres.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use hmac::{Hmac, KeyInit, Mac};
use http_body_util::BodyExt;
use punchlist_server::{
    AppState, Bootstrap, Bootstrapped, GithubClient, bootstrap, router, run_jobs_until_idle,
};
use serde_json::{Value, json};
use sha2::Sha256;
use sqlx::PgPool;
use tower::ServiceExt;

const SECRET: &str = "test-webhook-secret";

const OPENED: &str = include_str!("fixtures/github/pull_request_opened.json");
const READY: &str = include_str!("fixtures/github/pull_request_ready_for_review.json");
const MERGED: &str = include_str!("fixtures/github/pull_request_closed_merged.json");
const CLOSED: &str = include_str!("fixtures/github/pull_request_closed_unmerged.json");
const CHECK_RUN: &str = include_str!("fixtures/github/check_run_completed.json");
const THREAD: &str = include_str!("fixtures/github/pull_request_review_thread_resolved.json");

struct Setup {
    app: Router,
    state: AppState,
    person: Bootstrapped,
}

async fn setup(pool: &PgPool) -> Setup {
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
    let state = AppState::new(pool.clone()).with_github_webhook_secret(SECRET);
    Setup {
        app: router(state.clone()),
        state,
        person,
    }
}

fn sign(secret: &str, body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(body);
    let hex: String = mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("sha256={hex}")
}

/// Sends a delivery signed with `secret` and returns the status.
async fn send(app: &Router, secret: &str, event: &str, delivery: &str, body: &str) -> StatusCode {
    let request = Request::builder()
        .method("POST")
        .uri("/api/github/webhook")
        .header("content-type", "application/json")
        .header("x-github-event", event)
        .header("x-github-delivery", delivery)
        .header("x-hub-signature-256", sign(secret, body.as_bytes()))
        .body(Body::from(body.to_string()))
        .unwrap();
    app.clone().oneshot(request).await.unwrap().status()
}

/// A signed delivery, then the worker run to idle.
async fn deliver(s: &Setup, event: &str, delivery: &str, body: &str) {
    assert_eq!(
        send(&s.app, SECRET, event, delivery, body).await,
        StatusCode::ACCEPTED
    );
    run_jobs_until_idle(&s.state).await.unwrap();
}

/// A fixture with `edit` applied to its JSON.
fn edited(fixture: &str, edit: impl FnOnce(&mut Value)) -> String {
    let mut value: Value = serde_json::from_str(fixture).unwrap();
    edit(&mut value);
    value.to_string()
}

async fn create_issue(s: &Setup, title: &str) -> String {
    let request = Request::builder()
        .method("POST")
        .uri("/api/issues")
        .header("authorization", format!("Bearer {}", s.person.token))
        .header("content-type", "application/json")
        .body(Body::from(json!({ "title": title }).to_string()))
        .unwrap();
    let response = s.app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice::<Value>(&bytes).unwrap()["row"]["issue"]["id"]
        .as_str()
        .unwrap()
        .to_string()
}

/// The workflow gives a person no easy path to `merging` or `in_progress` (they need a
/// runner, a pull request and review), so tests put the issue there directly.
async fn force_status(pool: &PgPool, issue: &str, status: &str) {
    sqlx::query("UPDATE issue SET status = $2 WHERE id = $1")
        .bind(issue)
        .bind(status)
        .execute(pool)
        .await
        .unwrap();
}

async fn count(pool: &PgPool, table: &'static str) -> i64 {
    // Test-only: callers pass literal table names.
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM {table}")))
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test]
async fn a_bad_or_missing_signature_is_401_and_queues_nothing(pool: PgPool) {
    let s = setup(&pool).await;
    assert_eq!(
        send(&s.app, "wrong-secret", "pull_request", "d1", OPENED).await,
        StatusCode::UNAUTHORIZED
    );
    let unsigned = Request::builder()
        .method("POST")
        .uri("/api/github/webhook")
        .header("x-github-event", "pull_request")
        .header("x-github-delivery", "d2")
        .body(Body::from(OPENED))
        .unwrap();
    assert_eq!(
        s.app.clone().oneshot(unsigned).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(count(&pool, "job").await, 0);
}

#[sqlx::test]
async fn no_configured_secret_is_503(pool: PgPool) {
    let s = setup(&pool).await;
    let app = router(AppState::new(pool.clone()));
    let request = Request::builder()
        .method("POST")
        .uri("/api/github/webhook")
        .header("x-github-event", "pull_request")
        .header("x-github-delivery", "d1")
        .header("x-hub-signature-256", sign(SECRET, OPENED.as_bytes()))
        .body(Body::from(OPENED))
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes).unwrap()["code"],
        "github_not_configured"
    );
    assert_eq!(count(&pool, "job").await, 0);
    drop(s);
}

#[sqlx::test]
async fn an_empty_secret_is_no_secret(pool: PgPool) {
    setup(&pool).await;
    let app = router(AppState::new(pool.clone()).with_github_webhook_secret(""));
    // Signed with the empty key, which anyone could do.
    assert_eq!(
        send(&app, "", "pull_request", "d1", CLOSED).await,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(count(&pool, "job").await, 0);
}

#[sqlx::test]
async fn pings_other_actions_and_unknown_repositories_are_accepted_without_a_job(pool: PgPool) {
    let s = setup(&pool).await;
    let ping = r#"{"zen":"Keep it logically awesome."}"#;
    assert_eq!(
        send(&s.app, SECRET, "ping", "d1", ping).await,
        StatusCode::ACCEPTED
    );
    let labeled = edited(OPENED, |v| v["action"] = json!("labeled"));
    assert_eq!(
        send(&s.app, SECRET, "pull_request", "d2", &labeled).await,
        StatusCode::ACCEPTED
    );
    let elsewhere = edited(OPENED, |v| v["repository"]["name"] = json!("other"));
    assert_eq!(
        send(&s.app, SECRET, "pull_request", "d3", &elsewhere).await,
        StatusCode::ACCEPTED
    );
    // The repository match ignores case.
    let shouting = edited(OPENED, |v| {
        v["repository"]["owner"]["login"] = json!("GannonH")
    });
    assert_eq!(
        send(&s.app, SECRET, "pull_request", "d4", &shouting).await,
        StatusCode::ACCEPTED
    );
    assert_eq!(count(&pool, "job").await, 1);
}

#[sqlx::test]
async fn a_malformed_payload_or_missing_header_is_422(pool: PgPool) {
    let s = setup(&pool).await;
    assert_eq!(
        send(
            &s.app,
            SECRET,
            "pull_request",
            "d1",
            r#"{"action":"opened"}"#
        )
        .await,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let no_delivery = Request::builder()
        .method("POST")
        .uri("/api/github/webhook")
        .header("x-github-event", "pull_request")
        .header("x-hub-signature-256", sign(SECRET, OPENED.as_bytes()))
        .body(Body::from(OPENED))
        .unwrap();
    assert_eq!(
        s.app.clone().oneshot(no_delivery).await.unwrap().status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(count(&pool, "job").await, 0);
}

#[sqlx::test]
async fn an_opened_draft_links_to_its_issue_then_ready_and_checks_update_it(pool: PgPool) {
    let s = setup(&pool).await;
    assert_eq!(create_issue(&s, "Fix the thing").await, "PL-1");

    deliver(&s, "pull_request", "d1", OPENED).await;
    let row: (String, bool, String, Option<String>) =
        sqlx::query_as("SELECT state, draft, checks, issue_id FROM pull_request WHERE number = 12")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        row,
        ("open".into(), true, "none".into(), Some("PL-1".into()))
    );

    deliver(&s, "pull_request", "d2", READY).await;
    deliver(&s, "check_run", "d3", CHECK_RUN).await;
    let row: (bool, String, i32, i32, String) = sqlx::query_as(
        "SELECT draft, checks, checks_total, checks_passed, merge_state
         FROM pull_request WHERE number = 12",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row, (false, "passing".into(), 1, 1, "clean".into()));
    assert_eq!(count(&pool, "job").await, 3);
}

#[sqlx::test]
async fn an_older_delivery_arriving_late_does_not_overwrite_newer_state(pool: PgPool) {
    let s = setup(&pool).await;
    create_issue(&s, "Fix the thing").await;
    deliver(&s, "pull_request", "d1", READY).await;
    deliver(&s, "pull_request", "d2", OPENED).await;
    let draft: bool = sqlx::query_scalar("SELECT draft FROM pull_request WHERE number = 12")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(!draft);
}

#[sqlx::test]
async fn a_failing_check_makes_the_checks_failing(pool: PgPool) {
    let s = setup(&pool).await;
    create_issue(&s, "Fix the thing").await;
    deliver(&s, "pull_request", "d1", OPENED).await;
    deliver(&s, "check_run", "d2", CHECK_RUN).await;
    let failed = edited(CHECK_RUN, |v| {
        v["check_run"]["id"] = json!(4021877999_i64);
        v["check_run"]["name"] = json!("e2e");
        v["check_run"]["conclusion"] = json!("failure");
    });
    deliver(&s, "check_run", "d3", &failed).await;
    let row: (String, i32, i32) = sqlx::query_as(
        "SELECT checks, checks_total, checks_passed FROM pull_request WHERE number = 12",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row, ("failing".into(), 2, 1));
}

#[sqlx::test]
async fn a_merged_pull_request_moves_a_merging_issue_to_done(pool: PgPool) {
    let s = setup(&pool).await;
    let issue = create_issue(&s, "Fix the thing").await;
    force_status(&pool, &issue, "merging").await;

    deliver(&s, "pull_request", "delivery-merged", MERGED).await;

    let status: String = sqlx::query_scalar("SELECT status FROM issue WHERE id = $1")
        .bind(&issue)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "done");
    let row: (String, String, String, String, Option<String>) = sqlx::query_as(
        "SELECT t.from_status, t.to_status, a.role, a.name, t.delivery_id
         FROM transition t JOIN actor a ON a.id = t.actor_id WHERE t.issue_id = $1",
    )
    .bind(&issue)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        row,
        (
            "merging".into(),
            "done".into(),
            "github".into(),
            "octocat".into(),
            Some("delivery-merged".into())
        )
    );
    // The PRD's `merging → done` has no gates.
    assert_eq!(count(&pool, "transition_gate").await, 0);
    let state: String = sqlx::query_scalar("SELECT state FROM pull_request WHERE number = 12")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(state, "merged");
}

#[sqlx::test]
async fn a_merge_while_the_issue_is_elsewhere_changes_no_status(pool: PgPool) {
    let s = setup(&pool).await;
    let issue = create_issue(&s, "Fix the thing").await;
    deliver(&s, "pull_request", "d1", MERGED).await;
    let status: String = sqlx::query_scalar("SELECT status FROM issue WHERE id = $1")
        .bind(&issue)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "backlog");
    assert_eq!(count(&pool, "transition").await, 0);
    assert_eq!(count(&pool, "pull_request").await, 1);
}

#[sqlx::test]
async fn closing_unmerged_moves_to_todo_once_with_one_comment(pool: PgPool) {
    let s = setup(&pool).await;
    let issue = create_issue(&s, "Fix the thing").await;
    force_status(&pool, &issue, "in_progress").await;

    // The same delivery twice: GitHub retries, and the job key makes it one job.
    assert_eq!(
        send(&s.app, SECRET, "pull_request", "dup", CLOSED).await,
        StatusCode::ACCEPTED
    );
    assert_eq!(
        send(&s.app, SECRET, "pull_request", "dup", CLOSED).await,
        StatusCode::ACCEPTED
    );
    assert_eq!(count(&pool, "job").await, 1);
    run_jobs_until_idle(&s.state).await.unwrap();

    let assert_once = |pool: PgPool| async move {
        let transitions: Vec<(String, String)> =
            sqlx::query_as("SELECT from_status, to_status FROM transition WHERE issue_id = 'PL-1'")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(transitions, vec![("in_progress".into(), "todo".into())]);
        let comments: Vec<(String, String)> = sqlx::query_as(
            "SELECT c.body, a.role FROM comment c JOIN actor a ON a.id = c.actor_id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            comments,
            vec![(
                "Pull request #12 was closed without merging by @octocat.\n\n\
                 https://github.com/gannonh/punchlist/pull/12"
                    .into(),
                "github".into()
            )]
        );
        let kinds: Vec<String> =
            sqlx::query_scalar("SELECT kind FROM event WHERE issue_id = 'PL-1' ORDER BY seq")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(kinds, vec!["created", "transition", "comment"]);
    };
    assert_once(pool.clone()).await;

    // The same payload again under a new delivery id: a job runs, and changes nothing.
    deliver(&s, "pull_request", "redelivered", CLOSED).await;
    assert_eq!(count(&pool, "job").await, 2);
    assert_once(pool.clone()).await;

    // The timeline shows the transition and the comment.
    let request = Request::builder()
        .uri(format!("/api/issues/{issue}/events"))
        .header("authorization", format!("Bearer {}", s.person.token))
        .body(Body::empty())
        .unwrap();
    let response = s.app.clone().oneshot(request).await.unwrap();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let events: Vec<Value> = serde_json::from_slice(&bytes)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&bytes)));
    let details: Vec<(&str, &str)> = events
        .iter()
        .map(|e| {
            (
                e["detail"]["kind"].as_str().unwrap(),
                e["actor"]["role"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        details,
        vec![
            ("created", "person"),
            ("transition", "github"),
            ("comment", "github")
        ]
    );
    assert_eq!(events[1]["detail"]["delivery_id"], "dup");
    assert!(
        events[2]["detail"]["body"]
            .as_str()
            .unwrap()
            .contains("@octocat")
    );
}

#[sqlx::test]
async fn a_pull_request_naming_no_issue_is_recorded_unlinked(pool: PgPool) {
    let s = setup(&pool).await;
    create_issue(&s, "Fix the thing").await;
    let stray = edited(OPENED, |v| {
        v["pull_request"]["head"]["ref"] = json!("dependabot/cargo/tokio-1.50");
        v["pull_request"]["title"] = json!("Bump tokio");
    });
    deliver(&s, "pull_request", "d1", &stray).await;
    let row: (i64, Option<String>) = sqlx::query_as("SELECT number, issue_id FROM pull_request")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row, (12, None));

    // A branch that names an issue the workspace does not have is unlinked too.
    let missing = edited(OPENED, |v| {
        v["pull_request"]["number"] = json!(13);
        v["pull_request"]["head"]["ref"] = json!("feature/pl-99-ghost");
    });
    deliver(&s, "pull_request", "d2", &missing).await;
    let linked: Option<String> =
        sqlx::query_scalar("SELECT issue_id FROM pull_request WHERE number = 13")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(linked, None);
}

#[sqlx::test]
async fn review_threads_count_toward_open_threads(pool: PgPool) {
    let s = setup(&pool).await;
    create_issue(&s, "Fix the thing").await;
    let open_threads = || async {
        sqlx::query_scalar::<_, i32>("SELECT open_threads FROM pull_request WHERE number = 12")
            .fetch_one(&pool)
            .await
            .unwrap()
    };

    deliver(&s, "pull_request_review_thread", "d1", THREAD).await;
    assert_eq!(open_threads().await, 0);

    let reopened = edited(THREAD, |v| v["action"] = json!("unresolved"));
    deliver(&s, "pull_request_review_thread", "d2", &reopened).await;
    assert_eq!(open_threads().await, 1);

    let other = edited(THREAD, |v| {
        v["action"] = json!("unresolved");
        v["thread"]["node_id"] = json!("PRRT_kwDOA1b2c84ZyXwV");
    });
    deliver(&s, "pull_request_review_thread", "d3", &other).await;
    assert_eq!(open_threads().await, 2);

    deliver(&s, "pull_request_review_thread", "d4", THREAD).await;
    assert_eq!(open_threads().await, 1);

    // Where a thread is comes from its first comment. A line is there when GitHub sends one.
    let on_line = edited(THREAD, |v| {
        v["thread"]["node_id"] = json!("PRRT_kwDOA1b2c84ZyXwV");
        v["thread"]["comments"][0]["original_line"] = json!(41);
    });
    deliver(&s, "pull_request_review_thread", "d5", &on_line).await;
    let places: Vec<(String, Option<String>, Option<i64>)> =
        sqlx::query_as("SELECT node_id, path, line FROM review_thread ORDER BY node_id")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        places,
        [
            (
                "PRRT_kwDOA1b2c84AbCdE".into(),
                Some("src/lib.rs".into()),
                None
            ),
            (
                "PRRT_kwDOA1b2c84ZyXwV".into(),
                Some("src/lib.rs".into()),
                Some(41)
            ),
        ]
    );
}

#[sqlx::test]
async fn a_thread_event_records_the_comment_author_and_body(pool: PgPool) {
    let s = setup(&pool).await;
    create_issue(&s, "Fix the thing").await;
    let comments = || async {
        sqlx::query_as::<_, (Option<Value>, Option<i32>)>(
            "SELECT comments, comments_total FROM review_thread",
        )
        .fetch_one(&pool)
        .await
        .unwrap()
    };
    let said = json!([
        {"author": "octocat", "body": "Should this return an error?", "cut": false}
    ]);

    let unresolved = edited(THREAD, |v| v["action"] = json!("unresolved"));
    deliver(&s, "pull_request_review_thread", "d1", &unresolved).await;
    assert_eq!(comments().await, (Some(said.clone()), Some(1)));

    // An event without its comments keeps them.
    let bare = edited(THREAD, |v| v["thread"]["comments"] = json!([]));
    deliver(&s, "pull_request_review_thread", "d2", &bare).await;
    assert_eq!(comments().await, (Some(said.clone()), Some(1)));

    // One with them replaces them: the last 10 are kept, and a body over 2000 characters is
    // cut there and marked.
    let long = edited(THREAD, |v| {
        let first = v["thread"]["comments"][0].clone();
        let mut later = first.clone();
        later["body"] = json!("é".repeat(2001));
        let mut all = vec![later; 11];
        all[0] = first;
        v["thread"]["comments"] = json!(all);
    });
    deliver(&s, "pull_request_review_thread", "d3", &long).await;
    let (recorded, total) = comments().await;
    let cut = json!({"author": "octocat", "body": "é".repeat(2000), "cut": true});
    assert_eq!((recorded, total), (Some(json!(vec![cut; 10])), Some(11)));

    // The unresolved thread is read with the issue's pull requests, by the refresh route
    // only, and a resolved one is not. Without a GitHub App nothing is refreshed.
    deliver(&s, "pull_request_review_thread", "d4", &unresolved).await;
    let read = |method: &'static str, uri: &'static str| {
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .header("authorization", format!("Bearer {}", s.person.token))
            .body(Body::empty())
            .unwrap();
        async {
            let response = s.app.clone().oneshot(request).await.unwrap();
            let status = response.status();
            let bytes = response.into_body().collect().await.unwrap().to_bytes();
            (status, serde_json::from_slice::<Value>(&bytes).unwrap())
        }
    };
    let (_, listed) = read("GET", "/api/issues/PL-1/pull-requests").await;
    assert_eq!(listed[0]["number"], 12);
    assert_eq!(listed[0].get("review_threads"), None);
    let (_, now) = read("POST", "/api/issues/PL-1/pull-requests/refresh").await;
    assert_eq!(now["pull_requests"], listed);
    assert_eq!(now["refreshed"], false);
    assert_eq!(
        now["review_threads"],
        json!([{
            "id": "PRRT_kwDOA1b2c84AbCdE",
            "pull_request": 12,
            "path": "src/lib.rs",
            "line": null,
            "comments": said,
            "comments_total": 1,
            "url": "https://github.com/gannonh/punchlist/pull/12#discussion_r1700001",
        }])
    );
    deliver(&s, "pull_request_review_thread", "d5", THREAD).await;
    let (_, now) = read("POST", "/api/issues/PL-1/pull-requests/refresh").await;
    assert_eq!(now["review_threads"], json!([]));

    let (status, _) = read("POST", "/api/issues/PL-9/pull-requests/refresh").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn the_review_threads_route_answers_the_recorded_open_threads_without_github(pool: PgPool) {
    let s = setup(&pool).await;
    create_issue(&s, "Fix the thing").await;
    let token = s.person.token.clone();
    let threads = |app: Router| {
        let token = token.clone();
        async move {
            let request = Request::builder()
                .uri("/api/issues/PL-1/review-threads")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap();
            let response = app.oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let bytes = response.into_body().collect().await.unwrap().to_bytes();
            serde_json::from_slice::<Value>(&bytes).unwrap()
        }
    };

    let unresolved = edited(THREAD, |v| v["action"] = json!("unresolved"));
    deliver(&s, "pull_request_review_thread", "d1", &unresolved).await;
    // Resolved, and on an open pull request: not listed.
    let resolved = edited(THREAD, |v| v["thread"]["node_id"] = json!("PRRT_resolved"));
    deliver(&s, "pull_request_review_thread", "d2", &resolved).await;
    // Unresolved, but on a closed pull request: not listed.
    let closed = pull_request(CLOSED, 13, "feature/pl-1-fix-the-thing", "Old", "punchlist");
    deliver(&s, "pull_request", "d3", &closed).await;
    let on_closed = edited(&unresolved, |v| {
        v["thread"]["node_id"] = json!("PRRT_closed");
        v["pull_request"]["number"] = json!(13);
    });
    deliver(&s, "pull_request_review_thread", "d4", &on_closed).await;
    // An event without comments: recorded with no text and, not being plain, no link.
    let bare = edited(&unresolved, |v| {
        v["thread"]["node_id"] = json!("PRRT_bare");
        v["thread"]["comments"] = json!([]);
    });
    deliver(&s, "pull_request_review_thread", "d5", &bare).await;
    let unsafe_url = edited(&unresolved, |v| {
        v["thread"]["node_id"] = json!("PRRT_unsafe");
        v["thread"]["comments"][0]["html_url"] = json!("javascript:alert(1)");
    });
    deliver(&s, "pull_request_review_thread", "d6", &unsafe_url).await;

    let said = json!([
        {"author": "octocat", "body": "Should this return an error?", "cut": false}
    ]);
    let expected = json!([
        {
            "id": "PRRT_kwDOA1b2c84AbCdE",
            "pull_request": 12,
            "path": "src/lib.rs",
            "line": null,
            "comments": said,
            "comments_total": 1,
            "url": "https://github.com/gannonh/punchlist/pull/12#discussion_r1700001",
        },
        {
            "id": "PRRT_unsafe",
            "pull_request": 12,
            "path": "src/lib.rs",
            "line": null,
            "comments": said,
            "comments_total": 1,
            "url": null,
        },
        {
            "id": "PRRT_bare",
            "pull_request": 12,
            "path": null,
            "line": null,
            "comments": [],
            "comments_total": 0,
            "url": null,
        },
    ]);
    assert_eq!(threads(s.app.clone()).await, expected);
    // A server with a GitHub App it cannot reach answers the same: the route reads no GitHub.
    let unreachable = GithubClient::with_token("http://127.0.0.1:9", "token");
    let with_app = router(s.state.clone().with_github(unreachable));
    assert_eq!(threads(with_app).await, expected);

    // Resolving the thread drops it from the list.
    deliver(&s, "pull_request_review_thread", "d7", THREAD).await;
    let ids: Vec<Value> = (threads(s.app.clone()).await.as_array().unwrap().iter())
        .map(|thread| thread["id"].clone())
        .collect();
    assert_eq!(ids, [json!("PRRT_unsafe"), json!("PRRT_bare")]);

    let (status, body) = get(&s, &s.person.token, "/api/issues/PL-9/review-threads").await;
    let (_, pull_requests) = get(&s, &s.person.token, "/api/issues/PL-9/pull-requests").await;
    assert_eq!((status, &body), (StatusCode::NOT_FOUND, &pull_requests));
}

#[sqlx::test]
async fn a_failing_job_is_retried_with_backoff_then_marked_failed(pool: PgPool) {
    let s = setup(&pool).await;
    sqlx::query(
        "INSERT INTO job (kind, idempotency_key, payload) VALUES ('github_event', 'bad', '{}')",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert!(punchlist_server::run_next_job(&s.state).await.unwrap());
    let row: (String, i32, bool, Option<String>) = sqlx::query_as(
        "SELECT state, attempts, run_after > now(), last_error FROM job WHERE idempotency_key = 'bad'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((row.0.as_str(), row.1, row.2), ("queued", 1, true));
    assert!(row.3.is_some());
    // Not ready until its backoff passes.
    assert!(!punchlist_server::run_next_job(&s.state).await.unwrap());

    sqlx::query("UPDATE job SET attempts = 9, run_after = now()")
        .execute(&pool)
        .await
        .unwrap();
    assert!(punchlist_server::run_next_job(&s.state).await.unwrap());
    let state: String = sqlx::query_scalar("SELECT state FROM job")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(state, "failed");
}

#[sqlx::test]
async fn a_thread_event_does_not_overwrite_a_merged_pull_request(pool: PgPool) {
    let s = setup(&pool).await;
    create_issue(&s, "Fix the thing").await;
    deliver(&s, "pull_request", "d1", MERGED).await;
    // The thread event's short pull request has no `merged`, and here it is newer.
    let later = edited(THREAD, |v| {
        v["pull_request"]["updated_at"] = json!("2026-10-09T19:00:00Z");
    });
    deliver(&s, "pull_request_review_thread", "d2", &later).await;
    let row: (String, i32) =
        sqlx::query_as("SELECT state, open_threads FROM pull_request WHERE number = 12")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((row.0.as_str(), row.1), ("merged", 0));
}

#[sqlx::test]
async fn a_late_created_check_run_does_not_undo_its_completion(pool: PgPool) {
    let s = setup(&pool).await;
    create_issue(&s, "Fix the thing").await;
    deliver(&s, "pull_request", "d1", OPENED).await;
    deliver(&s, "check_run", "d2", CHECK_RUN).await;
    let created = edited(CHECK_RUN, |v| {
        v["action"] = json!("created");
        v["check_run"]["status"] = json!("queued");
        v["check_run"]["conclusion"] = Value::Null;
    });
    deliver(&s, "check_run", "d3", &created).await;
    let run: (String, Option<String>) = sqlx::query_as("SELECT status, conclusion FROM check_run")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        (run.0.as_str(), run.1.as_deref()),
        ("completed", Some("success"))
    );
    let checks: String = sqlx::query_scalar("SELECT checks FROM pull_request WHERE number = 12")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(checks, "passing");
}

#[sqlx::test]
async fn a_merged_pull_request_stays_merged(pool: PgPool) {
    let s = setup(&pool).await;
    create_issue(&s, "Fix the thing").await;
    deliver(&s, "pull_request", "d1", MERGED).await;
    // An open payload with the same or a later `updated_at` arriving after the merge.
    let open = edited(OPENED, |v| {
        v["pull_request"]["updated_at"] = json!("2026-10-09T18:30:00Z");
    });
    deliver(&s, "pull_request", "d2", &open).await;
    let state: String = sqlx::query_scalar("SELECT state FROM pull_request WHERE number = 12")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(state, "merged");
}

#[sqlx::test]
async fn a_rerun_that_passes_replaces_the_failure_it_reran(pool: PgPool) {
    let s = setup(&pool).await;
    create_issue(&s, "Fix the thing").await;
    deliver(&s, "pull_request", "d1", OPENED).await;
    let failed = edited(CHECK_RUN, |v| {
        v["check_run"]["conclusion"] = json!("failure")
    });
    deliver(&s, "check_run", "d2", &failed).await;
    let summary = || async {
        sqlx::query_as::<_, (String, i32, i32)>(
            "SELECT checks, checks_total, checks_passed FROM pull_request WHERE number = 12",
        )
        .fetch_one(&pool)
        .await
        .unwrap()
    };
    assert_eq!(summary().await, ("failing".into(), 1, 0));

    // GitHub gives the rerun a new id, with the same name on the same commit.
    let rerun = edited(CHECK_RUN, |v| v["check_run"]["id"] = json!(4021878000_i64));
    deliver(&s, "check_run", "d3", &rerun).await;
    assert_eq!(summary().await, ("passing".into(), 1, 1));
}

#[sqlx::test]
async fn a_late_queued_check_run_does_not_undo_in_progress(pool: PgPool) {
    let s = setup(&pool).await;
    create_issue(&s, "Fix the thing").await;
    deliver(&s, "pull_request", "d1", OPENED).await;
    let with_status = |action: &str, status: &str| {
        edited(CHECK_RUN, |v| {
            v["action"] = json!(action);
            v["check_run"]["status"] = json!(status);
            v["check_run"]["conclusion"] = Value::Null;
        })
    };
    deliver(
        &s,
        "check_run",
        "d2",
        &with_status("created", "in_progress"),
    )
    .await;
    deliver(&s, "check_run", "d3", &with_status("created", "queued")).await;
    let status: String = sqlx::query_scalar("SELECT status FROM check_run")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "in_progress");
}

#[sqlx::test]
async fn a_thread_event_first_records_a_merged_pull_request_as_merged(pool: PgPool) {
    let s = setup(&pool).await;
    create_issue(&s, "Fix the thing").await;
    let merged = edited(THREAD, |v| {
        v["pull_request"]["state"] = json!("closed");
        v["pull_request"]["merged_at"] = json!("2026-10-09T18:30:00Z");
    });
    deliver(&s, "pull_request_review_thread", "d1", &merged).await;
    let state: String = sqlx::query_scalar("SELECT state FROM pull_request WHERE number = 12")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(state, "merged");
}

#[sqlx::test]
async fn a_thread_event_first_to_record_a_close_moves_the_issue_once(pool: PgPool) {
    let s = setup(&pool).await;
    let issue = create_issue(&s, "Fix the thing").await;
    force_status(&pool, &issue, "in_progress").await;
    let closed = edited(THREAD, |v| v["pull_request"]["state"] = json!("closed"));

    // The thread event does not say who closed it; the `closed` delivery then adds nothing.
    deliver(&s, "pull_request_review_thread", "d-thread", &closed).await;
    deliver(&s, "pull_request", "d-closed", CLOSED).await;

    let transitions: Vec<(String, String, String, Option<String>)> = sqlx::query_as(
        "SELECT t.from_status, t.to_status, a.name, t.delivery_id
         FROM transition t JOIN actor a ON a.id = t.actor_id WHERE t.issue_id = $1",
    )
    .bind(&issue)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        transitions,
        [(
            "in_progress".into(),
            "todo".into(),
            "GitHub".into(),
            Some("d-thread".into())
        )]
    );
    let comments: Vec<String> = sqlx::query_scalar("SELECT body FROM comment")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(
        comments,
        ["Pull request #12 was closed without merging.\n\n\
          https://github.com/gannonh/punchlist/pull/12"]
    );
}

#[sqlx::test]
async fn redelivering_a_delivery_whose_job_failed_runs_it_again(pool: PgPool) {
    let s = setup(&pool).await;
    create_issue(&s, "Fix the thing").await;
    deliver(&s, "pull_request", "flaky", OPENED).await;
    // The job gave up, and its effects are gone.
    sqlx::query(
        "UPDATE job SET state = 'failed', attempts = 10, last_error = 'database down',
                finished_at = now()",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("DELETE FROM pull_request")
        .execute(&pool)
        .await
        .unwrap();

    deliver(&s, "pull_request", "flaky", OPENED).await;
    let job: (String, i32, Option<String>) = sqlx::query_as(
        "SELECT state, attempts, last_error FROM job WHERE idempotency_key = 'flaky'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(job, ("done".into(), 1, None));
    assert_eq!(count(&pool, "job").await, 1);
    assert_eq!(count(&pool, "pull_request").await, 1);
}

#[sqlx::test]
async fn closing_unmerged_ends_the_running_run_so_the_issue_can_be_claimed_again(pool: PgPool) {
    let s = setup(&pool).await;
    let issue = create_issue(&s, "Fix the thing").await;
    force_status(&pool, &issue, "in_progress").await;
    sqlx::query(
        "WITH a AS (INSERT INTO actor (workspace_id, name, role, token_hash)
                    SELECT workspace_id, 'r1', 'runner', 'runner-token' FROM actor LIMIT 1
                    RETURNING id, workspace_id),
              r AS (INSERT INTO runner (workspace_id, actor_id, name, agents, registered_by)
                    SELECT workspace_id, id, 'r1', '{claude-code}', id FROM a
                    RETURNING id, workspace_id),
              u AS (INSERT INTO run (workspace_id, issue_id, status, agent, branch)
                    SELECT workspace_id, $1, 'in_progress', 'claude-code', 'b' FROM r
                    RETURNING id)
         INSERT INTO attempt (run_id, issue_id, runner_id, attempt, lease_expires_at)
         SELECT u.id, $1, r.id, 1, now() + interval '1 hour' FROM u, r",
    )
    .bind(&issue)
    .execute(&pool)
    .await
    .unwrap();

    deliver(&s, "pull_request", "closed", CLOSED).await;

    let run: (String, bool) =
        sqlx::query_as("SELECT outcome, finished_at IS NOT NULL FROM run WHERE issue_id = $1")
            .bind(&issue)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(run, ("failed".into(), true));
    let attempt: (String, Option<String>) =
        sqlx::query_as("SELECT state, failure_reason FROM attempt WHERE issue_id = $1")
            .bind(&issue)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        attempt,
        (
            "failed".into(),
            Some(
                "the issue moved to todo because pull request #12 was closed without merging"
                    .into()
            )
        )
    );
}

/// The person asks to move `issue` to `to`; returns the status the answer shows.
async fn move_to(s: &Setup, issue: &str, to: &str) -> String {
    let request = Request::builder()
        .method("POST")
        .uri(format!("/api/issues/{issue}/transitions"))
        .header("authorization", format!("Bearer {}", s.person.token))
        .header("content-type", "application/json")
        .body(Body::from(json!({ "to": to }).to_string()))
        .unwrap();
    let response = s.app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice::<Value>(&bytes).unwrap()["issue"]["status"]
        .as_str()
        .unwrap()
        .to_string()
}

async fn status(pool: &PgPool, issue: &str) -> String {
    sqlx::query_scalar("SELECT status FROM issue WHERE id = $1")
        .bind(issue)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Each transition of `issue`, oldest first: from, to, the actor's role and name.
async fn transitions(pool: &PgPool, issue: &str) -> Vec<(String, String, String, String)> {
    sqlx::query_as(
        "SELECT t.from_status, t.to_status, a.role, a.name
         FROM transition t JOIN actor a ON a.id = t.actor_id
         WHERE t.issue_id = $1 ORDER BY t.created_at",
    )
    .bind(issue)
    .fetch_all(pool)
    .await
    .unwrap()
}

fn row(from: &str, to: &str, role: &str, name: &str) -> (String, String, String, String) {
    (from.into(), to.into(), role.into(), name.into())
}

#[sqlx::test]
async fn an_issue_entering_merging_after_its_merge_moves_to_done(pool: PgPool) {
    let s = setup(&pool).await;
    let issue = create_issue(&s, "Fix the thing").await;
    force_status(&pool, &issue, "human_review").await;
    deliver(&s, "pull_request", "d1", MERGED).await;
    assert_eq!(status(&pool, &issue).await, "human_review");

    // The answer is the person's move; the catch-up follows it in the same transaction.
    assert_eq!(move_to(&s, &issue, "merging").await, "merging");
    assert_eq!(status(&pool, &issue).await, "done");
    assert_eq!(
        transitions(&pool, &issue).await,
        [
            row("human_review", "merging", "person", "Gannon"),
            row("merging", "done", "github", "GitHub"),
        ]
    );
}

#[sqlx::test]
async fn an_issue_entering_merging_with_its_pull_request_open_waits_for_the_merge(pool: PgPool) {
    let s = setup(&pool).await;
    let issue = create_issue(&s, "Fix the thing").await;
    force_status(&pool, &issue, "human_review").await;
    deliver(&s, "pull_request", "d1", READY).await;

    assert_eq!(move_to(&s, &issue, "merging").await, "merging");
    assert_eq!(status(&pool, &issue).await, "merging");

    deliver(&s, "pull_request", "d2", MERGED).await;
    assert_eq!(status(&pool, &issue).await, "done");
    assert_eq!(
        transitions(&pool, &issue).await,
        [
            row("human_review", "merging", "person", "Gannon"),
            row("merging", "done", "github", "octocat"),
        ]
    );
}

#[sqlx::test]
async fn an_older_merged_pull_request_does_not_move_an_issue_whose_newer_one_is_open(pool: PgPool) {
    let s = setup(&pool).await;
    let issue = create_issue(&s, "Fix the thing").await;
    force_status(&pool, &issue, "human_review").await;
    deliver(&s, "pull_request", "d1", MERGED).await;
    let newer = edited(READY, |v| {
        v["number"] = json!(13);
        v["pull_request"]["number"] = json!(13);
        v["pull_request"]["id"] = json!(1934102999_i64);
        v["pull_request"]["head"]["ref"] = json!("feature/pl-1-fix-the-thing-2");
        v["pull_request"]["html_url"] = json!("https://github.com/gannonh/punchlist/pull/13");
    });
    deliver(&s, "pull_request", "d2", &newer).await;
    let states: Vec<(i64, String)> =
        sqlx::query_as("SELECT number, state FROM pull_request ORDER BY number")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(states, [(12, "merged".into()), (13, "open".into())]);

    assert_eq!(move_to(&s, &issue, "merging").await, "merging");
    assert_eq!(status(&pool, &issue).await, "merging");
    assert_eq!(
        transitions(&pool, &issue).await,
        [row("human_review", "merging", "person", "Gannon")]
    );
}

#[sqlx::test]
async fn an_issue_in_merging_with_its_pull_request_merged_moves_to_done_at_start(pool: PgPool) {
    let s = setup(&pool).await;
    let waiting = create_issue(&s, "Fix the thing").await;
    let open = create_issue(&s, "Another thing").await;
    force_status(&pool, &waiting, "human_review").await;
    deliver(&s, "pull_request", "d1", MERGED).await;
    // Moved before the catch-up was deployed: no catch-up ran on its way in.
    force_status(&pool, &waiting, "merging").await;
    force_status(&pool, &open, "merging").await;

    punchlist_server::catch_up_at_start(&pool).await.unwrap();
    assert_eq!(status(&pool, &waiting).await, "done");
    assert_eq!(status(&pool, &open).await, "merging");
    assert_eq!(
        transitions(&pool, &waiting).await,
        [row("merging", "done", "github", "GitHub")]
    );

    // A second start writes nothing.
    punchlist_server::catch_up_at_start(&pool).await.unwrap();
    assert_eq!(count(&pool, "transition").await, 1);
}

/// A GET with a bearer token: the status and the JSON body.
async fn get(s: &Setup, token: &str, uri: &str) -> (StatusCode, Value) {
    let request = Request::builder()
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let response = s.app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// `fixture` as pull request `number` on `branch` of `gannonh/<repository>`, titled `title`.
fn pull_request(fixture: &str, number: i64, branch: &str, title: &str, repository: &str) -> String {
    edited(fixture, |v| {
        v["number"] = json!(number);
        v["pull_request"]["number"] = json!(number);
        v["pull_request"]["head"]["ref"] = json!(branch);
        v["pull_request"]["title"] = json!(title);
        v["pull_request"]["html_url"] = json!(format!(
            "https://github.com/gannonh/{repository}/pull/{number}"
        ));
        v["repository"]["name"] = json!(repository);
        v["repository"]["full_name"] = json!(format!("gannonh/{repository}"));
    })
}

#[sqlx::test]
async fn open_pull_requests_list_the_workspaces_open_and_draft_ones_with_what_they_wait_on(
    pool: PgPool,
) {
    let s = setup(&pool).await;
    bootstrap(
        &pool,
        &Bootstrap {
            workspace_name: "Other".into(),
            issue_prefix: "OT".into(),
            repository_owner: "gannonh".into(),
            repository_name: "other".into(),
            person_name: "Someone".into(),
        },
    )
    .await
    .unwrap();
    for title in ["Merge me", "Review me", "Draft in merging", "Gone"] {
        create_issue(&s, title).await;
    }
    force_status(&pool, "PL-1", "merging").await;
    force_status(&pool, "PL-2", "human_review").await;
    force_status(&pool, "PL-3", "merging").await;

    let deliveries = [
        pull_request(READY, 1, "feature/pl-1-a", "Merge me", "punchlist"),
        pull_request(READY, 2, "feature/pl-2-b", "Review me", "punchlist"),
        pull_request(OPENED, 3, "feature/pl-3-c", "Draft in merging", "punchlist"),
        pull_request(MERGED, 4, "feature/pl-4-d", "Merged", "punchlist"),
        pull_request(CLOSED, 5, "feature/pl-4-e", "Closed", "punchlist"),
        pull_request(READY, 6, "stray", "Bump deps", "punchlist"),
        pull_request(READY, 7, "feature/ot-1-x", "Elsewhere", "other"),
    ];
    for (n, body) in deliveries.iter().enumerate() {
        deliver(&s, "pull_request", &format!("d{n}"), body).await;
    }
    assert_eq!(count(&pool, "pull_request").await, 7);
    sqlx::query(
        "UPDATE pull_request SET updated_at = '2026-10-09T18:00:00Z',
             url = CASE number WHEN 2 THEN 'javascript:alert(1)' ELSE url END",
    )
    .execute(&pool)
    .await
    .unwrap();

    let (status, body) = get(&s, &s.person.token, "/api/pull-requests").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let issue = |id: &str, title: &str, status: &str, name: &str| json!({"id": id, "title": title, "status": status, "status_name": name});
    let item = |number: i64, title: &str, branch: &str, draft: bool, url: Value, issue: Value| {
        json!({
            "number": number, "title": title, "branch": branch, "url": url, "draft": draft,
            "merge_state": if draft { "unknown" } else { "clean" },
            "checks_passed": 0, "checks_total": 0, "open_threads": 0,
            "updated_at": "2026-10-09T18:00:00Z", "issue": issue,
            "waits_on_merge": number == 1,
        })
    };
    let url = |n: i64| json!(format!("https://github.com/gannonh/punchlist/pull/{n}"));
    assert_eq!(
        body,
        json!([
            item(6, "Bump deps", "stray", false, url(6), Value::Null),
            item(
                3,
                "Draft in merging",
                "feature/pl-3-c",
                true,
                url(3),
                issue("PL-3", "Draft in merging", "merging", "Merging")
            ),
            item(
                2,
                "Review me",
                "feature/pl-2-b",
                false,
                Value::Null,
                issue("PL-2", "Review me", "human_review", "Human Review")
            ),
            item(
                1,
                "Merge me",
                "feature/pl-1-a",
                false,
                url(1),
                issue("PL-1", "Merge me", "merging", "Merging")
            ),
        ])
    );

    let agent = agent_token(&s).await;
    let (status, body) = get(&s, &agent, "/api/pull-requests").await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
}

/// A standalone agent's token.
async fn agent_token(s: &Setup) -> String {
    let request = Request::builder()
        .method("POST")
        .uri("/api/actors")
        .header("authorization", format!("Bearer {}", s.person.token))
        .header("content-type", "application/json")
        .body(Body::from(json!({ "kind": "codex" }).to_string()))
        .unwrap();
    let response = s.app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice::<Value>(&bytes).unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string()
}
