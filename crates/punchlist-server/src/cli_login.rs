//! `/api/cli-logins` (PL-170): `pl auth login` asks for a login, a person approves or denies
//! it on the approval page (`/cli-login?code=<code>`) in a browser signed in to Punchlist,
//! and `pl` polls with its secret until the answer is a CLI session, which it sends as its
//! bearer value (`auth::session_account`). Punchlist's own flow, so it works with any sign-in.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::header::HOST;
use axum::http::{HeaderMap, StatusCode, Uri};
use punchlist_api::{
    CliLogin, CliLoginInfo, CliLoginPoll, CliLoginState, CliLoginToken, ErrorBody,
};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::auth::{Actor, hash_token};
use crate::session::{account_of, cli_device, https, random_id, user_agent};
use crate::{ApiError, AppState};

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(create_login))
        .routes(routes!(poll_login))
        .routes(routes!(show_login))
        .routes(routes!(approve_login))
        .routes(routes!(deny_login))
}

/// Consonants alone, so a code spells no word and has no 0/O or 1/I to confuse.
const CODE_LETTERS: &[u8] = b"BCDFGHJKLMNPQRSTVWXZ";

/// 8 random letters of `CODE_LETTERS`.
fn new_code() -> String {
    let mut code = String::with_capacity(8);
    while code.len() < 8 {
        let mut bytes = [0u8; 16];
        aws_lc_rs::rand::fill(&mut bytes).expect("the system's random number generator");
        // 240 is the largest multiple of 20 under 256: rejecting the rest keeps each letter
        // equally likely.
        (bytes.iter())
            .filter(|byte| **byte < 240)
            .take(8 - code.len())
            .for_each(|byte| code.push(CODE_LETTERS[usize::from(*byte) % 20] as char));
    }
    code
}

/// The origin a browser reaches the instance at: `PUNCHLIST_PUBLIC_URL`, else the request's.
fn origin(state: &AppState, uri: &Uri, headers: &HeaderMap) -> String {
    if let Some(url) = state.public_url.as_deref() {
        return url.to_string();
    }
    let host = ["x-forwarded-host", HOST.as_str()]
        .iter()
        .find_map(|name| headers.get(*name)?.to_str().ok())
        .unwrap_or("localhost");
    let scheme = if https(state, uri, headers) {
        "https"
    } else {
        "http"
    };
    format!("{scheme}://{host}")
}

/// Starts a `pl auth login`. No token: this is how `pl` gets one.
#[utoipa::path(
    post,
    path = "/api/cli-logins",
    security(()),
    responses((status = 201, body = CliLogin))
)]
async fn create_login(
    State(state): State<AppState>,
    uri: Uri,
    headers: HeaderMap,
) -> Result<(StatusCode, Json<CliLogin>), ApiError> {
    // ponytail: anyone can start a login and nothing limits how many; expired ones are swept
    // here a day after they expire. A per-address limit would bound the table.
    sqlx::query!("DELETE FROM cli_login WHERE expires_at < now() - interval '1 day'")
        .execute(&state.pool)
        .await?;
    let lifetime = state.cli_login_lifetime;
    let device_secret = random_id();
    let code = loop {
        let code = new_code();
        let inserted = sqlx::query!(
            "INSERT INTO cli_login (code, secret_hash, user_agent, expires_at)
             VALUES ($1, $2, $3, now() + make_interval(secs => $4))
             ON CONFLICT (code) DO NOTHING",
            code,
            hash_token(&device_secret),
            user_agent(&headers),
            lifetime.as_secs_f64(),
        )
        .execute(&state.pool)
        .await?;
        if inserted.rows_affected() == 1 {
            break code;
        }
    };
    let url = format!("{}/cli-login?code={code}", origin(&state, &uri, &headers));
    Ok((
        StatusCode::CREATED,
        Json(CliLogin {
            code,
            url,
            device_secret,
            expires_in: lifetime.as_secs(),
        }),
    ))
}

/// `pl`'s poll: an approved login answers its new CLI session once, which expires 90 days
/// after its last use.
#[utoipa::path(
    post,
    path = "/api/cli-logins/token",
    security(()),
    request_body = CliLoginPoll,
    responses(
        (status = 200, body = CliLoginToken),
        (status = 404, body = ErrorBody, description = "No login has this secret."),
        (status = 409, body = ErrorBody, description = "The login's session was already answered."),
    )
)]
async fn poll_login(
    State(state): State<AppState>,
    Json(poll): Json<CliLoginPoll>,
) -> Result<Json<CliLoginToken>, ApiError> {
    let mut tx = state.pool.begin().await?;
    let login = sqlx::query!(
        r#"SELECT code, state, account_id, user_agent, expires_at > now() AS "live!"
           FROM cli_login WHERE secret_hash = $1 FOR UPDATE"#,
        hash_token(&poll.device_secret),
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::CliLoginNotFound("no login has this secret".into()))?;
    let answer = match (login.state.as_str(), login.live, login.account_id) {
        ("used", ..) => {
            return Err(ApiError::Conflict(
                "this login's session was already answered".into(),
            ));
        }
        (_, false, _) => CliLoginToken::Expired,
        ("denied", ..) => CliLoginToken::Denied,
        ("approved", true, Some(account_id)) => {
            let token = random_id();
            sqlx::query!(
                "INSERT INTO session (id, account_id, expires_at, user_agent, kind)
                 VALUES ($1, $2, now() + interval '90 days', $3, 'cli')",
                hash_token(&token),
                account_id,
                login.user_agent,
            )
            .execute(&mut *tx)
            .await?;
            sqlx::query!(
                "UPDATE cli_login SET state = 'used' WHERE code = $1",
                login.code
            )
            .execute(&mut *tx)
            .await?;
            CliLoginToken::Approved { token }
        }
        _ => CliLoginToken::Pending,
    };
    tx.commit().await?;
    Ok(Json(answer))
}

/// The login the approval page asks a person about.
#[utoipa::path(
    get,
    path = "/api/cli-logins/{code}",
    params(("code" = String, Path)),
    responses(
        (status = 200, body = CliLoginInfo),
        (status = 403, body = ErrorBody, description = "Not a person."),
        (status = 404, body = ErrorBody),
    )
)]
async fn show_login(
    State(state): State<AppState>,
    actor: Actor,
    Path(code): Path<String>,
) -> Result<Json<CliLoginInfo>, ApiError> {
    account_of(&actor)?;
    let code = code.to_ascii_uppercase();
    let login = sqlx::query!(
        r#"SELECT state, user_agent, expires_at > now() AS "live!"
           FROM cli_login WHERE code = $1"#,
        code,
    )
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| not_found(&code))?;
    let state = match (login.state.as_str(), login.live) {
        ("approved" | "used", _) => CliLoginState::Approved,
        ("denied", _) => CliLoginState::Denied,
        (_, false) => CliLoginState::Expired,
        _ => CliLoginState::Pending,
    };
    Ok(Json(CliLoginInfo {
        code,
        device: cli_device(login.user_agent.as_deref()),
        state,
    }))
}

/// Approve: `pl`'s next poll gets a session of the person's account.
#[utoipa::path(
    post,
    path = "/api/cli-logins/{code}/approve",
    params(("code" = String, Path)),
    responses(
        (status = 204, description = "Approved."),
        (status = 403, body = ErrorBody, description = "Not a person."),
        (status = 404, body = ErrorBody),
        (status = 409, body = ErrorBody, description = "Already approved or denied, or expired."),
    )
)]
async fn approve_login(
    State(state): State<AppState>,
    actor: Actor,
    Path(code): Path<String>,
) -> Result<StatusCode, ApiError> {
    let account = account_of(&actor)?;
    decide(&state, &code, "approved", Some(account)).await
}

/// Deny: `pl`'s next poll says so, and no session is made.
#[utoipa::path(
    post,
    path = "/api/cli-logins/{code}/deny",
    params(("code" = String, Path)),
    responses(
        (status = 204, description = "Denied."),
        (status = 403, body = ErrorBody, description = "Not a person."),
        (status = 404, body = ErrorBody),
        (status = 409, body = ErrorBody, description = "Already approved or denied, or expired."),
    )
)]
async fn deny_login(
    State(state): State<AppState>,
    actor: Actor,
    Path(code): Path<String>,
) -> Result<StatusCode, ApiError> {
    account_of(&actor)?;
    decide(&state, &code, "denied", None).await
}

/// Approves or denies a pending, live login, once.
async fn decide(
    state: &AppState,
    code: &str,
    decision: &str,
    account: Option<uuid::Uuid>,
) -> Result<StatusCode, ApiError> {
    let code = code.to_ascii_uppercase();
    let decided = sqlx::query!(
        "UPDATE cli_login SET state = $2, account_id = $3
         WHERE code = $1 AND state = 'pending' AND expires_at > now()",
        code,
        decision,
        account,
    )
    .execute(&state.pool)
    .await?;
    if decided.rows_affected() == 1 {
        return Ok(StatusCode::NO_CONTENT);
    }
    let login = sqlx::query!(
        r#"SELECT state, expires_at > now() AS "live!" FROM cli_login WHERE code = $1"#,
        code,
    )
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| not_found(&code))?;
    Err(ApiError::Conflict(match login.state.as_str() {
        "approved" | "used" => format!("login {code} was already approved"),
        "denied" => format!("login {code} was already denied"),
        _ if !login.live => format!("login {code} expired"),
        other => format!("login {code} is {other}"),
    }))
}

fn not_found(code: &str) -> ApiError {
    ApiError::CliLoginNotFound(format!("login {code} not found"))
}

#[cfg(test)]
mod tests {
    use super::{CODE_LETTERS, new_code};

    #[test]
    fn a_code_is_8_of_the_letters() {
        for _ in 0..200 {
            let code = new_code();
            assert_eq!(code.len(), 8, "{code}");
            assert!(code.bytes().all(|b| CODE_LETTERS.contains(&b)), "{code}");
        }
    }
}
