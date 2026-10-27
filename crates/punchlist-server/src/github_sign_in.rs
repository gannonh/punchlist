//! Sign-in with GitHub (ADR 0013): the workspace's GitHub App authorizes the browser's GitHub
//! user, and the server links that user to the signed-in account, or signs in the account
//! linked to it. The user's token is used for one `GET /user` and dropped.

use axum::Json;
use axum::extract::{Query, State};
use axum::http::header::{LOCATION, SET_COOKIE};
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use punchlist_api::{ErrorBody, SignInOptions};
use serde::Deserialize;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::auth::{hash_token, request_cookie, session_account, session_id};
use crate::session::{cookie, https, random_id, start_session};
use crate::{ApiError, AppState};

/// The GitHub App's OAuth client, and where GitHub is.
#[derive(Debug, Clone)]
pub struct GithubSignIn {
    pub client_id: String,
    pub client_secret: String,
    /// `https://github.com`, or a fake GitHub in tests (`GITHUB_OAUTH_URL`).
    pub oauth_url: String,
    /// `https://api.github.com`, or a fake GitHub in tests (`GITHUB_API_URL`).
    pub api_url: String,
}

/// Holds the `state` of a sign-in in flight, and for a link the hash of the session that
/// started it, as `<state>.<session hash>`.
const STATE_COOKIE: &str = "punchlist_github";
const STATE_PATH: &str = "/api/auth/github";

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(options))
        .routes(routes!(start))
        .routes(routes!(callback))
}

/// The ways the sign-in page offers.
#[utoipa::path(
    get,
    path = "/api/auth",
    security(()),
    responses((status = 200, body = SignInOptions))
)]
async fn options(State(state): State<AppState>) -> Json<SignInOptions> {
    Json(SignInOptions {
        github: state.github_sign_in.is_some(),
    })
}

#[derive(Deserialize, utoipa::IntoParams)]
struct StartQuery {
    /// `1` links GitHub to the signed-in account instead of signing in.
    link: Option<String>,
}

/// Sends the browser to GitHub to authorize the App, with a fresh `state` kept in a cookie for
/// ten minutes.
#[utoipa::path(
    get,
    path = "/api/auth/github/start",
    params(StartQuery),
    security(()),
    responses(
        (status = 303, description = "To GitHub's authorization page."),
        (status = 401, body = ErrorBody, description = "A link without a session."),
        (status = 404, body = ErrorBody, description = "Sign-in with GitHub is not configured."),
    )
)]
async fn start(
    State(state): State<AppState>,
    Query(query): Query<StartQuery>,
    uri: Uri,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let github = state.github_sign_in.as_deref().ok_or_else(not_configured)?;
    let session = match query.link.as_deref() {
        Some("1") => {
            let hash = hash_token(session_id(&headers).ok_or(ApiError::Unauthorized)?);
            session_account(&state.pool, &hash)
                .await?
                .ok_or(ApiError::Unauthorized)?;
            hash
        }
        _ => String::new(),
    };
    let flow = random_id();
    let authorize = reqwest::Url::parse_with_params(
        &format!("{}/login/oauth/authorize", github.oauth_url),
        [("client_id", github.client_id.as_str()), ("state", &flow)],
    )
    .map_err(|error| ApiError::Invalid(format!("GITHUB_OAUTH_URL: {error}")))?;
    let https = https(&state, &uri, &headers);
    let value = format!("{flow}.{session}");
    let state_cookie = cookie(STATE_COOKIE, &value, STATE_PATH, "; Max-Age=600", https);
    Ok(redirect(authorize.as_str(), [state_cookie]))
}

#[derive(Deserialize, utoipa::IntoParams)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
}

#[derive(Deserialize)]
struct AccessToken {
    access_token: Option<String>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct GithubUser {
    id: i64,
    login: String,
}

/// GitHub sends the browser back here. The `state` must be the one the browser's cookie holds.
/// A link stores the GitHub user on the account and goes to `/`; a sign-in starts a session of
/// the account linked to the user and goes to `/`, or, with none linked, goes to
/// `/sign-in?unlinked=<login>`.
#[utoipa::path(
    get,
    path = "/api/auth/github/callback",
    params(CallbackQuery),
    security(()),
    responses(
        (status = 303, description = "To the app, or to the sign-in page when no account is linked."),
        (status = 400, body = ErrorBody, description = "A wrong or missing `state` or code."),
        (status = 409, body = ErrorBody, description = "The GitHub user is linked to another account."),
        (status = 502, body = ErrorBody, description = "GitHub refused the code or could not be read."),
    )
)]
async fn callback(
    State(state): State<AppState>,
    Query(query): Query<CallbackQuery>,
    uri: Uri,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let github = state.github_sign_in.as_deref().ok_or_else(not_configured)?;
    let https = https(&state, &uri, &headers);
    let clear = cookie(STATE_COOKIE, "", STATE_PATH, "; Max-Age=0", https);
    let expired = || {
        ApiError::BadRequest(
            "this sign-in with GitHub expired or did not start in this browser; start again".into(),
        )
    };
    let (flow, link) = request_cookie(&headers, STATE_COOKIE)
        .and_then(|value| value.split_once('.'))
        .ok_or_else(expired)?;
    if query.state.as_deref() != Some(flow) {
        return Err(expired());
    }
    // A link is for the session that started it, still live.
    let linking = match link {
        "" => None,
        link => {
            let session = session_id(&headers).map(hash_token);
            if session.as_deref() != Some(link) {
                return Err(expired());
            }
            Some(
                session_account(&state.pool, link)
                    .await?
                    .ok_or_else(expired)?,
            )
        }
    };
    let code = query
        .code
        .ok_or_else(|| ApiError::BadRequest("GitHub sent no code".into()))?;
    let user = github_user(github, &code).await?;
    if let Some(account) = linking {
        let linked = sqlx::query!(
            "UPDATE account SET github_user_id = $2, github_login = $3 WHERE id = $1",
            account,
            user.id,
            user.login,
        )
        .execute(&state.pool)
        .await;
        match linked {
            Err(sqlx::Error::Database(error)) if error.is_unique_violation() => {
                return Err(ApiError::Conflict(format!(
                    "GitHub user {} is linked to another Punchlist account",
                    user.login
                )));
            }
            linked => linked?,
        };
        return Ok(redirect("/", [clear]));
    }
    // A GitHub user can rename itself: the stored login follows the id.
    let account = sqlx::query_scalar!(
        "UPDATE account SET github_login = $2 WHERE github_user_id = $1 RETURNING id",
        user.id,
        user.login,
    )
    .fetch_optional(&state.pool)
    .await?;
    let Some(account) = account else {
        // The login is letters, digits and hyphens (`github_user`), so it needs no escaping.
        return Ok(redirect(
            &format!("/sign-in?unlinked={}", user.login),
            [clear],
        ));
    };
    let session = start_session(&state, account, &uri, &headers).await?;
    Ok(redirect("/", [clear, session]))
}

/// Exchanges the code for the user's token and reads who the user is with it. The token goes
/// no further.
async fn github_user(github: &GithubSignIn, code: &str) -> Result<GithubUser, ApiError> {
    let unreadable = |error: reqwest::Error| {
        tracing::warn!(%error, "sign-in with GitHub: GitHub could not be read");
        ApiError::GithubSignIn("GitHub could not be read; try again".into())
    };
    let client = reqwest::Client::new();
    let token: AccessToken = client
        .post(format!("{}/login/oauth/access_token", github.oauth_url))
        .header("accept", "application/json")
        .json(&serde_json::json!({
            "client_id": github.client_id,
            "client_secret": github.client_secret,
            "code": code,
        }))
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(unreadable)?
        .json()
        .await
        .map_err(unreadable)?;
    let Some(token) = token.access_token else {
        let error = token.error.unwrap_or_default();
        tracing::warn!(error, "sign-in with GitHub: GitHub refused the code");
        return Err(ApiError::GithubSignIn(format!(
            "GitHub refused the sign-in ({error}); start again"
        )));
    };
    let user: GithubUser = client
        .get(format!("{}/user", github.api_url))
        .bearer_auth(token)
        .header("accept", "application/vnd.github+json")
        .header("user-agent", "punchlist")
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(unreadable)?
        .json()
        .await
        .map_err(unreadable)?;
    // GitHub's logins are letters, digits and single hyphens, at most 39 characters.
    let plain = (1..=39).contains(&user.login.len())
        && (user.login.bytes()).all(|byte| byte.is_ascii_alphanumeric() || byte == b'-');
    if !plain {
        return Err(ApiError::GithubSignIn(
            "GitHub answered a login that is not a GitHub login".into(),
        ));
    }
    Ok(user)
}

fn not_configured() -> ApiError {
    ApiError::NotConfigured("sign-in with GitHub is not configured".into())
}

/// A 303 to `location` that sets each cookie.
fn redirect<const N: usize>(location: &str, cookies: [String; N]) -> Response {
    let mut response = StatusCode::SEE_OTHER.into_response();
    let headers = response.headers_mut();
    if let Ok(location) = HeaderValue::from_str(location) {
        headers.insert(LOCATION, location);
    }
    for cookie in cookies {
        if let Ok(cookie) = HeaderValue::from_str(&cookie) {
            headers.append(SET_COOKIE, cookie);
        }
    }
    response
}
