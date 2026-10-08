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
    pub role: Role,
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
            "SELECT id, workspace_id, role FROM actor WHERE token_hash = $1",
            hash_token(token)
        )
        .fetch_optional(&state.pool)
        .await?
        .ok_or(ApiError::Unauthorized)?;
        let role = Role::parse(&row.role).ok_or(ApiError::Unauthorized)?;
        Ok(Actor {
            id: row.id,
            workspace_id: row.workspace_id,
            role,
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
