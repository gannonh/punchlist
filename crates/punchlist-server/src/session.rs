//! `/api/session`: a person signs in from the browser with their token, which the server
//! keeps in the session cookie (`auth::SESSION_COOKIE`), and signs out.

use axum::Json;
use axum::extract::State;
use axum::http::header::SET_COOKIE;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use punchlist_api::{ErrorBody, Session, SignIn};
use punchlist_core::Role;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::auth::{Actor, SESSION_COOKIE, hash_token, parse_role};
use crate::{ApiError, AppState};

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(sign_in, show_session, sign_out))
}

/// Sign in with a person's token: the answer sets the session cookie to it.
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
    let row = sqlx::query!(
        "SELECT id, workspace_id, name, role, kind FROM actor WHERE token_hash = $1",
        hash_token(&request.token),
    )
    .fetch_optional(&state.pool)
    .await?
    .ok_or(ApiError::Unauthorized)?;
    let role = parse_role(&row.role)?;
    if role != Role::Person {
        return Err(ApiError::Unauthorized);
    }
    // `Secure` only over https: the development app is plain http on Tailscale, where a
    // browser would not send a `Secure` cookie back.
    let https = uri.scheme_str() == Some("https")
        || (headers.get("x-forwarded-proto")).is_some_and(|proto| proto == "https");
    let cookie = format!(
        "{SESSION_COOKIE}={}; HttpOnly; SameSite=Strict; Path=/{}",
        request.token,
        if https { "; Secure" } else { "" }
    );
    let session = Session {
        actor: punchlist_api::Actor {
            id: row.id,
            name: row.name,
            role,
            kind: row.kind,
        },
        workspace_id: row.workspace_id,
    };
    Ok(([(SET_COOKIE, cookie)], Json(session)).into_response())
}

/// Who the request is signed in as, by its bearer token or its session cookie.
#[utoipa::path(
    get,
    path = "/api/session",
    responses((status = 200, body = Session), (status = 401, body = ErrorBody))
)]
async fn show_session(actor: Actor) -> Json<Session> {
    Json(Session {
        actor: (&actor).into(),
        workspace_id: actor.workspace_id,
    })
}

/// Sign out: the answer clears the session cookie.
#[utoipa::path(
    delete,
    path = "/api/session",
    security(()),
    responses((status = 204, description = "The session cookie is cleared."))
)]
async fn sign_out() -> Response {
    let cookie = format!("{SESSION_COOKIE}=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0");
    (StatusCode::NO_CONTENT, [(SET_COOKIE, cookie)]).into_response()
}
