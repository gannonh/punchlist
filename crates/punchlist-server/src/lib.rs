//! The Punchlist server: the HTTP API over Postgres.

mod actors;
mod auth;
mod bootstrap;
mod cli_login;
pub mod db;
mod error;
mod events;
mod github;
mod github_api;
mod github_sign_in;
mod inbox;
mod invites;
mod issues;
mod jobs;
mod linear_import;
mod projects;
mod proof;
mod pull_requests;
mod rows;
mod runners;
mod runs;
mod search;
mod session;
mod workflow;
mod workspaces;

use auth::BearerAuth;
use axum::http::StatusCode;
use axum::routing::{any, get};
use axum::{Json, Router};
use sqlx::PgPool;
use tower_http::services::{ServeDir, ServeFile};
use utoipa::OpenApi;
use utoipa_axum::router::OpenApiRouter;

pub use auth::hash_token;
pub use bootstrap::{Bootstrap, Bootstrapped, bootstrap, join};
pub use error::ApiError;
pub use events::write_pull_request_events;
pub use github::catch_up_at_start;
pub use github_api::GithubClient;
pub use github_sign_in::GithubSignIn;
pub use jobs::{run_jobs_until_idle, run_next_job, work_jobs};
pub use runs::expire_leases;
pub use workflow::{queue_workflow_load, queue_workflow_loads_at_start, warn_invalid_workflows};

/// The migrations in `migrations/`, run when the server starts.
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!();

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    /// How long a lease lasts without a heartbeat.
    pub lease: std::time::Duration,
    /// Sent after an issue becomes claimable, to wake runners that are long-polling for a
    /// claim: it entered Start or a status with a dispatch rule, or a run of it ended, which
    /// frees a dispatch that waited behind the run or queues the next one.
    pub starts: tokio::sync::broadcast::Sender<(uuid::Uuid, String)>,
    /// The secret GitHub signs webhook deliveries with. Without it the webhook route
    /// answers 503.
    pub github_webhook_secret: Option<std::sync::Arc<str>>,
    /// Notified after a job is enqueued, to wake the job worker.
    pub job_wake: std::sync::Arc<tokio::sync::Notify>,
    /// Reads `.punchlist/` from the repository. Without it a workflow load fails.
    pub github: Option<GithubClient>,
    /// Where proof files are kept, and how large one may be.
    pub proof: ProofStore,
    /// Turns true when the server begins to shut down. Open event streams end on it, since
    /// graceful shutdown waits for every open response and a stream never finishes by itself.
    pub shutdown: tokio::sync::watch::Receiver<bool>,
    /// Sign-in with GitHub (ADR 0013). Without it the sign-in page offers the token alone.
    pub github_sign_in: Option<std::sync::Arc<GithubSignIn>>,
    /// The instance's own origin, such as `https://sartre.tail984796.ts.net:7880`: the GitHub
    /// callback's and the only `Origin` a browser's write may come from. Without it both come
    /// from the request's `Host` (or `X-Forwarded-Host`).
    pub public_url: Option<std::sync::Arc<str>>,
    /// How long a `pl auth login` waits for a person's approval: 10 minutes.
    pub cli_login_lifetime: std::time::Duration,
}

/// Proof files on local disk, each named by its storage key.
#[derive(Clone, Debug)]
pub struct ProofStore {
    pub dir: std::path::PathBuf,
    pub max_bytes: u64,
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
            proof: ProofStore {
                dir: std::env::temp_dir().join("punchlist-proof"),
                max_bytes: 100 * 1024 * 1024,
            },
            shutdown: tokio::sync::watch::channel(false).1,
            github_sign_in: None,
            public_url: None,
            cli_login_lifetime: std::time::Duration::from_secs(600),
        }
    }

    pub fn with_github_sign_in(mut self, github_sign_in: GithubSignIn) -> AppState {
        self.github_sign_in = Some(std::sync::Arc::new(github_sign_in));
        self
    }

    /// `url` is an origin, such as `https://example.com:7880`, with no path.
    pub fn with_public_url(mut self, url: &str) -> AppState {
        self.public_url = Some(url.trim_end_matches('/').into());
        self
    }

    pub fn with_cli_login_lifetime(mut self, lifetime: std::time::Duration) -> AppState {
        self.cli_login_lifetime = lifetime;
        self
    }

    pub fn with_shutdown(mut self, shutdown: tokio::sync::watch::Receiver<bool>) -> AppState {
        self.shutdown = shutdown;
        self
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

    pub fn with_proof(mut self, proof: ProofStore) -> AppState {
        self.proof = proof;
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

/// Every API route, registered with its OpenAPI path (ADR 0008). `proof_max_bytes` sizes
/// the proof upload's body limit.
fn api_routes(proof_max_bytes: u64) -> OpenApiRouter<AppState> {
    OpenApiRouter::with_openapi(ApiDoc::openapi())
        .merge(actors::routes())
        .merge(cli_login::routes())
        .merge(events::routes())
        .merge(github::routes())
        .merge(inbox::routes())
        .merge(invites::routes())
        .merge(issues::routes())
        .merge(linear_import::routes())
        .merge(projects::routes())
        .merge(proof::routes(proof_max_bytes))
        .merge(pull_requests::routes())
        .merge(runners::routes())
        .merge(runs::routes())
        .merge(search::routes())
        .merge(session::routes())
        .merge(github_sign_in::routes())
        .merge(workflow::routes())
        .merge(workspaces::routes())
}

/// The API router, which also serves the OpenAPI document built from its routes at
/// `/api/openapi.json`.
pub fn router(state: AppState) -> Router {
    let (router, api) = routes_and_document(state.proof.max_bytes);
    router
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::same_origin,
        ))
        .with_state(state)
        .route("/api/openapi.json", get(move || async move { Json(api) }))
}

/// The OpenAPI document, built from the routes without a database.
pub fn openapi() -> utoipa::openapi::OpenApi {
    routes_and_document(0).1
}

fn routes_and_document(proof_max_bytes: u64) -> (Router<AppState>, utoipa::openapi::OpenApi) {
    let (router, mut api) = api_routes(proof_max_bytes).split_for_parts();
    (workspaces::with_workspace_routes(router, &mut api), api)
}

/// `router` serving the web app's build in `dir` too: its files at `/`, and its
/// `index.html` for any other path outside `/api/`, so the app's own routes load it. Without
/// an `index.html` in `dir` it is the API alone.
pub fn with_web_app(router: Router, dir: &std::path::Path) -> Router {
    let index = dir.join("index.html");
    if !index.is_file() {
        tracing::warn!(
            "no {}; serving the API without the web app",
            index.display()
        );
        return router;
    }
    let app = ServeDir::new(dir).fallback(ServeFile::new(index));
    router
        .route("/api/{*rest}", any(|| async { StatusCode::NOT_FOUND }))
        .fallback_service(app)
}
