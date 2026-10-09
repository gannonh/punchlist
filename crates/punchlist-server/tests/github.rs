//! GitHub webhook deliveries end to end: signed requests against the router, the jobs they
//! queue, and the rows the jobs write, read back from Postgres.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use hmac::{Hmac, KeyInit, Mac};
use http_body_util::BodyExt;
use punchlist_server::{AppState, Bootstrap, Bootstrapped, bootstrap, router, run_jobs_until_idle};
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
    serde_json::from_slice::<Value>(&bytes).unwrap()["id"]
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
        assert_eq!(kinds, vec!["transition", "comment"]);
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
        vec![("transition", "github"), ("comment", "github")]
    );
    assert_eq!(events[0]["detail"]["delivery_id"], "dup");
    assert!(
        events[1]["detail"]["body"]
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
