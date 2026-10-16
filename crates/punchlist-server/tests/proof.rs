//! Proof uploads, lists and downloads: multipart requests against the router, then the rows
//! and files they left, read back from Postgres and the proof directory.

use std::convert::Infallible;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use http_body_util::channel::Channel;
use punchlist_server::{AppState, Bootstrap, Bootstrapped, ProofStore, bootstrap, router};
use serde_json::{Value, json};
use sqlx::PgPool;
use tempfile::TempDir;
use tower::ServiceExt;

const BOUNDARY: &str = "punchlist-test-boundary";

struct Setup {
    app: Router,
    person: Bootstrapped,
    dir: TempDir,
}

async fn setup(pool: &PgPool, prefix: &str) -> Setup {
    let person = bootstrap(
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
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::new(pool.clone()).with_proof(ProofStore {
        dir: dir.path().to_path_buf(),
        max_bytes: 16,
    });
    Setup {
        app: router(state),
        person,
        dir,
    }
}

async fn send(app: &Router, request: Request<Body>) -> (StatusCode, Value) {
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
    send(app, request).await
}

/// Uploads `bytes` as `content_type` to the issue, with a title and, when given, a kind.
async fn attach(
    s: &Setup,
    token: &str,
    id: &str,
    content_type: &str,
    bytes: &[u8],
    kind: Option<&str>,
) -> (StatusCode, Value) {
    let mut body = Vec::new();
    let mut field =
        |name: &str, value: &str| {
            body.extend(format!(
            "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
        ).bytes());
        };
    field("title", "List at 1280×800");
    if let Some(kind) = kind {
        field("kind", kind);
    }
    body.extend(
        format!(
            "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"f\"\r\n\
             Content-Type: {content_type}\r\n\r\n"
        )
        .bytes(),
    );
    body.extend(bytes);
    body.extend(format!("\r\n--{BOUNDARY}--\r\n").bytes());
    let request = Request::builder()
        .method("POST")
        .uri(format!("/api/issues/{id}/proof"))
        .header("authorization", format!("Bearer {token}"))
        .header(
            "content-type",
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .body(Body::from(body))
        .unwrap();
    send(&s.app, request).await
}

async fn issue(s: &Setup) -> String {
    let (status, issue) = call(
        &s.app,
        &s.person.token,
        "POST",
        "/api/issues",
        Some(json!({"title": "Fix it"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{issue}");
    issue["id"].as_str().unwrap().to_string()
}

/// The files in the proof directory.
fn stored(s: &Setup) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(s.dir.path())
        .map(|entries| {
            entries
                .map(|entry| entry.unwrap().file_name().into_string().unwrap())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

async fn proof_rows(pool: &PgPool) -> i64 {
    sqlx::query_scalar!(r#"SELECT count(*) AS "n!" FROM proof"#)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test]
async fn each_kind_is_stored_listed_and_served_with_its_content_type(pool: PgPool) {
    let s = setup(&pool, "PL").await;
    let id = issue(&s).await;
    let token = s.person.token.clone();
    let uploads = [
        (
            "image/png",
            &b"\x89PNG\r\n\x1a\nimage"[..],
            Some("image"),
            "image",
        ),
        ("video/mp4", &b"ftypmp42"[..], None, "video"),
        (
            "text/plain; charset=utf-8",
            &b"all 4 checks ok"[..],
            None,
            "report",
        ),
    ];
    let mut ids = Vec::new();
    for (content_type, bytes, kind, expected) in uploads {
        let (status, proof) = attach(&s, &token, &id, content_type, bytes, kind).await;
        assert_eq!(status, StatusCode::CREATED, "{proof}");
        assert_eq!(
            (
                &proof["kind"],
                &proof["size"],
                &proof["attempt"],
                &proof["title"]
            ),
            (
                &json!(expected),
                &json!(bytes.len()),
                &json!(0),
                &json!("List at 1280×800")
            )
        );
        assert_eq!(proof["actor"]["name"], "Gannon");
        ids.push(proof["id"].as_str().unwrap().to_string());
    }
    // The parameter is not stored: the content type is the one proof can be.
    let (status, list) = call(
        &s.app,
        &token,
        "GET",
        &format!("/api/issues/{id}/proof"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let listed: Vec<(&str, &str, &str)> = (list.as_array().unwrap().iter())
        .map(|p| {
            (
                p["id"].as_str().unwrap(),
                p["kind"].as_str().unwrap(),
                p["content_type"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        listed,
        [
            (ids[0].as_str(), "image", "image/png"),
            (ids[1].as_str(), "video", "video/mp4"),
            (ids[2].as_str(), "report", "text/plain"),
        ]
    );
    assert_eq!(stored(&s).len(), 3);

    let request = Request::builder()
        .uri(format!("/api/proof/{}", ids[0]))
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let response = s.app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let header = |name: &str| response.headers()[name].to_str().unwrap().to_string();
    assert_eq!(
        (
            header("content-type"),
            header("x-content-type-options"),
            header("content-security-policy")
        ),
        ("image/png".into(), "nosniff".into(), "sandbox".into())
    );
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&bytes[..], b"\x89PNG\r\n\x1a\nimage");
}

#[sqlx::test]
async fn a_content_type_proof_cannot_be_or_a_wrong_kind_is_refused_and_nothing_is_kept(
    pool: PgPool,
) {
    let s = setup(&pool, "PL").await;
    let id = issue(&s).await;
    let token = s.person.token.clone();
    let (status, body) = attach(&s, &token, &id, "text/html", b"<script>", None).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        body["message"],
        "proof cannot be `text/html`; it can be image/png, image/jpeg, image/gif, image/webp, \
         video/mp4, video/webm, video/quicktime, text/plain, text/markdown, application/xml, \
         application/pdf"
    );
    let (status, body) = attach(&s, &token, &id, "image/png", b"png", Some("video")).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["message"], "`image/png` is image proof, not `video`");
    assert_eq!(proof_rows(&pool).await, 0);
    assert_eq!(stored(&s), Vec::<String>::new());
}

#[sqlx::test]
async fn a_file_over_the_limit_is_refused_and_its_part_removed(pool: PgPool) {
    let s = setup(&pool, "PL").await;
    let id = issue(&s).await;
    let token = s.person.token.clone();
    let (status, body) = attach(&s, &token, &id, "text/plain", &[b'x'; 17], None).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{body}");
    assert_eq!(
        body,
        json!({"code": "too_large", "message": "the file is over the limit of 16 bytes"})
    );
    assert_eq!(proof_rows(&pool).await, 0);
    assert_eq!(stored(&s), Vec::<String>::new());
    // At the limit is fine.
    let (status, _) = attach(&s, &token, &id, "text/plain", &[b'x'; 16], None).await;
    assert_eq!(status, StatusCode::CREATED);
}

#[sqlx::test]
async fn an_agent_cannot_attach_proof_in_human_review_or_download_it(pool: PgPool) {
    let s = setup(&pool, "PL").await;
    let id = issue(&s).await;
    let (status, agent) = call(
        &s.app,
        &s.person.token,
        "POST",
        "/api/actors",
        Some(json!({"kind": "claude-code"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{agent}");
    let agent = agent["token"].as_str().unwrap().to_string();

    // Before Human Review the agent attaches proof as itself.
    let (status, proof) = attach(&s, &agent, &id, "image/png", b"png", None).await;
    assert_eq!(status, StatusCode::CREATED, "{proof}");
    assert_eq!(proof["actor"]["role"], "agent");
    assert_eq!(proof["actor"]["kind"], "claude-code");
    // Agents reach only issues, so not the file.
    let (status, _) = call(
        &s.app,
        &agent,
        "GET",
        &format!("/api/proof/{}", proof["id"].as_str().unwrap()),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Test-only: the path to Human Review needs pull request evidence from GitHub.
    sqlx::query!("UPDATE issue SET status = 'human_review' WHERE id = $1", id)
        .execute(&pool)
        .await
        .unwrap();
    let (status, body) = attach(&s, &agent, &id, "image/png", b"png", None).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(
        (&body["code"], &body["message"]),
        (
            &json!("locked"),
            &json!("human_review is locked: an agent cannot act on an issue in it")
        )
    );
    assert_eq!(proof_rows(&pool).await, 1);
    assert_eq!(stored(&s).len(), 1);
    // The person still can.
    let (status, _) = attach(&s, &s.person.token, &id, "image/png", b"png", None).await;
    assert_eq!(status, StatusCode::CREATED);
}

#[sqlx::test]
async fn an_upload_that_outlives_its_attempt_is_refused_and_nothing_is_kept(pool: PgPool) {
    let s = setup(&pool, "PL").await;
    let id = issue(&s).await;
    // Room for one chunk: the second send returns only once the server has read the first,
    // so the upload has started in attempt 0.
    let (mut body, channel) = Channel::<Bytes, Infallible>::new(1);
    let request = Request::builder()
        .method("POST")
        .uri(format!("/api/issues/{id}/proof"))
        .header("authorization", format!("Bearer {}", s.person.token))
        .header(
            "content-type",
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .body(Body::new(channel))
        .unwrap();
    let app = s.app.clone();
    let response = tokio::spawn(async move { send(&app, request).await });
    body.send_data(Bytes::from(format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"title\"\r\n\r\nShot\r\n\
         --{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"f\"\r\n\
         Content-Type: image/png\r\n\r\npn"
    )))
    .await
    .unwrap();
    body.send_data(Bytes::from_static(b"g")).await.unwrap();

    // Test-only: the issue is started again while the file is still uploading.
    sqlx::query!(
        "INSERT INTO transition (issue_id, from_status, to_status, actor_id, workflow_version)
         VALUES ($1, 'start', 'in_progress', $2, 'test')",
        id,
        s.person.actor_id,
    )
    .execute(&pool)
    .await
    .unwrap();
    body.send_data(Bytes::from(format!("\r\n--{BOUNDARY}--\r\n")))
        .await
        .unwrap();
    drop(body);

    let (status, error) = response.await.unwrap();
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(
        (&error["code"], &error["message"]),
        (
            &json!("stale_attempt"),
            &json!(format!(
                "{id} moved from attempt 0 to attempt 1 while the proof uploaded, so it was not attached"
            ))
        )
    );
    assert_eq!(proof_rows(&pool).await, 0);
    assert_eq!(stored(&s), Vec::<String>::new());
}

#[sqlx::test]
async fn another_workspaces_proof_is_not_found(pool: PgPool) {
    let s = setup(&pool, "PL").await;
    let id = issue(&s).await;
    let (_, proof) = attach(&s, &s.person.token, &id, "image/png", b"png", None).await;
    let other = bootstrap(
        &pool,
        &Bootstrap {
            workspace_name: "OT".into(),
            issue_prefix: "OT".into(),
            repository_owner: "gannonh".into(),
            repository_name: "ot".into(),
            person_name: "Other".into(),
        },
    )
    .await
    .unwrap();
    let uri = format!("/api/proof/{}", proof["id"].as_str().unwrap());
    let (status, body) = call(&s.app, &other.token, "GET", &uri, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");
    let (status, _) = call(
        &s.app,
        &s.person.token,
        "GET",
        "/api/issues/PL-9/proof",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
