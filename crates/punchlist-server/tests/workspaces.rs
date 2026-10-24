//! One account in several workspaces (ADR 0011): HTTP requests against the router with the
//! account's token, then the rows they wrote, read back from Postgres.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use punchlist_server::{AppState, Bootstrap, Bootstrapped, bootstrap, join, router};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

fn input(name: &str, prefix: &str) -> Bootstrap {
    Bootstrap {
        workspace_name: name.into(),
        issue_prefix: prefix.into(),
        repository_owner: "gannonh".into(),
        repository_name: prefix.to_lowercase(),
        person_name: "Gannon".into(),
    }
}

async fn call(
    app: &Router,
    auth: (&str, &str),
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header(auth.0, auth.1)
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

/// The account in `PL` and `GW`, an issue in each, and `OT`, a workspace of another account,
/// with an issue.
struct Setup {
    app: Router,
    account: Bootstrapped,
    gw: Uuid,
    gw_person: Uuid,
    other: Bootstrapped,
}

async fn setup(pool: &PgPool) -> Setup {
    let app = router(AppState::new(pool.clone()));
    let account = bootstrap(pool, &input("Punchlist", "PL")).await.unwrap();
    let (gw, gw_person) = join(pool, account.account_id, &input("Groundwork", "GW"))
        .await
        .unwrap();
    let other = bootstrap(pool, &input("Other", "OT")).await.unwrap();
    let bearer = format!("Bearer {}", account.token);
    for (workspace, title) in [
        (account.workspace_id, "In Punchlist"),
        (gw, "In Groundwork"),
    ] {
        let uri = format!("/api/workspaces/{workspace}/issues");
        let (status, body) = call(
            &app,
            ("authorization", &bearer),
            "POST",
            &uri,
            Some(json!({"title": title})),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }
    let other_bearer = format!("Bearer {}", other.token);
    let (status, body) = call(
        &app,
        ("authorization", &other_bearer),
        "POST",
        "/api/issues",
        Some(json!({"title": "Not theirs"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    Setup {
        app,
        account,
        gw,
        gw_person,
        other,
    }
}

fn ids(list: &Value) -> Vec<&str> {
    (list["issues"].as_array().unwrap().iter())
        .map(|row| row["issue"]["id"].as_str().unwrap())
        .collect()
}

#[sqlx::test]
async fn one_token_reaches_each_workspace_of_its_account_and_no_other(pool: PgPool) {
    let s = setup(&pool).await;
    let bearer = format!("Bearer {}", s.account.token);
    let auth = ("authorization", bearer.as_str());

    let (status, workspaces) = call(&s.app, auth, "GET", "/api/workspaces", None).await;
    assert_eq!(status, StatusCode::OK, "{workspaces}");
    assert_eq!(
        workspaces,
        json!([
            {"id": s.account.workspace_id, "name": "Punchlist", "issue_prefix": "PL", "repository": "gannonh/pl"},
            {"id": s.gw, "name": "Groundwork", "issue_prefix": "GW", "repository": "gannonh/gw"},
        ])
    );

    let uri = format!("/api/workspaces/{}/issues", s.gw);
    let (status, list) = call(&s.app, auth, "GET", &uri, None).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(ids(&list), ["GW-1"]);
    let uri = format!("/api/workspaces/{}/issues", s.account.workspace_id);
    let (_, list) = call(&s.app, auth, "GET", &uri, None).await;
    assert_eq!(ids(&list), ["PL-1"]);

    let uri = format!("/api/workspaces/{}/issues", s.other.workspace_id);
    let (status, body) = call(&s.app, auth, "GET", &uri, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    let uri = format!("/api/workspaces/{}/events", s.other.workspace_id);
    let (status, _) = call(&s.app, auth, "GET", &uri, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, body) = call(&s.app, auth, "GET", "/api/issues/OT-1", None).await;
    assert_eq!(
        (status, body),
        (
            StatusCode::FORBIDDEN,
            json!({"code": "forbidden", "message": "issue OT-1 is in another workspace"})
        )
    );

    let (status, body) = call(&s.app, auth, "GET", "/api/issues", None).await;
    assert_eq!(
        (status, body),
        (
            StatusCode::BAD_REQUEST,
            json!({
                "code": "workspace_not_named",
                "message": "this account is in several workspaces (GW, PL); name one with /api/workspaces/{id}/"
            })
        )
    );
    let other_bearer = format!("Bearer {}", s.other.token);
    let (status, list) = call(
        &s.app,
        ("authorization", &other_bearer),
        "GET",
        "/api/issues",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(ids(&list), ["OT-1"]);
}

#[sqlx::test]
async fn an_edit_in_a_workspace_is_recorded_as_the_accounts_person_there(pool: PgPool) {
    let s = setup(&pool).await;
    let cookie = format!("punchlist_session={}", s.account.token);
    let auth = ("cookie", cookie.as_str());

    let (status, body) = call(
        &s.app,
        auth,
        "PATCH",
        "/api/issues/GW-1",
        Some(json!({"title": "Renamed"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = call(
        &s.app,
        auth,
        "POST",
        "/api/issues/GW-1/transitions",
        Some(json!({"to": "todo"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let edited = sqlx::query_scalar!(
        "SELECT actor_id FROM event WHERE issue_id = 'GW-1' AND kind = 'edited'"
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let moved = sqlx::query_scalar!("SELECT actor_id FROM transition WHERE issue_id = 'GW-1'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!((edited, moved), (s.gw_person, s.gw_person));

    // Who is signed in needs no workspace: the person in the oldest one.
    let (status, session) = call(&s.app, auth, "GET", "/api/session", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        session,
        json!({
            "actor": {"id": s.account.actor_id, "name": "Gannon", "role": "person"},
            "workspace_id": s.account.workspace_id,
            "account": {"id": s.account.account_id, "name": "Gannon"},
        })
    );
}

#[sqlx::test]
async fn an_agent_token_stays_in_its_workspace(pool: PgPool) {
    let s = setup(&pool).await;
    let bearer = format!("Bearer {}", s.account.token);
    let uri = format!("/api/workspaces/{}/actors", s.account.workspace_id);
    let (status, body) = call(
        &s.app,
        ("authorization", &bearer),
        "POST",
        &uri,
        Some(json!({"kind": "codex"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let agent = format!("Bearer {}", body["token"].as_str().unwrap());
    let auth = ("authorization", agent.as_str());

    let (status, _) = call(&s.app, auth, "GET", "/api/issues/GW-1", None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let uri = format!("/api/workspaces/{}/issues", s.gw);
    let (status, _) = call(&s.app, auth, "GET", &uri, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let uri = format!("/api/workspaces/{}/issues", s.account.workspace_id);
    let (status, list) = call(&s.app, auth, "GET", &uri, None).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(ids(&list), ["PL-1"]);
    // `pl` resolves the workspace through this list: the agent's own only.
    let (status, workspaces) = call(&s.app, auth, "GET", "/api/workspaces", None).await;
    assert_eq!(status, StatusCode::OK, "{workspaces}");
    let prefixes: Vec<&str> = (workspaces.as_array().unwrap().iter())
        .map(|workspace| workspace["issue_prefix"].as_str().unwrap())
        .collect();
    assert_eq!(prefixes, ["PL"]);
}
