//! `/api/cli-logins` (PL-170): `pl auth login` asks for a login, a signed-in browser approves
//! or denies it, and the poll answers a CLI session once, which works as a bearer value. A
//! browser session's id does not, and a CLI session is no cookie.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use punchlist_server::{AppState, Bootstrap, bootstrap, router};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

const PL_AGENT: &str = "pl/0.1.0 (sartre)";

#[derive(Clone, Copy)]
enum Auth<'a> {
    None,
    Cookie(&'a str),
    Bearer(&'a str),
}

async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    auth: Auth<'_>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("user-agent", PL_AGENT);
    request = match auth {
        Auth::None => request,
        Auth::Cookie(cookie) => request
            .header("cookie", cookie)
            .header("origin", "http://localhost"),
        Auth::Bearer(value) => request.header("authorization", format!("Bearer {value}")),
    };
    let request = request
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

struct Setup {
    app: Router,
    pool: PgPool,
    token: String,
    /// The browser's `Cookie` header and its session id.
    cookie: String,
}

async fn setup(pool: PgPool, state: impl FnOnce(AppState) -> AppState) -> Setup {
    let person = bootstrap(
        &pool,
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
    let app = router(state(AppState::new(pool.clone())));
    let request = Request::builder()
        .method("POST")
        .uri("/api/session")
        .header("host", "localhost")
        .header("origin", "http://localhost")
        .header("content-type", "application/json")
        .body(Body::from(json!({ "token": person.token }).to_string()))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = response.headers()["set-cookie"].to_str().unwrap();
    let cookie = cookie.split(';').next().unwrap().to_string();
    Setup {
        app,
        pool,
        token: person.token,
        cookie,
    }
}

/// Starts a login: its code and secret.
async fn start(app: &Router) -> (String, String) {
    let (status, body) = send(app, "POST", "/api/cli-logins", Auth::None, None).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let code = body["code"].as_str().unwrap().to_string();
    assert_eq!(
        body["url"],
        format!("http://localhost/cli-login?code={code}")
    );
    assert_eq!(body["expires_in"], 600);
    (code, body["device_secret"].as_str().unwrap().to_string())
}

async fn poll(app: &Router, secret: &str) -> (StatusCode, Value) {
    let body = json!({ "device_secret": secret });
    send(app, "POST", "/api/cli-logins/token", Auth::None, Some(body)).await
}

async fn decide(s: &Setup, code: &str, decision: &str) -> (StatusCode, Value) {
    let uri = format!("/api/cli-logins/{code}/{decision}");
    send(&s.app, "POST", &uri, Auth::Cookie(&s.cookie), None).await
}

async fn session_kinds(pool: &PgPool) -> Vec<String> {
    sqlx::query_scalar("SELECT kind FROM session ORDER BY created_at")
        .fetch_all(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrator = "punchlist_server::MIGRATOR")]
async fn an_approved_login_answers_a_cli_session_once(pool: PgPool) {
    let s = setup(pool, |state| state).await;
    let (code, secret) = start(&s.app).await;
    assert_eq!(code.len(), 8);

    // The approval page reads it, then the person approves.
    let (status, info) = send(
        &s.app,
        "GET",
        &format!("/api/cli-logins/{}", code.to_lowercase()),
        Auth::Cookie(&s.cookie),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        info,
        json!({ "code": code, "device": "pl on sartre", "state": "pending" })
    );
    assert_eq!(
        poll(&s.app, &secret).await,
        (StatusCode::OK, json!({ "state": "pending" }))
    );
    assert_eq!(decide(&s, &code, "approve").await.0, StatusCode::NO_CONTENT);

    let (status, body) = poll(&s.app, &secret).await;
    assert_eq!(
        (status, &body["state"]),
        (StatusCode::OK, &json!("approved"))
    );
    let token = body["token"].as_str().unwrap().to_string();
    assert_eq!(session_kinds(&s.pool).await, ["browser", "cli"]);

    // Once only: the poll answers the session once, and the code is approved once.
    let (status, again) = poll(&s.app, &secret).await;
    assert_eq!(status, StatusCode::CONFLICT, "{again}");
    assert_eq!(
        again["message"],
        "this login's session was already answered"
    );
    let (status, twice) = decide(&s, &code, "approve").await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(
        twice["message"],
        format!("login {code} was already approved")
    );
    assert_eq!(session_kinds(&s.pool).await, ["browser", "cli"]);

    // The session works as `pl`'s bearer value, and shows in the list as `pl on sartre`,
    // the current one, 90 days from now.
    let (status, issues) = send(&s.app, "GET", "/api/issues", Auth::Bearer(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{issues}");
    let (_, sessions) = send(&s.app, "GET", "/api/sessions", Auth::Bearer(&token), None).await;
    let shown: Vec<(&str, bool)> = (sessions.as_array().unwrap().iter())
        .map(|s| {
            (
                s["device"].as_str().unwrap(),
                s["current"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(shown, [("pl on sartre", true), ("Unknown device", false)]);
    let days: f64 = sqlx::query_scalar(
        "SELECT (extract(epoch FROM expires_at - now()) / 86400)::float8 FROM session WHERE kind = 'cli'",
    )
    .fetch_one(&s.pool)
    .await
    .unwrap();
    assert!((89.9..=90.0).contains(&days), "{days}");

    // Ending it from the browser makes the next call 401.
    let cli = (sessions.as_array().unwrap().iter())
        .find(|s| s["current"] == true)
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let uri = format!("/api/sessions/{cli}");
    let (status, _) = send(&s.app, "DELETE", &uri, Auth::Cookie(&s.cookie), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = send(&s.app, "GET", "/api/issues", Auth::Bearer(&token), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrator = "punchlist_server::MIGRATOR")]
async fn a_denied_login_makes_no_session(pool: PgPool) {
    let s = setup(pool, |state| state).await;
    let (code, secret) = start(&s.app).await;
    assert_eq!(decide(&s, &code, "deny").await.0, StatusCode::NO_CONTENT);
    assert_eq!(
        poll(&s.app, &secret).await,
        (StatusCode::OK, json!({ "state": "denied" }))
    );
    let (status, body) = decide(&s, &code, "approve").await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["message"], format!("login {code} was already denied"));
    assert_eq!(session_kinds(&s.pool).await, ["browser"]);
}

#[sqlx::test(migrator = "punchlist_server::MIGRATOR")]
async fn an_expired_login_cannot_be_approved(pool: PgPool) {
    let s = setup(pool, |state| {
        state.with_cli_login_lifetime(std::time::Duration::ZERO)
    })
    .await;
    let (status, body) = send(&s.app, "POST", "/api/cli-logins", Auth::None, None).await;
    assert_eq!(
        (status, &body["expires_in"]),
        (StatusCode::CREATED, &json!(0))
    );
    let code = body["code"].as_str().unwrap();
    let secret = body["device_secret"].as_str().unwrap();
    let (status, body) = decide(&s, code, "approve").await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["message"], format!("login {code} expired"));
    assert_eq!(
        poll(&s.app, secret).await,
        (StatusCode::OK, json!({ "state": "expired" }))
    );
    assert_eq!(session_kinds(&s.pool).await, ["browser"]);
}

#[sqlx::test(migrator = "punchlist_server::MIGRATOR")]
async fn a_wrong_secret_or_code_is_404(pool: PgPool) {
    let s = setup(pool, |state| state).await;
    start(&s.app).await;
    let (status, body) = poll(&s.app, "not-the-secret").await;
    assert_eq!(
        (status, &body["message"]),
        (StatusCode::NOT_FOUND, &json!("no login has this secret"))
    );
    let (status, body) = decide(&s, "BCDFGHJK", "approve").await;
    assert_eq!(
        (status, &body["message"]),
        (StatusCode::NOT_FOUND, &json!("login BCDFGHJK not found"))
    );
}

#[sqlx::test(migrator = "punchlist_server::MIGRATOR")]
async fn only_a_person_approves(pool: PgPool) {
    let s = setup(pool, |state| state).await;
    let (code, _) = start(&s.app).await;
    let uri = format!("/api/cli-logins/{code}/approve");
    let (status, _) = send(&s.app, "POST", &uri, Auth::None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // A person's token approves too; an agent's is refused.
    let (status, agent) = send(
        &s.app,
        "POST",
        "/api/actors",
        Auth::Bearer(&s.token),
        Some(json!({ "kind": "codex" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{agent}");
    let agent = agent["token"].as_str().unwrap();
    let (status, _) = send(&s.app, "POST", &uri, Auth::Bearer(agent), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send(&s.app, "POST", &uri, Auth::Bearer(&s.token), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

/// A browser's session id is a cookie only and `pl`'s a bearer value only.
#[sqlx::test(migrator = "punchlist_server::MIGRATOR")]
async fn each_session_kind_works_only_its_own_way(pool: PgPool) {
    let s = setup(pool, |state| state).await;
    let browser_id = s.cookie.strip_prefix("punchlist_session=").unwrap();
    let (status, _) = send(&s.app, "GET", "/api/issues", Auth::Bearer(browser_id), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (code, secret) = start(&s.app).await;
    decide(&s, &code, "approve").await;
    let token = poll(&s.app, &secret).await.1["token"]
        .as_str()
        .unwrap()
        .to_string();
    let as_cookie = format!("punchlist_session={token}");
    let (status, _) = send(&s.app, "GET", "/api/issues", Auth::Cookie(&as_cookie), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // The account's own token still works beside it.
    let (status, _) = send(&s.app, "GET", "/api/issues", Auth::Bearer(&s.token), None).await;
    assert_eq!(status, StatusCode::OK);
}
