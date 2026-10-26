//! Browser sign-in (ADR 0013): token sign-in and sign-in with GitHub both start a server-side
//! session, the cookie holds its id and no token, and a write carrying it must come from the
//! instance's own origin. GitHub is a fake served on a local port.

use axum::Router;
use axum::body::Body;
use axum::extract::Json;
use axum::http::{HeaderMap, Request, StatusCode};
use axum::routing::{get, post};
use http_body_util::BodyExt;
use punchlist_server::{
    AppState, Bootstrap, Bootstrapped, GithubSignIn, bootstrap, hash_token, router,
};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

const ORIGIN: &str = "http://localhost";

async fn workspace(pool: &PgPool, prefix: &str) -> Bootstrapped {
    let input = Bootstrap {
        workspace_name: prefix.into(),
        issue_prefix: prefix.into(),
        repository_owner: "gannonh".into(),
        repository_name: format!("repo-{prefix}"),
        person_name: "Gannon".into(),
    };
    bootstrap(pool, &input).await.unwrap()
}

/// GitHub's two OAuth endpoints and `GET /user`, for the users `octocat` and `stranger`.
async fn fake_github() -> String {
    async fn access_token(Json(body): Json<Value>) -> Json<Value> {
        let login = (body["code"].as_str()).and_then(|code| code.strip_prefix("code-"));
        match login {
            Some(login)
                if body["client_id"] == "client-id" && body["client_secret"] == "secret" =>
            {
                Json(json!({"access_token": format!("token-{login}"), "token_type": "bearer"}))
            }
            _ => Json(json!({"error": "bad_verification_code"})),
        }
    }
    async fn user(headers: HeaderMap) -> Result<Json<Value>, StatusCode> {
        let token = headers["authorization"].to_str().unwrap();
        match token.strip_prefix("Bearer token-") {
            Some("octocat") => Ok(Json(json!({"id": 583231, "login": "octocat", "name": "x"}))),
            Some("stranger") => Ok(Json(json!({"id": 9001, "login": "stranger"}))),
            _ => Err(StatusCode::UNAUTHORIZED),
        }
    }
    let app = Router::new()
        .route("/login/oauth/access_token", post(access_token))
        .route("/user", get(user));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

async fn app_with_github(pool: &PgPool) -> Router {
    let github = fake_github().await;
    router(
        AppState::new(pool.clone()).with_github_sign_in(GithubSignIn {
            client_id: "client-id".into(),
            client_secret: "secret".into(),
            oauth_url: github.clone(),
            api_url: github,
        }),
    )
}

/// A request to the app on `localhost`, as a browser there sends it: `headers` are the cookie,
/// the origin and the like.
fn browser(
    method: &str,
    uri: &str,
    headers: &[(&str, &str)],
    body: Option<Value>,
) -> Request<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "localhost")
        .header("content-type", "application/json");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    request
        .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
        .unwrap()
}

async fn send(app: &Router, request: Request<Body>) -> (StatusCode, HeaderMap, Value) {
    let response = app.clone().oneshot(request).await.unwrap();
    let (status, headers) = (response.status(), response.headers().clone());
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, headers, json)
}

/// The values the answer's `Set-Cookie` headers give the named cookie.
fn set_cookies<'a>(headers: &'a HeaderMap, name: &str) -> Vec<&'a str> {
    (headers.get_all("set-cookie").iter())
        .filter_map(|value| value.to_str().ok()?.strip_prefix(name)?.strip_prefix('='))
        .collect()
}

/// The session id of the answer's session cookie.
fn session_of(headers: &HeaderMap) -> String {
    let [cookie] = set_cookies(headers, "punchlist_session")[..] else {
        panic!("no session cookie in {headers:?}");
    };
    cookie.split(';').next().unwrap().to_string()
}

/// Token sign-in: the session cookie's `Cookie` header.
async fn sign_in(app: &Router, token: &str) -> String {
    let request = browser(
        "POST",
        "/api/session",
        &[("origin", ORIGIN), ("user-agent", "Firefox")],
        Some(json!({"token": token})),
    );
    let (status, headers, body) = send(app, request).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    format!("punchlist_session={}", session_of(&headers))
}

async fn sessions(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM session")
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn github_of(pool: &PgPool, account: Uuid) -> (Option<i64>, Option<String>) {
    sqlx::query_as("SELECT github_user_id, github_login FROM account WHERE id = $1")
        .bind(account)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// `start` with `cookie`, then GitHub's redirect back as `login`: the callback's answer.
async fn github_flow(
    app: &Router,
    cookie: &str,
    link: bool,
    login: &str,
) -> (StatusCode, HeaderMap, Value) {
    let uri = if link {
        "/api/auth/github/start?link=1"
    } else {
        "/api/auth/github/start"
    };
    let (status, headers, body) = send(app, browser("GET", uri, &[("cookie", cookie)], None)).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    let location = reqwest::Url::parse(headers["location"].to_str().unwrap()).unwrap();
    assert_eq!(location.path(), "/login/oauth/authorize");
    let query: std::collections::HashMap<_, _> = location.query_pairs().into_owned().collect();
    assert_eq!(query["client_id"], "client-id");
    assert_eq!(
        query["redirect_uri"],
        "http://localhost/api/auth/github/callback"
    );
    let [flow] = set_cookies(&headers, "punchlist_github")[..] else {
        panic!("no state cookie in {headers:?}");
    };
    assert!(
        flow.ends_with("; HttpOnly; SameSite=Lax; Path=/api/auth/github; Max-Age=600"),
        "{flow}"
    );
    let flow = flow.split(';').next().unwrap();
    let cookies = format!("{cookie}; punchlist_github={flow}");
    let callback = format!(
        "/api/auth/github/callback?code=code-{login}&state={}",
        query["state"]
    );
    send(
        app,
        browser("GET", &callback, &[("cookie", &cookies)], None),
    )
    .await
}

#[sqlx::test]
async fn token_sign_in_starts_a_session_and_the_cookie_holds_no_token(pool: PgPool) {
    let person = workspace(&pool, "PL").await;
    let app = router(AppState::new(pool.clone()));
    let request = browser(
        "POST",
        "/api/session",
        &[],
        Some(json!({"token": person.token})),
    );
    let (status, headers, session) = send(&app, request).await;
    assert_eq!(status, StatusCode::OK, "{session}");
    let id = session_of(&headers);
    assert_eq!(
        headers["set-cookie"],
        format!("punchlist_session={id}; HttpOnly; SameSite=Lax; Path=/")
    );
    assert_eq!(id.len(), 43, "32 bytes, base64url: {id}");
    assert_ne!(id, person.token);
    let expected = json!({
        "actor": {"id": person.actor_id, "name": "Gannon", "role": "person"},
        "workspace_id": person.workspace_id,
        "account": {"id": person.account_id, "name": "Gannon"},
    });
    assert_eq!(session, expected);
    // The table holds the id's hash, and no account's token hashes to the cookie's value.
    let stored: Vec<(String, Uuid)> = sqlx::query_as("SELECT id, account_id FROM session")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(stored, [(hash_token(&id), person.account_id)]);
    let tokens: i64 = sqlx::query_scalar("SELECT count(*) FROM account WHERE token_hash = $1")
        .bind(hash_token(&id))
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(tokens, 0);
    // Over https the cookie is `Secure`.
    let https = browser(
        "POST",
        "/api/session",
        &[("x-forwarded-proto", "https")],
        Some(json!({"token": person.token})),
    );
    let (_, headers, _) = send(&app, https).await;
    assert!(
        headers["set-cookie"]
            .to_str()
            .unwrap()
            .ends_with("; Path=/; Secure")
    );

    let cookie = format!("theme=dark; punchlist_session={id}");
    let read = |uri: &str| browser("GET", uri, &[("cookie", &cookie)], None);
    let (status, _, me) = send(&app, read("/api/session")).await;
    assert_eq!((status, me), (StatusCode::OK, expected));
    let (status, _, issues) = send(&app, read("/api/issues")).await;
    assert_eq!(
        (status, issues),
        (StatusCode::OK, json!({"issues": [], "last_event_seq": 0}))
    );
    // A token in the cookie is no session.
    let token_cookie = format!("punchlist_session={}", person.token);
    let request = browser("GET", "/api/session", &[("cookie", &token_cookie)], None);
    assert_eq!(send(&app, request).await.0, StatusCode::UNAUTHORIZED);
    // A runner's token still signs nothing in.
    let (_, _, runner) = send(
        &app,
        browser(
            "POST",
            "/api/runners",
            &[("authorization", &format!("Bearer {}", person.token))],
            Some(json!({"name": "sartre", "agents": ["claude-code"]})),
        ),
    )
    .await;
    let request = browser(
        "POST",
        "/api/session",
        &[],
        Some(json!({"token": runner["token"]})),
    );
    assert_eq!(send(&app, request).await.0, StatusCode::UNAUTHORIZED);

    // Deleting the row signs the browser out on its next request.
    let second = sign_in(&app, &person.token).await;
    sqlx::query("DELETE FROM session WHERE id = $1")
        .bind(hash_token(&id))
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        send(&app, read("/api/session")).await.0,
        StatusCode::UNAUTHORIZED
    );
    // Signing out deletes the cookie's row and clears the cookie.
    let request = browser(
        "DELETE",
        "/api/session",
        &[("cookie", &second), ("origin", ORIGIN)],
        None,
    );
    let (status, headers, _) = send(&app, request).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        headers["set-cookie"],
        "punchlist_session=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0"
    );
    // The https sign-in's session is left.
    assert_eq!(sessions(&pool).await, 1);
    let request = browser("GET", "/api/session", &[("cookie", &second)], None);
    assert_eq!(send(&app, request).await.0, StatusCode::UNAUTHORIZED);
}

#[sqlx::test]
async fn a_session_lasts_thirty_days_from_its_last_use(pool: PgPool) {
    let person = workspace(&pool, "PL").await;
    let app = router(AppState::new(pool.clone()));
    let cookie = sign_in(&app, &person.token).await;
    let user_agent: Option<String> = sqlx::query_scalar("SELECT user_agent FROM session")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(user_agent.as_deref(), Some("Firefox"));
    let read = || browser("GET", "/api/session", &[("cookie", &cookie)], None);

    sqlx::query("UPDATE session SET last_used_at = now() - interval '10 days', expires_at = now() + interval '20 days'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(send(&app, read()).await.0, StatusCode::OK);
    let days_left: f64 = sqlx::query_scalar(
        "SELECT extract(epoch FROM expires_at - now())::float8 / 86400 FROM session",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(days_left > 29.9, "{days_left}");

    sqlx::query("UPDATE session SET expires_at = now() - interval '1 second'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(send(&app, read()).await.0, StatusCode::UNAUTHORIZED);
}

#[sqlx::test]
async fn a_write_with_the_session_cookie_must_come_from_the_instances_origin(pool: PgPool) {
    let person = workspace(&pool, "PL").await;
    let app = router(AppState::new(pool.clone()));
    let cookie = sign_in(&app, &person.token).await;
    let bearer = format!("Bearer {}", person.token);
    let create = |headers: &[(&str, &str)]| {
        browser(
            "POST",
            "/api/issues",
            headers,
            Some(json!({"title": "Hello"})),
        )
    };
    let status = |request| {
        let app = app.clone();
        async move { send(&app, request).await.0 }
    };

    let refused = create(&[("cookie", &cookie), ("origin", "https://example.com")]);
    let (code, _, body) = send(&app, refused).await;
    assert_eq!(
        (code, body),
        (
            StatusCode::FORBIDDEN,
            json!({"code": "forbidden", "message": "refused a write from another origin"})
        )
    );
    assert_eq!(
        status(create(&[("cookie", &cookie)])).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        status(create(&[("cookie", &cookie), ("origin", "null")])).await,
        StatusCode::FORBIDDEN
    );
    // The proxy's host counts as the request's own.
    let proxied = create(&[
        ("cookie", &cookie),
        ("origin", "https://proxy.example"),
        ("x-forwarded-host", "proxy.example"),
    ]);
    assert_eq!(status(proxied).await, StatusCode::CREATED);
    assert_eq!(
        status(create(&[("cookie", &cookie), ("origin", ORIGIN)])).await,
        StatusCode::CREATED
    );
    // `pl` and runners send a bearer token and no `Origin`.
    assert_eq!(
        status(create(&[("authorization", &bearer)])).await,
        StatusCode::CREATED
    );
    assert_eq!(
        status(create(&[
            ("authorization", &bearer),
            ("origin", "https://example.com")
        ]))
        .await,
        StatusCode::FORBIDDEN
    );
    // Reads are not writes.
    let read = browser(
        "GET",
        "/api/issues",
        &[("cookie", &cookie), ("origin", "https://example.com")],
        None,
    );
    assert_eq!(status(read).await, StatusCode::OK);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM issue")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 3);

    // With a public URL, only it.
    let app = router(AppState::new(pool.clone()).with_public_url("https://punchlist.example/"));
    let request = create(&[("cookie", &cookie), ("origin", ORIGIN)]);
    assert_eq!(send(&app, request).await.0, StatusCode::FORBIDDEN);
    let request = create(&[("cookie", &cookie), ("origin", "https://punchlist.example")]);
    assert_eq!(send(&app, request).await.0, StatusCode::CREATED);
}

#[sqlx::test]
async fn a_person_links_github_then_signs_in_with_it(pool: PgPool) {
    let person = workspace(&pool, "PL").await;
    let app = app_with_github(&pool).await;
    let (_, _, options) = send(&app, browser("GET", "/api/auth", &[], None)).await;
    assert_eq!(options, json!({"github": true}));
    // A link needs a session.
    let request = browser("GET", "/api/auth/github/start?link=1", &[], None);
    assert_eq!(send(&app, request).await.0, StatusCode::UNAUTHORIZED);

    let cookie = sign_in(&app, &person.token).await;
    let (status, headers, body) = github_flow(&app, &cookie, true, "octocat").await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    assert_eq!(headers["location"], "/");
    assert_eq!(
        set_cookies(&headers, "punchlist_github"),
        ["; HttpOnly; SameSite=Lax; Path=/api/auth/github; Max-Age=0"]
    );
    assert!(set_cookies(&headers, "punchlist_session").is_empty());
    assert_eq!(
        github_of(&pool, person.account_id).await,
        (Some(583231), Some("octocat".into()))
    );
    let (_, _, session) = send(
        &app,
        browser("GET", "/api/session", &[("cookie", &cookie)], None),
    )
    .await;
    assert_eq!(
        session["account"],
        json!({"id": person.account_id, "name": "Gannon", "github_login": "octocat"})
    );

    // A fresh browser signs in with GitHub alone.
    let before = sessions(&pool).await;
    let (status, headers, body) = github_flow(&app, "", false, "octocat").await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    assert_eq!(headers["location"], "/");
    let id = session_of(&headers);
    assert_eq!(sessions(&pool).await, before + 1);
    let cookie = format!("punchlist_session={id}");
    let (status, _, session) = send(
        &app,
        browser("GET", "/api/session", &[("cookie", &cookie)], None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(session["actor"]["id"], json!(person.actor_id));
}

#[sqlx::test]
async fn an_unlinked_github_user_is_sent_to_the_sign_in_page(pool: PgPool) {
    let person = workspace(&pool, "PL").await;
    let app = app_with_github(&pool).await;
    let cookie = sign_in(&app, &person.token).await;
    github_flow(&app, &cookie, true, "octocat").await;
    let before = sessions(&pool).await;

    let (status, headers, _) = github_flow(&app, "", false, "stranger").await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(headers["location"], "/sign-in?unlinked=stranger");
    assert!(set_cookies(&headers, "punchlist_session").is_empty());
    assert_eq!(sessions(&pool).await, before);
}

#[sqlx::test]
async fn a_callback_without_its_state_is_refused(pool: PgPool) {
    let person = workspace(&pool, "PL").await;
    let app = app_with_github(&pool).await;
    let cookie = sign_in(&app, &person.token).await;
    let before = sessions(&pool).await;
    let (_, headers, _) = send(
        &app,
        browser(
            "GET",
            "/api/auth/github/start?link=1",
            &[("cookie", &cookie)],
            None,
        ),
    )
    .await;
    let flow = set_cookies(&headers, "punchlist_github")[0]
        .split(';')
        .next()
        .unwrap();
    let state = flow.split('.').next().unwrap();
    let callback = |state: &str, cookies: &str| {
        let uri = format!("/api/auth/github/callback?code=code-octocat&state={state}");
        browser("GET", &uri, &[("cookie", cookies)], None)
    };
    let with_flow = format!("{cookie}; punchlist_github={flow}");
    let other = sign_in(&app, &person.token).await;
    for request in [
        callback("tampered", &with_flow),
        callback(state, &cookie),
        browser(
            "GET",
            "/api/auth/github/callback?code=code-octocat",
            &[("cookie", &with_flow)],
            None,
        ),
        // A link is for the session that started it.
        callback(state, &format!("{other}; punchlist_github={flow}")),
    ] {
        let (status, _, body) = send(&app, request).await;
        assert_eq!(
            (status, &body["code"]),
            (StatusCode::BAD_REQUEST, &json!("bad_request")),
            "{body}"
        );
    }
    assert_eq!(github_of(&pool, person.account_id).await, (None, None));
    assert_eq!(sessions(&pool).await, before + 1);
}

#[sqlx::test]
async fn a_github_user_links_to_one_account(pool: PgPool) {
    let first = workspace(&pool, "PL").await;
    let second = workspace(&pool, "GW").await;
    let app = app_with_github(&pool).await;
    let cookie = sign_in(&app, &first.token).await;
    github_flow(&app, &cookie, true, "octocat").await;

    let cookie = sign_in(&app, &second.token).await;
    let (status, _, body) = github_flow(&app, &cookie, true, "octocat").await;
    assert_eq!(
        (status, body),
        (
            StatusCode::CONFLICT,
            json!({"code": "conflict", "message": "GitHub user octocat is linked to another Punchlist account"})
        )
    );
    assert_eq!(github_of(&pool, second.account_id).await, (None, None));
    assert_eq!(
        github_of(&pool, first.account_id).await,
        (Some(583231), Some("octocat".into()))
    );
}

#[sqlx::test]
async fn without_the_apps_client_there_is_no_github_sign_in(pool: PgPool) {
    workspace(&pool, "PL").await;
    let app = router(AppState::new(pool.clone()));
    let (_, _, options) = send(&app, browser("GET", "/api/auth", &[], None)).await;
    assert_eq!(options, json!({"github": false}));
    for uri in [
        "/api/auth/github/start",
        "/api/auth/github/callback?code=x&state=y",
    ] {
        let (status, _, body) = send(&app, browser("GET", uri, &[], None)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    }
}
