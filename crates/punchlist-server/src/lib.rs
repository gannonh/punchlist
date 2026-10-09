//! The Punchlist server: the HTTP API over Postgres.

mod auth;
mod bootstrap;
mod error;
mod issues;
mod pull_requests;
mod runners;
mod runs;

use auth::BearerAuth;
use axum::{Json, Router, routing::get};
use sqlx::PgPool;
use utoipa::OpenApi;
use utoipa_axum::router::OpenApiRouter;

pub use auth::hash_token;
pub use bootstrap::{Bootstrap, Bootstrapped, bootstrap};
pub use error::ApiError;
pub use runs::expire_leases;

/// The migrations in `migrations/`, run when the server starts.
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!();

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    /// How long a lease lasts without a heartbeat.
    pub lease: std::time::Duration,
    /// Sent after an issue is committed into Start, to wake runners that are long-polling.
    pub starts: tokio::sync::broadcast::Sender<(uuid::Uuid, String)>,
}

impl AppState {
    pub fn new(pool: PgPool) -> AppState {
        AppState {
            pool,
            lease: std::time::Duration::from_secs(30),
            starts: tokio::sync::broadcast::channel(256).0,
        }
    }

    pub fn with_lease(mut self, lease: std::time::Duration) -> AppState {
        self.lease = lease;
        self
    }
}

#[derive(OpenApi)]
#[openapi(
    info(title = "Punchlist", description = "The Punchlist API."),
    modifiers(&BearerAuth),
    security(("bearer" = []))
)]
struct ApiDoc;

/// The API router and the OpenAPI document built from the same routes (ADR 0008).
pub fn router(state: AppState) -> Router {
    let (router, api) = OpenApiRouter::with_openapi(ApiDoc::openapi())
        .merge(issues::routes())
        .merge(pull_requests::routes())
        .merge(runners::routes())
        .merge(runs::routes())
        .with_state(state)
        .split_for_parts();
    router.route("/api/openapi.json", get(move || async move { Json(api) }))
}
