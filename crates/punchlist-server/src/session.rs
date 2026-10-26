//! `/api/session`: a person signs in from the browser with their token, which starts a
//! server-side session held by the session cookie (`auth::SESSION_COOKIE`), and signs out.
//! Sign-in with GitHub (`github_sign_in`) starts one the same way (ADR 0013).

use axum::Json;
use axum::extract::State;
use axum::http::header::{SET_COOKIE, USER_AGENT};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use punchlist_api::{Account, ErrorBody, Session, SignIn};
use punchlist_core::Role;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use crate::auth::{Actor, SESSION_COOKIE, hash_token, session_id};
use crate::{ApiError, AppState};

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(sign_in, show_session, sign_out))
}

/// 32 random bytes, base64url: a session id or an OAuth `state`.
pub fn random_id() -> String {
    let mut bytes = [0u8; 32];
    aws_lc_rs::rand::fill(&mut bytes).expect("the system's random number generator");
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Whether the browser reaches the instance over https, so its cookies are `Secure`. The
/// development app is plain http on Tailscale, where a browser would not send a `Secure`
/// cookie back.
pub fn https(state: &AppState, uri: &Uri, headers: &HeaderMap) -> bool {
    (state.public_url.as_deref()).is_some_and(|url| url.starts_with("https://"))
        || uri.scheme_str() == Some("https")
        || (headers.get("x-forwarded-proto")).is_some_and(|proto| proto == "https")
}

/// A cookie's `Set-Cookie` value, with the attributes every Punchlist cookie has. `Lax`, not
/// `Strict`, so the session cookie set by GitHub's redirect back is kept; `auth::same_origin`
/// is the defence against cross-site writes.
pub fn cookie(name: &str, value: &str, path: &str, extra: &str, https: bool) -> String {
    let secure = if https { "; Secure" } else { "" };
    format!("{name}={value}; HttpOnly; SameSite=Lax; Path={path}{extra}{secure}")
}

/// Starts a session for the account, ending the one the request came with, and answers the
/// session cookie's `Set-Cookie` value.
pub async fn start_session(
    state: &AppState,
    account_id: Uuid,
    uri: &Uri,
    headers: &HeaderMap,
) -> Result<String, sqlx::Error> {
    end_session(&state.pool, headers).await?;
    // ponytail: expired sessions are swept only here, at a sign-in; a scheduled job would keep
    // the table smaller on an instance where people rarely sign in.
    sqlx::query!("DELETE FROM session WHERE expires_at <= now()")
        .execute(&state.pool)
        .await?;
    let id = random_id();
    let user_agent = (headers.get(USER_AGENT))
        .and_then(|value| value.to_str().ok())
        .map(|value| value.chars().take(500).collect::<String>());
    sqlx::query!(
        "INSERT INTO session (id, account_id, expires_at, user_agent)
         VALUES ($1, $2, now() + interval '30 days', $3)",
        hash_token(&id),
        account_id,
        user_agent,
    )
    .execute(&state.pool)
    .await?;
    Ok(cookie(
        SESSION_COOKIE,
        &id,
        "/",
        "",
        https(state, uri, headers),
    ))
}

/// Deletes the session the request's cookie holds, if any.
async fn end_session(pool: &sqlx::PgPool, headers: &HeaderMap) -> Result<(), sqlx::Error> {
    if let Some(id) = session_id(headers) {
        sqlx::query!("DELETE FROM session WHERE id = $1", hash_token(id))
            .execute(pool)
            .await?;
    }
    Ok(())
}

/// Sign in with a person's token: the answer sets the session cookie to a new session of the
/// token's account, which reaches every workspace of the account (ADR 0011, 0013).
#[utoipa::path(
    post,
    path = "/api/session",
    request_body = SignIn,
    security(()),
    responses(
        (status = 200, body = Session, headers(("set-cookie" = String, description = "The session cookie."))),
        (status = 401, body = ErrorBody, description = "Not a person's token."),
    )
)]
async fn sign_in(
    State(state): State<AppState>,
    uri: Uri,
    headers: HeaderMap,
    Json(request): Json<SignIn>,
) -> Result<Response, ApiError> {
    // Only a person's token signs in, and a person's token is their account's: the session
    // shows its person in the oldest of its workspaces.
    let row = sqlx::query!(
        "SELECT acc.id AS account_id, acc.name AS account_name, acc.github_login, a.id,
                a.workspace_id, a.name
         FROM account acc
         JOIN actor a ON a.account_id = acc.id
         JOIN workspace w ON w.id = a.workspace_id
         WHERE acc.token_hash = $1
         ORDER BY w.created_at, w.id
         LIMIT 1",
        hash_token(&request.token),
    )
    .fetch_optional(&state.pool)
    .await?
    .ok_or(ApiError::Unauthorized)?;
    let cookie = start_session(&state, row.account_id, &uri, &headers).await?;
    let session = Session {
        actor: punchlist_api::Actor {
            id: row.id,
            name: row.name,
            role: Role::Person,
            kind: None,
        },
        workspace_id: row.workspace_id,
        account: Some(Account {
            id: row.account_id,
            name: row.account_name,
            github_login: row.github_login,
        }),
    };
    Ok(([(SET_COOKIE, cookie)], Json(session)).into_response())
}

/// Who the request is signed in as, by its bearer token or its session cookie. A person's
/// account shows its person in the oldest of its workspaces.
#[utoipa::path(
    get,
    path = "/api/session",
    responses((status = 200, body = Session), (status = 401, body = ErrorBody))
)]
async fn show_session(actor: Actor) -> Json<Session> {
    Json(Session {
        actor: (&actor).into(),
        workspace_id: actor.workspace_id,
        account: actor.account,
    })
}

/// Sign out: the answer deletes the cookie's session and clears the cookie.
#[utoipa::path(
    delete,
    path = "/api/session",
    security(()),
    responses((status = 204, description = "The session is deleted and its cookie cleared."))
)]
async fn sign_out(
    State(state): State<AppState>,
    uri: Uri,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    end_session(&state.pool, &headers).await?;
    let https = https(&state, &uri, &headers);
    let cookie = cookie(SESSION_COOKIE, "", "/", "; Max-Age=0", https);
    Ok((StatusCode::NO_CONTENT, [(SET_COOKIE, cookie)]).into_response())
}
