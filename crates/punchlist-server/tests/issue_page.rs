//! What the issue page reads: the transitions list and its verdicts, which must be what a
//! request then answers, the session a standalone agent's token reads, and each attempt's
//! model. End to end against the router.

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
const READY: &str = include_str!("fixtures/github/pull_request_ready_for_review.json");
const CHECK_RUN: &str = include_str!("fixtures/github/check_run_completed.json");

struct Setup {
    state: AppState,
    app: Router,
    person: Bootstrapped,
    agent: String,
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
    let app = router(state.clone());
    let (status, created) = call(
        &app,
        &person.token,
        "POST",
        "/api/actors",
        Some(json!({"kind": "codex"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let agent = created["token"].as_str().unwrap().to_string();
    Setup {
        state,
        app,
        person,
        agent,
    }
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

/// A signed fixture delivery, `edit`ed, run to idle.
async fn deliver(
    s: &Setup,
    event: &str,
    delivery: &str,
    fixture: &str,
    edit: impl FnOnce(&mut Value),
) {
    let mut payload: Value = serde_json::from_str(fixture).unwrap();
    edit(&mut payload);
    let body = payload.to_string();
    let mut mac = Hmac::<Sha256>::new_from_slice(SECRET.as_bytes()).unwrap();
    mac.update(body.as_bytes());
    let hex: String = (mac.finalize().into_bytes().iter())
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let request = Request::builder()
        .method("POST")
        .uri("/api/github/webhook")
        .header("content-type", "application/json")
        .header("x-github-event", event)
        .header("x-github-delivery", delivery)
        .header("x-hub-signature-256", format!("sha256={hex}"))
        .body(Body::from(body))
        .unwrap();
    let response = s.app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    run_jobs_until_idle(&s.state).await.unwrap();
}

#[derive(Clone, Copy, Debug)]
enum Evidence {
    None,
    /// A ready pull request whose one check failed.
    FailingCheck,
    /// A ready, clean pull request whose one check passed, and proof for the attempt.
    Passing,
}

/// A new issue in `status`, with `evidence` recorded for it.
async fn issue_in(s: &Setup, pool: &PgPool, status: &str, evidence: Evidence) -> String {
    let (_, created) = call(
        &s.app,
        &s.person.token,
        "POST",
        "/api/issues",
        Some(json!({"title": "Fix the thing"})),
    )
    .await;
    let id = created["row"]["issue"]["id"].as_str().unwrap().to_string();
    sqlx::query("UPDATE issue SET status = $2 WHERE id = $1")
        .bind(&id)
        .bind(status)
        .execute(pool)
        .await
        .unwrap();
    let (conclusion, proof) = match evidence {
        Evidence::None => return id,
        Evidence::FailingCheck => ("failure", false),
        Evidence::Passing => ("success", true),
    };
    let number: i64 = id.trim_start_matches("PL-").parse().unwrap();
    let branch = format!("feature/{}-fix", id.to_lowercase());
    let sha = format!("{number:040x}");
    deliver(s, "pull_request", &format!("pr-{id}"), READY, |p| {
        p["pull_request"]["number"] = json!(number);
        p["pull_request"]["head"]["ref"] = json!(branch);
        p["pull_request"]["head"]["sha"] = json!(sha);
        p["pull_request"]["html_url"] = json!(format!(
            "https://github.com/gannonh/punchlist/pull/{number}"
        ));
    })
    .await;
    deliver(s, "check_run", &format!("check-{id}"), CHECK_RUN, |p| {
        p["check_run"]["head_sha"] = json!(sha);
        p["check_run"]["conclusion"] = json!(conclusion);
        p["check_run"]["pull_requests"][0]["number"] = json!(number);
        p["check_run"]["pull_requests"][0]["head"]["ref"] = json!(branch);
        p["check_run"]["pull_requests"][0]["head"]["sha"] = json!(sha);
    })
    .await;
    if proof {
        sqlx::query(
            "INSERT INTO proof (issue_id, attempt, kind, title, content_type, size, storage_key, actor_id)
             VALUES ($1, 0, 'image', 'Screenshot', 'image/png', 1, $1, $2)",
        )
        .bind(&id)
        .bind(s.person.actor_id)
        .execute(pool)
        .await
        .unwrap();
    }
    id
}

/// One listed transition: its target and its verdict, as `kind`, then the gate or the code.
fn summary(option: &Value) -> (String, String) {
    let verdict = &option["verdict"];
    let detail = match verdict["kind"].as_str().unwrap() {
        "allowed" => String::new(),
        "blocked" => verdict["gate"].as_str().unwrap().to_string(),
        "refused" => verdict["code"].as_str().unwrap().to_string(),
        other => panic!("unknown verdict {other}"),
    };
    (
        option["to"]["key"].as_str().unwrap().to_string(),
        format!("{} {detail}", verdict["kind"].as_str().unwrap())
            .trim()
            .to_string(),
    )
}

#[sqlx::test]
async fn each_listed_verdict_is_what_the_request_answers(pool: PgPool) {
    let s = setup(&pool).await;
    use Evidence::*;
    // Status, evidence, then the listing for a person and for a standalone agent.
    let cases: &[(&str, Evidence, &str, &str)] = &[
        (
            "backlog",
            None,
            "todo allowed",
            "todo refused role_not_allowed",
        ),
        (
            "todo",
            None,
            "start allowed",
            "start refused role_not_allowed",
        ),
        (
            "start",
            None,
            "in_progress refused role_not_allowed",
            "in_progress refused role_not_allowed",
        ),
        (
            "in_progress",
            None,
            "agent_review refused role_not_allowed",
            "agent_review blocked pr_open",
        ),
        (
            "agent_review",
            FailingCheck,
            "human_review refused role_not_allowed",
            "human_review blocked ci_green",
        ),
        (
            "agent_review",
            Passing,
            "human_review refused role_not_allowed",
            "human_review allowed",
        ),
        (
            "human_review",
            None,
            "merging allowed",
            "merging refused locked",
        ),
        (
            "merging",
            None,
            "todo allowed",
            "todo refused role_not_allowed",
        ),
        ("done", None, "", ""),
    ];
    for &(status, evidence, for_person, for_agent) in cases {
        for (role, token, expected) in [
            ("person", s.person.token.clone(), for_person),
            ("agent", s.agent.clone(), for_agent),
        ] {
            let id = issue_in(&s, &pool, status, evidence).await;
            let (code, list) = call(
                &s.app,
                &token,
                "GET",
                &format!("/api/issues/{id}/transitions"),
                Option::None,
            )
            .await;
            assert_eq!(code, StatusCode::OK, "{list}");
            assert_eq!(list["status"]["key"], status);
            let options = list["transitions"].as_array().unwrap();
            let listed: Vec<String> = (options.iter())
                .map(|option| {
                    let (to, verdict) = summary(option);
                    format!("{to} {verdict}")
                })
                .collect();
            assert_eq!(
                listed.join(", "),
                expected,
                "{status} {evidence:?} as {role}"
            );

            // Each verdict is what the request answers.
            for option in options {
                let to = option["to"]["key"].as_str().unwrap();
                let (code, answer) = call(
                    &s.app,
                    &token,
                    "POST",
                    &format!("/api/issues/{id}/transitions"),
                    Some(json!({"to": to})),
                )
                .await;
                let verdict = &option["verdict"];
                match verdict["kind"].as_str().unwrap() {
                    "allowed" => {
                        assert_eq!(code, StatusCode::OK, "{answer}");
                        assert_eq!(answer["issue"]["status"], to);
                    }
                    "blocked" => {
                        assert_eq!(code, StatusCode::CONFLICT, "{answer}");
                        assert_eq!(answer["code"], "gate_failed");
                        assert_eq!(answer["gate"], verdict["gate"]);
                        let names = verdict["outside"].as_array().map(|names| {
                            let names: Vec<&str> =
                                names.iter().map(|n| n.as_str().unwrap()).collect();
                            names.join(", ")
                        });
                        let text = match names {
                            Some(names) => {
                                format!("{}: {names}", verdict["reason"].as_str().unwrap())
                            }
                            Option::None => verdict["reason"].as_str().unwrap().to_string(),
                        };
                        assert_eq!(answer["reason"], json!(text));
                    }
                    _ => {
                        assert_eq!(code, StatusCode::CONFLICT, "{answer}");
                        assert_eq!(answer["code"], verdict["code"]);
                        assert_eq!(answer["message"], verdict["message"]);
                    }
                }
            }
        }
    }
}

#[sqlx::test]
async fn a_failing_check_blocks_by_name_and_a_locked_status_locks_the_agent(pool: PgPool) {
    let s = setup(&pool).await;
    let id = issue_in(&s, &pool, "agent_review", Evidence::FailingCheck).await;
    let (_, list) = call(
        &s.app,
        &s.agent,
        "GET",
        &format!("/api/issues/{id}/transitions"),
        None,
    )
    .await;
    assert_eq!(
        list,
        json!({
            "status": {"key": "agent_review", "name": "Agent Review", "locked_to": [],
                       "dispatch": "codex", "dispatch_runs_per_entry": 3, "closed": false},
            "viewer_locked": false,
            "transitions": [{
                "to": {"key": "human_review", "name": "Human Review",
                       "locked_to": ["agent", "runner"], "dispatch": null, "closed": false},
                "by": ["agent"],
                "verdict": {"kind": "blocked", "gate": "ci_green", "passed": false,
                            "reason": "1 of 1 checks failed on 0000000",
                            "outside": ["`check` (failure)"]},
            }],
        })
    );
    // The pull request names the check, for the checks table.
    let (_, pull_requests) = call(
        &s.app,
        &s.agent,
        "GET",
        &format!("/api/issues/{id}/pull-requests"),
        None,
    )
    .await;
    assert_eq!(
        pull_requests[0]["check_runs"],
        json!([{"name": "check", "status": "completed", "conclusion": "failure",
                "app": "github-actions"}])
    );

    let locked = issue_in(&s, &pool, "human_review", Evidence::None).await;
    let (_, list) = call(
        &s.app,
        &s.agent,
        "GET",
        &format!("/api/issues/{locked}/transitions"),
        None,
    )
    .await;
    assert_eq!(list["viewer_locked"], true);
    assert_eq!(list["status"]["locked_to"], json!(["agent", "runner"]));
    assert_eq!(
        list["transitions"][0]["verdict"],
        json!({"kind": "refused", "code": "locked",
               "message": "human_review is locked: an agent cannot act on an issue in it"})
    );
}

#[sqlx::test]
async fn a_standalone_agent_reads_its_session_and_cannot_sign_in(pool: PgPool) {
    let s = setup(&pool).await;
    let (status, session) = call(&s.app, &s.agent, "GET", "/api/session", None).await;
    assert_eq!(status, StatusCode::OK, "{session}");
    assert_eq!(session["actor"]["role"], "agent");
    assert_eq!(session["actor"]["kind"], "codex");
    assert_eq!(session["actor"]["name"], "Codex");
    assert_eq!(session["workspace_id"], json!(s.person.workspace_id));
    // Signing out needs no token at all.
    let (status, _) = call(&s.app, &s.agent, "DELETE", "/api/session", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    // A browser cannot sign in with it.
    let (status, _) = call(
        &s.app,
        "",
        "POST",
        "/api/session",
        Some(json!({"token": s.agent})),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[sqlx::test]
async fn an_attempt_records_the_model_its_runner_registered(pool: PgPool) {
    let s = setup(&pool).await;
    let id = issue_in(&s, &pool, "start", Evidence::None).await;
    let (status, runner) = call(
        &s.app,
        &s.person.token,
        "POST",
        "/api/runners",
        Some(json!({"name": "sartre", "agents": ["claude-code"],
                    "models": {"claude-code": "haiku"}})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{runner}");
    let token = runner["token"].as_str().unwrap();
    let (status, claim) = call(&s.app, token, "POST", "/api/runs/claim", Some(json!({}))).await;
    assert_eq!(status, StatusCode::OK, "{claim}");
    let (_, runs) = call(
        &s.app,
        &s.agent,
        "GET",
        &format!("/api/issues/{id}/runs"),
        None,
    )
    .await;
    assert_eq!(runs[0]["model"], "haiku");
    assert_eq!(runs[0]["attempts"][0]["model"], "haiku");
    assert_eq!(runs[0]["attempts"][0]["runner"]["name"], "sartre");
}
