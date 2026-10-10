use axum::extract::FromRequestParts;
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
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
    /// For an agent: the issue of the run it acts for, the only issue it may change.
    pub issue_id: Option<String>,
}

impl Actor {
    /// Refuses an agent acting on an issue other than its run's.
    pub fn check_issue(&self, issue_id: &str) -> Result<(), ApiError> {
        match &self.issue_id {
            Some(own) if self.role == Role::Agent && own != issue_id => Err(ApiError::Forbidden(
                format!("an agent acts only on its run's issue, {own}"),
            )),
            _ => Ok(()),
        }
    }
}

impl From<&Actor> for punchlist_api::Actor {
    fn from(actor: &Actor) -> punchlist_api::Actor {
        punchlist_api::Actor {
            id: actor.id,
            name: actor.name.clone(),
            role: actor.role,
        }
    }
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
        let token = parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .ok_or(ApiError::Unauthorized)?;
        let row = sqlx::query!(
            r#"SELECT a.id, a.workspace_id, a.name, a.role,
                      r.issue_id AS "issue_id?", r.outcome AS "outcome?"
               FROM actor a LEFT JOIN run r ON r.id = a.run_id
               WHERE a.token_hash = $1"#,
            hash_token(token)
        )
        .fetch_optional(&state.pool)
        .await?
        .ok_or(ApiError::Unauthorized)?;
        let role = parse_role(&row.role)?;
        // An agent's token works only while its run is running.
        if role == Role::Agent && row.outcome.as_deref() != Some("running") {
            return Err(ApiError::Unauthorized);
        }
        // An agent's token reaches only its run's issue: `/api/issues/<id>` and below. Every
        // other endpoint, such as creating issues or reading the rest of the workspace, is
        // for people and runners.
        if role == Role::Agent {
            let own = row.issue_id.as_deref();
            let path = parts.uri.path().strip_prefix("/api/issues/");
            if own.is_none() || path.and_then(|rest| rest.split('/').next()) != own {
                return Err(ApiError::Forbidden(format!(
                    "an agent acts only on its run's issue, {}",
                    own.unwrap_or("none")
                )));
            }
        }
        Ok(Actor {
            id: row.id,
            workspace_id: row.workspace_id,
            role,
            name: row.name,
            issue_id: row.issue_id,
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
