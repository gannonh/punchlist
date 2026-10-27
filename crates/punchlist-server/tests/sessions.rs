//! `/api/sessions` (PL-168): a person lists the account's browser sessions and ends one, or
//! every one but the browser they are in. Another account's session is 404; agents get 403.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use punchlist_server::{AppState, Bootstrap, Bootstrapped, bootstrap, join, router};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

const IPHONE: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Mobile/15E148 Safari/604.1";
const MAC_CHROME: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";

fn input(prefix: &str) -> Bootstrap {
    Bootstrap {
        workspace_name: prefix.into(),
        issue_prefix: prefix.into(),
        repository_owner: "gannonh".into(),
        repository_name: format!("repo-{prefix}"),
        person_name: "Gannon".into(),
    }
}

async fn workspace(pool: &PgPool, prefix: &str) -> Bootstrapped {
    bootstrap(pool, &input(prefix)).await.unwrap()
}

/// A request as the browser on `localhost` sends it, or as `pl` with a token.
fn request(method: &str, uri: &str, auth: Auth, body: Option<Value>) -> Request<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "localhost")
        .header("content-type", "application/json");
    request = match auth {
        Auth::Cookie(cookie) => request
            .header("cookie", cookie)
            .header("origin", "http://localhost"),
        Auth::Token(token) => request.header("authorization", format!("Bearer {token}")),
    };
    request
        .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
        .unwrap()
}

#[derive(Clone, Copy)]
enum Auth<'a> {
    Cookie(&'a str),
    Token(&'a str),
}

async fn send(app: &Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// Signs in with the token from a browser with `user_agent`: its `Cookie` header.
async fn sign_in(app: &Router, token: &str, user_agent: &str) -> String {
    let request = Request::builder()
        .method("POST")
        .uri("/api/session")
        .header("host", "localhost")
        .header("origin", "http://localhost")
        .header("content-type", "application/json")
        .header("user-agent", user_agent)
        .body(Body::from(json!({ "token": token }).to_string()))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = response.headers()["set-cookie"].to_str().unwrap();
    cookie.split(';').next().unwrap().to_string()
}

/// The sessions' `(device, current)`, in the answer's order, and their ids.
async fn list(app: &Router, auth: Auth<'_>) -> (Vec<(String, bool)>, Vec<String>) {
    let (status, body) = send(app, request("GET", "/api/sessions", auth, None)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let sessions = body.as_array().unwrap();
    let shown = (sessions.iter())
        .map(|s| {
            let keys: Vec<&str> = s.as_object().unwrap().keys().map(String::as_str).collect();
            assert_eq!(
                keys,
                ["created_at", "current", "device", "id", "last_used_at"]
            );
            (
                s["device"].as_str().unwrap().to_string(),
                s["current"].as_bool().unwrap(),
            )
        })
        .collect();
    let ids = (sessions.iter())
        .map(|s| s["id"].as_str().unwrap().to_string())
        .collect();
    (shown, ids)
}

/// Whether the cookie still signs the browser in.
async fn works(app: &Router, cookie: &str) -> StatusCode {
    send(
        app,
        request("GET", "/api/session", Auth::Cookie(cookie), None),
    )
    .await
    .0
}

/// Moves a session's last use into the past, so the order is the one the test sets.
async fn used_ago(pool: &PgPool, user_agent: &str, minutes: i32) {
    sqlx::query(
        "UPDATE session SET last_used_at = now() - make_interval(mins => $2) WHERE user_agent = $1",
    )
    .bind(user_agent)
    .bind(minutes)
    .execute(pool)
    .await
    .unwrap();
}

#[sqlx::test]
async fn a_person_lists_their_sessions_newest_use_first_with_the_current_one(pool: PgPool) {
    let person = workspace(&pool, "PL").await;
    let app = router(AppState::new(pool.clone()));
    let phone = sign_in(&app, &person.token, IPHONE).await;
    let desk = sign_in(&app, &person.token, MAC_CHROME).await;
    let shell = sign_in(&app, &person.token, "curl/8.0").await;
    used_ago(&pool, IPHONE, 5).await;
    used_ago(&pool, MAC_CHROME, 10).await;
    used_ago(&pool, "curl/8.0", 20).await;

    // The request is a use of its own session, so the current one is newest.
    let (shown, ids) = list(&app, Auth::Cookie(&desk)).await;
    assert_eq!(
        shown,
        [
            ("Chrome on macOS".to_string(), true),
            ("Safari on iPhone".to_string(), false),
            ("Unknown device".to_string(), false),
        ]
    );
    // The id is the public one, not the cookie's value or its hash.
    let public: Vec<String> =
        sqlx::query_scalar("SELECT public_id::text FROM session ORDER BY last_used_at DESC")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(ids, public);
    for cookie in [&phone, &desk, &shell] {
        let value = cookie.strip_prefix("punchlist_session=").unwrap();
        assert!(!ids.iter().any(|id| id == value));
    }
    // With a token, no session is the current one.
    let (shown, _) = list(&app, Auth::Token(&person.token)).await;
    assert!(shown.iter().all(|(_, current)| !current), "{shown:?}");
    // A session past its expiry is not listed.
    sqlx::query("UPDATE session SET expires_at = now() WHERE user_agent = 'curl/8.0'")
        .execute(&pool)
        .await
        .unwrap();
    let (shown, _) = list(&app, Auth::Cookie(&phone)).await;
    assert_eq!(
        shown,
        [
            ("Safari on iPhone".to_string(), true),
            ("Chrome on macOS".to_string(), false),
        ]
    );
}

#[sqlx::test]
async fn revoking_one_session_ends_that_browser_alone(pool: PgPool) {
    let person = workspace(&pool, "PL").await;
    let app = router(AppState::new(pool.clone()));
    let phone = sign_in(&app, &person.token, IPHONE).await;
    let desk = sign_in(&app, &person.token, MAC_CHROME).await;
    used_ago(&pool, IPHONE, 5).await;
    let (shown, ids) = list(&app, Auth::Cookie(&desk)).await;
    assert_eq!(shown[1], ("Safari on iPhone".to_string(), false));

    let uri = format!("/api/sessions/{}", ids[1]);
    let (status, body) = send(&app, request("DELETE", &uri, Auth::Cookie(&desk), None)).await;
    assert_eq!((status, body), (StatusCode::NO_CONTENT, Value::Null));
    assert_eq!(works(&app, &phone).await, StatusCode::UNAUTHORIZED);
    assert_eq!(works(&app, &desk).await, StatusCode::OK);
    let (shown, _) = list(&app, Auth::Cookie(&desk)).await;
    assert_eq!(shown, [("Chrome on macOS".to_string(), true)]);
    // Again: it is gone.
    let (status, body) = send(&app, request("DELETE", &uri, Auth::Cookie(&desk), None)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["message"], format!("session {} not found", ids[1]));
}

#[sqlx::test]
async fn revoking_all_others_keeps_the_current_session(pool: PgPool) {
    let person = workspace(&pool, "PL").await;
    let app = router(AppState::new(pool.clone()));
    let phone = sign_in(&app, &person.token, IPHONE).await;
    let desk = sign_in(&app, &person.token, MAC_CHROME).await;
    let shell = sign_in(&app, &person.token, "curl/8.0").await;

    let (status, body) = send(
        &app,
        request("DELETE", "/api/sessions", Auth::Cookie(&desk), None),
    )
    .await;
    assert_eq!((status, body), (StatusCode::OK, json!({"revoked": 2})));
    assert_eq!(works(&app, &phone).await, StatusCode::UNAUTHORIZED);
    assert_eq!(works(&app, &shell).await, StatusCode::UNAUTHORIZED);
    assert_eq!(works(&app, &desk).await, StatusCode::OK);
    let stored: Vec<String> = sqlx::query_scalar("SELECT user_agent FROM session")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(stored, [MAC_CHROME]);

    // With a token there is no current session: `pl session revoke --all-others` ends them all.
    let (status, body) = send(
        &app,
        request("DELETE", "/api/sessions", Auth::Token(&person.token), None),
    )
    .await;
    assert_eq!((status, body), (StatusCode::OK, json!({"revoked": 1})));
    assert_eq!(works(&app, &desk).await, StatusCode::UNAUTHORIZED);
}

#[sqlx::test]
async fn another_accounts_session_is_not_found_and_keeps_working(pool: PgPool) {
    let person = workspace(&pool, "PL").await;
    let other = workspace(&pool, "GW").await;
    let app = router(AppState::new(pool.clone()));
    let mine = sign_in(&app, &person.token, MAC_CHROME).await;
    let theirs = sign_in(&app, &other.token, IPHONE).await;
    let (_, their_ids) = list(&app, Auth::Cookie(&theirs)).await;

    let uri = format!("/api/sessions/{}", their_ids[0]);
    let (status, body) = send(&app, request("DELETE", &uri, Auth::Cookie(&mine), None)).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["code"], "not_found");
    assert_eq!(works(&app, &theirs).await, StatusCode::OK);
    // Nor does `revoke all others` reach it, or the list show it.
    let (status, body) = send(
        &app,
        request("DELETE", "/api/sessions", Auth::Cookie(&mine), None),
    )
    .await;
    assert_eq!((status, body), (StatusCode::OK, json!({"revoked": 0})));
    assert_eq!(works(&app, &theirs).await, StatusCode::OK);
    let (shown, _) = list(&app, Auth::Cookie(&mine)).await;
    assert_eq!(shown, [("Chrome on macOS".to_string(), true)]);
    // An id that is no session at all is 404 too.
    let uri = format!("/api/sessions/{}", Uuid::new_v4());
    let (status, _) = send(&app, request("DELETE", &uri, Auth::Cookie(&mine), None)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn an_account_in_several_workspaces_needs_none_named(pool: PgPool) {
    let person = workspace(&pool, "PL").await;
    join(&pool, person.account_id, &input("GW")).await.unwrap();
    let app = router(AppState::new(pool.clone()));
    let desk = sign_in(&app, &person.token, MAC_CHROME).await;
    let (shown, ids) = list(&app, Auth::Cookie(&desk)).await;
    assert_eq!(shown, [("Chrome on macOS".to_string(), true)]);
    let uri = format!("/api/sessions/{}", ids[0]);
    let (status, _) = send(
        &app,
        request("DELETE", &uri, Auth::Token(&person.token), None),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[sqlx::test]
async fn an_agents_token_is_refused(pool: PgPool) {
    let person = workspace(&pool, "PL").await;
    let app = router(AppState::new(pool.clone()));
    let desk = sign_in(&app, &person.token, MAC_CHROME).await;
    let (status, created) = send(
        &app,
        request(
            "POST",
            "/api/actors",
            Auth::Token(&person.token),
            Some(json!({"kind": "codex"})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let agent = created["token"].as_str().unwrap();
    let (_, ids) = list(&app, Auth::Cookie(&desk)).await;

    for (method, uri) in [
        ("GET", "/api/sessions".to_string()),
        ("DELETE", "/api/sessions".to_string()),
        ("DELETE", format!("/api/sessions/{}", ids[0])),
    ] {
        let (status, body) = send(&app, request(method, &uri, Auth::Token(agent), None)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}: {body}");
        assert_eq!(
            body["message"],
            "an agent acts only on the workspace's issues"
        );
    }
    assert_eq!(works(&app, &desk).await, StatusCode::OK);
}
