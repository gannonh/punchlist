//! The Punchlist server: the HTTP API over Postgres.

mod auth;
mod bootstrap;
mod error;
mod github;
mod github_api;
mod issues;
mod jobs;
mod pull_requests;
mod runners;
mod runs;
mod workflow;

use auth::BearerAuth;
use axum::{Json, Router, routing::get};
use sqlx::PgPool;
use utoipa::OpenApi;
use utoipa_axum::router::OpenApiRouter;

pub use auth::hash_token;
pub use bootstrap::{Bootstrap, Bootstrapped, bootstrap};
pub use error::ApiError;
pub use github_api::GithubClient;
pub use jobs::{run_jobs_until_idle, run_next_job, work_jobs};
pub use runs::expire_leases;
pub use workflow::queue_workflow_load;

/// The migrations in `migrations/`, run when the server starts.
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!();

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    /// How long a lease lasts without a heartbeat.
    pub lease: std::time::Duration,
    /// Sent after an issue is committed into Start, to wake runners that are long-polling.
    pub starts: tokio::sync::broadcast::Sender<(uuid::Uuid, String)>,
    /// The secret GitHub signs webhook deliveries with. Without it the webhook route
    /// answers 503.
    pub github_webhook_secret: Option<std::sync::Arc<str>>,
    /// Notified after a job is enqueued, to wake the job worker.
    pub job_wake: std::sync::Arc<tokio::sync::Notify>,
    /// Reads `.punchlist/` from the repository. Without it a workflow load fails.
    pub github: Option<GithubClient>,
}

impl AppState {
    pub fn new(pool: PgPool) -> AppState {
        AppState {
            pool,
            lease: std::time::Duration::from_secs(30),
            starts: tokio::sync::broadcast::channel(256).0,
            github_webhook_secret: None,
            job_wake: std::sync::Arc::new(tokio::sync::Notify::new()),
            github: None,
        }
    }

    pub fn with_github(mut self, github: GithubClient) -> AppState {
        self.github = Some(github);
        self
    }

    pub fn with_github_webhook_secret(
        mut self,
        secret: impl Into<std::sync::Arc<str>>,
    ) -> AppState {
        self.github_webhook_secret = Some(secret.into());
        self
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
        .merge(github::routes())
        .merge(issues::routes())
        .merge(pull_requests::routes())
        .merge(runners::routes())
        .merge(runs::routes())
        .merge(workflow::routes())
        .with_state(state)
        .split_for_parts();
    router.route("/api/openapi.json", get(move || async move { Json(api) }))
}
