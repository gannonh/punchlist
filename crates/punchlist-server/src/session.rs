//! `/api/session`: a person signs in from the browser with their token, which starts a
//! server-side session held by the session cookie (`auth::SESSION_COOKIE`), and signs out.
//! Sign-in with GitHub (`github_sign_in`) starts one the same way (ADR 0013). `/api/sessions`
//! lists the account's sessions and ends them, from any of its browsers or with its token.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::header::{AUTHORIZATION, SET_COOKIE, USER_AGENT};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use punchlist_api::{Account, ErrorBody, Session, SessionInfo, SessionsRevoked, SignIn};
use punchlist_core::Role;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use crate::auth::{Actor, SESSION_COOKIE, hash_token, session_id};
use crate::{ApiError, AppState};

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(sign_in, show_session, sign_out))
        .routes(routes!(list_sessions, revoke_other_sessions))
        .routes(routes!(revoke_session))
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

/// The account of a person's request, by its token or its session; agents and runners have
/// no sessions.
fn account_of(actor: &Actor) -> Result<Uuid, ApiError> {
    (actor.account.as_ref())
        .map(|account| account.id)
        .ok_or_else(|| ApiError::Forbidden("only a person has sessions".into()))
}

/// The hash of the session the request came with: none for a request with a token.
fn current_session(headers: &HeaderMap) -> Option<String> {
    if headers.contains_key(AUTHORIZATION) {
        return None;
    }
    session_id(headers).map(hash_token)
}

/// The account's live sessions, newest use first.
#[utoipa::path(
    get,
    path = "/api/sessions",
    responses(
        (status = 200, body = Vec<SessionInfo>),
        (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody, description = "An agent's or a runner's token."),
    )
)]
async fn list_sessions(
    State(state): State<AppState>,
    actor: Actor,
    headers: HeaderMap,
) -> Result<Json<Vec<SessionInfo>>, ApiError> {
    let rows = sqlx::query!(
        r#"SELECT public_id, user_agent, created_at, last_used_at,
                  COALESCE(id = $2, false) AS "current!"
           FROM session
           WHERE account_id = $1 AND expires_at > now()
           ORDER BY last_used_at DESC, created_at DESC, public_id"#,
        account_of(&actor)?,
        current_session(&headers),
    )
    .fetch_all(&state.pool)
    .await?;
    let sessions = (rows.into_iter())
        .map(|row| SessionInfo {
            id: row.public_id,
            device: device(row.user_agent.as_deref().unwrap_or_default()),
            created_at: row.created_at,
            last_used_at: row.last_used_at,
            current: row.current,
        })
        .collect();
    Ok(Json(sessions))
}

/// Ends one of the account's sessions: that browser's next request answers 401.
#[utoipa::path(
    delete,
    path = "/api/sessions/{id}",
    params(("id" = Uuid, Path, description = "The session's `id` in `GET /api/sessions`.")),
    responses(
        (status = 204, description = "The session is ended."),
        (status = 403, body = ErrorBody),
        (status = 404, body = ErrorBody, description = "No such session of this account."),
    )
)]
async fn revoke_session(
    State(state): State<AppState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let deleted = sqlx::query!(
        "DELETE FROM session WHERE public_id = $1 AND account_id = $2",
        id,
        account_of(&actor)?,
    )
    .execute(&state.pool)
    .await?;
    if deleted.rows_affected() == 0 {
        return Err(ApiError::SessionNotFound(id));
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Ends every session of the account but the one the request came with; with a token, every
/// session.
#[utoipa::path(
    delete,
    path = "/api/sessions",
    responses((status = 200, body = SessionsRevoked), (status = 403, body = ErrorBody))
)]
async fn revoke_other_sessions(
    State(state): State<AppState>,
    actor: Actor,
    headers: HeaderMap,
) -> Result<Json<SessionsRevoked>, ApiError> {
    let deleted = sqlx::query!(
        "DELETE FROM session WHERE account_id = $1 AND id IS DISTINCT FROM $2",
        account_of(&actor)?,
        current_session(&headers),
    )
    .execute(&state.pool)
    .await?;
    Ok(Json(SessionsRevoked {
        revoked: deleted.rows_affected(),
    }))
}

/// The browser and OS a user agent names, such as `Safari on iPhone`; one of them when the
/// other is unknown, else `Unknown device`. Order matters: Edge's and Chrome on iOS's agents
/// name Safari and Chrome too, and Android's names Linux.
// ponytail: a substring table, not a user-agent database; an uncommon browser shows as the
// engine it claims, or as its OS alone.
fn device(user_agent: &str) -> String {
    const BROWSERS: [(&str, &str); 8] = [
        ("Edg/", "Edge"),
        ("EdgiOS/", "Edge"),
        ("OPR/", "Opera"),
        ("Firefox/", "Firefox"),
        ("FxiOS/", "Firefox"),
        ("CriOS/", "Chrome"),
        ("Chrome/", "Chrome"),
        ("Safari/", "Safari"),
    ];
    const SYSTEMS: [(&str, &str); 8] = [
        ("iPhone", "iPhone"),
        ("iPad", "iPad"),
        ("Android", "Android"),
        ("CrOS", "ChromeOS"),
        ("Windows", "Windows"),
        ("Mac OS X", "macOS"),
        ("Macintosh", "macOS"),
        ("Linux", "Linux"),
    ];
    let find = |table: &[(&str, &'static str)]| {
        (table.iter()).find_map(|(needle, name)| user_agent.contains(needle).then_some(*name))
    };
    match (find(&BROWSERS), find(&SYSTEMS)) {
        (Some(browser), Some(system)) => format!("{browser} on {system}"),
        (Some(name), None) | (None, Some(name)) => name.to_string(),
        (None, None) => "Unknown device".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::device;

    #[test]
    fn device_names_the_browser_and_os() {
        let cases = [
            (
                "Mozilla/5.0 (iPhone; CPU iPhone OS 17_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Mobile/15E148 Safari/604.1",
                "Safari on iPhone",
            ),
            (
                "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Mobile Safari/537.36",
                "Chrome on Android",
            ),
            (
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36",
                "Chrome on macOS",
            ),
            (
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36 Edg/126.0.0.0",
                "Edge on Windows",
            ),
            ("curl/8.0", "Unknown device"),
            ("<b>x</b>", "Unknown device"),
            ("", "Unknown device"),
        ];
        for (user_agent, expected) in cases {
            assert_eq!(device(user_agent), expected, "{user_agent}");
        }
    }
}
