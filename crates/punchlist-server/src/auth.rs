use axum::extract::FromRequestParts;
use axum::http::header::{AUTHORIZATION, COOKIE};
use axum::http::request::Parts;
use axum::http::{HeaderMap, Method};
use percent_encoding::percent_decode_str;
use punchlist_core::Role;
use sha2::{Digest, Sha256};
use utoipa::Modify;
use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use uuid::Uuid;

use crate::{ApiError, AppState};

/// The actor a request's bearer token belongs to.
#[derive(Debug, Clone)]
pub struct Actor {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub name: String,
    pub role: Role,
    /// The agent key, such as `codex`. Only agents have one.
    pub kind: Option<String>,
    /// The hash of the token the request came with; none for an actor acting without one.
    pub token_hash: Option<String>,
}

impl From<&Actor> for punchlist_api::Actor {
    fn from(actor: &Actor) -> punchlist_api::Actor {
        punchlist_api::Actor {
            id: actor.id,
            name: actor.name.clone(),
            role: actor.role,
            kind: actor.kind.clone(),
        }
    }
}

/// The cookie a browser signs in with (`POST /api/session`); its value is a person's token.
/// It is `SameSite=Strict`, so a browser sends it only on requests that Punchlist's own
/// pages make: that is the defence against cross-site request forgery, since no other
/// site's page can make a request that carries it.
pub const SESSION_COOKIE: &str = "punchlist_session";

/// The token in the request's session cookie.
fn session_token(headers: &HeaderMap) -> Option<&str> {
    (headers.get_all(COOKIE).iter())
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .find_map(|pair| pair.trim().strip_prefix(SESSION_COOKIE)?.strip_prefix('='))
        .filter(|token| !token.is_empty())
}

/// Reads an `actor.role` column. Its `CHECK` constraint allows only known roles, so an
/// unknown one is corrupt data, not a bad request.
pub fn parse_role(value: &str) -> Result<Role, sqlx::Error> {
    Role::parse(value)
        .ok_or_else(|| sqlx::Error::Decode(format!("unknown actor role `{value}`").into()))
}

/// A fresh bearer token. Only its hash is stored.
pub fn new_token() -> String {
    format!("plt_{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

/// SHA-256 of a bearer token, in hex. Only the hash is stored.
pub fn hash_token(token: &str) -> String {
    Sha256::digest(token.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

impl FromRequestParts<AppState> for Actor {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Actor, ApiError> {
        let (token, from_cookie) = match parts.headers.get(AUTHORIZATION) {
            Some(value) => (
                (value.to_str().ok())
                    .and_then(|value| value.strip_prefix("Bearer "))
                    .ok_or(ApiError::Unauthorized)?,
                false,
            ),
            None => (
                session_token(&parts.headers).ok_or(ApiError::Unauthorized)?,
                true,
            ),
        };
        // The issue a path names: `/api/issues/<id>` and below. Decoded as the route's `Path`
        // decodes it, so `PL%2D1` is checked as the `PL-1` the handler acts on.
        let issue_id = parts
            .uri
            .path()
            .strip_prefix("/api/issues/")
            .and_then(|rest| rest.split('/').next())
            .and_then(|id| percent_decode_str(id).decode_utf8().ok())
            .filter(|id| !id.is_empty());
        let issue_id = issue_id.as_deref();
        let token_hash = hash_token(token);
        let row = sqlx::query!(
            r#"SELECT a.id, a.workspace_id, a.name, a.role, a.kind, a.run_id,
                      r.issue_id AS "issue_id?", r.outcome AS "outcome?",
                      (SELECT workspace_id FROM issue WHERE id = $2) AS "issue_workspace_id?"
               FROM actor a LEFT JOIN run r ON r.id = a.run_id
               WHERE a.token_hash = $1"#,
            token_hash,
            issue_id,
        )
        .fetch_optional(&state.pool)
        .await?
        .ok_or(ApiError::Unauthorized)?;
        let role = parse_role(&row.role)?;
        // Only a person signs in from a browser.
        if from_cookie && role != Role::Person {
            return Err(ApiError::Unauthorized);
        }
        // A run's agent says its own run waits on a person, which that route refuses once the
        // run has ended.
        let own_waiting = parts.method == Method::POST
            && (row.run_id)
                .is_some_and(|run| parts.uri.path() == format!("/api/runs/{run}/waiting"));
        // A run's agent's token works only while its run is running, but for that route.
        if role == Role::Agent
            && row.run_id.is_some()
            && row.outcome.as_deref() != Some("running")
            && !own_waiting
        {
            return Err(ApiError::Unauthorized);
        }
        // No token reaches another workspace's issue. An issue that does not exist anywhere
        // is the handler's 404.
        if let Some(issue_workspace_id) = row.issue_workspace_id
            && issue_workspace_id != row.workspace_id
        {
            tracing::warn!(
                actor = %row.id,
                actor_workspace = %row.workspace_id,
                issue_workspace = %issue_workspace_id,
                issue = issue_id.unwrap_or_default(),
                "workspace mismatch: refused a request for another workspace's issue"
            );
            return Err(ApiError::Forbidden(format!(
                "issue {} is in another workspace",
                issue_id.unwrap_or_default()
            )));
        }
        // An agent reaches only issues: a run's agent its run's issue, `/api/issues/<id>` and
        // below, and its run's waiting route, and a standalone agent any issue of its
        // workspace, and who it is
        // (`GET /api/session`), so the web app can show an issue as that agent. Every other
        // endpoint, such as creating issues or reading the rest of the workspace, is for
        // people and runners.
        let own_session = parts.method == Method::GET && parts.uri.path() == "/api/session";
        if role == Role::Agent && !(row.run_id.is_none() && own_session) && !own_waiting {
            match (row.run_id, row.issue_id.as_deref()) {
                (Some(_), own) if issue_id.is_none() || issue_id != own => {
                    return Err(ApiError::Forbidden(format!(
                        "an agent acts only on its run's issue, {}",
                        own.unwrap_or("none")
                    )));
                }
                (None, _) if issue_id.is_none() => {
                    return Err(ApiError::Forbidden("an agent acts only on issues".into()));
                }
                _ => {}
            }
        }
        Ok(Actor {
            id: row.id,
            workspace_id: row.workspace_id,
            role,
            name: row.name,
            kind: row.kind,
            token_hash: Some(token_hash),
        })
    }
}

pub struct BearerAuth;

impl Modify for BearerAuth {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "bearer",
            SecurityScheme::Http(HttpBuilder::new().scheme(HttpAuthScheme::Bearer).build()),
        );
    }
}
