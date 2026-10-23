use anyhow::Context;
use clap::{Parser, Subcommand};
use punchlist_server::{
    AppState, Bootstrap, GithubClient, MIGRATOR, ProofStore, bootstrap, catch_up_merges_at_start,
    expire_leases, openapi, queue_workflow_load, queue_workflow_loads_at_start, router,
    warn_invalid_workflows, with_web_app, work_jobs, write_pull_request_events,
};
use sqlx::postgres::PgPoolOptions;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "punchlist-server", version, about = "The Punchlist server")]
struct Cli {
    /// Postgres connection string.
    #[arg(long, env = "DATABASE_URL", global = true, hide_env_values = true)]
    database_url: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run migrations and serve the API.
    Serve {
        /// Address to listen on.
        #[arg(long, env = "PUNCHLIST_BIND", default_value = "127.0.0.1:7878")]
        bind: String,
        /// The secret GitHub signs webhook deliveries with. Without it, or if it is empty, the
        /// webhook route answers 503.
        #[arg(long, env = "GITHUB_WEBHOOK_SECRET", hide_env_values = true)]
        github_webhook_secret: Option<String>,
        /// The GitHub App's id. With its private key, the server reads `.punchlist/` from
        /// the repository's default branch.
        #[arg(long, env = "GITHUB_APP_ID", requires = "github_app_private_key")]
        github_app_id: Option<String>,
        /// The GitHub App's private key, as PEM. One line is fine.
        #[arg(
            long,
            env = "GITHUB_APP_PRIVATE_KEY",
            hide_env_values = true,
            requires = "github_app_id"
        )]
        github_app_private_key: Option<String>,
        /// A test setting: report each run's GitHub token to expire this many seconds after
        /// it is minted, instead of GitHub's hour, so runners refresh it sooner.
        #[arg(long, env = "PUNCHLIST_GITHUB_TOKEN_LIFETIME", hide = true)]
        github_token_lifetime: Option<u64>,
        /// The directory proof files are kept in. A leading `~/` is the home directory.
        #[arg(
            long,
            env = "PUNCHLIST_PROOF_DIR",
            default_value = "~/.local/share/punchlist/proof"
        )]
        proof_dir: String,
        /// The largest proof file accepted, in bytes.
        #[arg(long, env = "PUNCHLIST_PROOF_MAX_BYTES", default_value_t = 104_857_600)]
        proof_max_bytes: u64,
        /// The web app's build. Served at `/` when it has an `index.html`.
        #[arg(long, env = "PUNCHLIST_WEB_DIR", default_value = "web/dist")]
        web_dir: std::path::PathBuf,
    },
    /// Print the OpenAPI document as JSON. Needs no database.
    Openapi,
    /// Create a workspace, its repository and one person, and print the person's token.
    Bootstrap {
        /// Workspace name.
        #[arg(long, default_value = "Punchlist")]
        workspace: String,
        /// Issue id prefix: `PL` gives PL-1, PL-2, ...
        #[arg(long, default_value = "PL")]
        prefix: String,
        /// The workspace's GitHub repository, as owner/name.
        #[arg(long, default_value = "gannonh/punchlist")]
        repository: String,
        /// The person's display name.
        #[arg(long)]
        person: String,
        /// Server URL to print in the `pl` config.
        #[arg(long, default_value = "http://127.0.0.1:7878")]
        server_url: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,sqlx=warn")),
        )
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();
    if let Command::Openapi = cli.command {
        println!("{}", serde_json::to_string_pretty(&openapi())?);
        return Ok(());
    }
    let database_url = cli
        .database_url
        .context("set DATABASE_URL or pass --database-url")?;
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&database_url)
        .await
        .context("connect to Postgres")?;
    MIGRATOR.run(&pool).await.context("run migrations")?;

    match cli.command {
        Command::Serve {
            bind,
            github_webhook_secret,
            github_app_id,
            github_app_private_key,
            github_token_lifetime,
            proof_dir,
            proof_max_bytes,
            web_dir,
        } => {
            let proof_dir = match proof_dir.strip_prefix("~/") {
                Some(rest) => std::path::PathBuf::from(
                    std::env::var_os("HOME").context("HOME is not set, so `~/` has no meaning")?,
                )
                .join(rest),
                None => proof_dir.into(),
            };
            std::fs::create_dir_all(&proof_dir)
                .with_context(|| format!("create {}", proof_dir.display()))?;
            warn_invalid_workflows(&pool)
                .await
                .context("read the active workflows")?;
            let expiry_pool = pool.clone();
            tokio::spawn(async move {
                let mut tick = tokio::time::interval(std::time::Duration::from_secs(5));
                loop {
                    tick.tick().await;
                    match expire_leases(&expiry_pool).await {
                        Ok(0) => {}
                        Ok(n) => tracing::warn!(attempts = n, "expired attempt leases"),
                        Err(error) => tracing::error!(%error, "expire leases"),
                    }
                }
            });
            // An open app sees recorded GitHub evidence within this interval plus the stream's
            // 500 ms poll.
            let events_pool = pool.clone();
            tokio::spawn(async move {
                let mut tick = tokio::time::interval(std::time::Duration::from_millis(250));
                loop {
                    tick.tick().await;
                    if let Err(error) = write_pull_request_events(&events_pool).await {
                        tracing::error!(%error, "write pull request events");
                    }
                }
            });
            let (stop, stopping) = tokio::sync::watch::channel(false);
            let mut state = AppState::new(pool)
                .with_proof(ProofStore {
                    dir: proof_dir,
                    max_bytes: proof_max_bytes,
                })
                .with_shutdown(stopping);
            // An empty secret is no secret: anyone could sign with it.
            match github_webhook_secret.filter(|secret| !secret.is_empty()) {
                Some(secret) => state = state.with_github_webhook_secret(secret),
                None => tracing::warn!("no GitHub webhook secret; the webhook route answers 503"),
            }
            match github_app_id.zip(github_app_private_key) {
                Some((id, key)) => {
                    let mut github = GithubClient::app(&id, &key)?;
                    if let Some(seconds) = github_token_lifetime {
                        github =
                            github.with_token_lifetime(std::time::Duration::from_secs(seconds));
                    }
                    state = state.with_github(github);
                    queue_workflow_loads_at_start(&state.pool, uuid::Uuid::new_v4())
                        .await
                        .context("queue the workflow loads")?;
                }
                None => tracing::warn!(
                    "no GitHub App credentials; the workflow cannot load from the repository"
                ),
            }
            catch_up_merges_at_start(&state.pool)
                .await
                .context("move the issues whose pull request merged")?;
            tokio::spawn(work_jobs(state.clone()));
            let app = with_web_app(router(state), &web_dir).layer(TraceLayer::new_for_http());
            let listener = tokio::net::TcpListener::bind(&bind)
                .await
                .with_context(|| format!("listen on {bind}"))?;
            tracing::info!("listening on http://{}", listener.local_addr()?);
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = tokio::signal::ctrl_c().await;
                    let _ = stop.send(true);
                })
                .await?;
        }
        Command::Bootstrap {
            workspace,
            prefix,
            repository,
            person,
            server_url,
        } => {
            let (owner, name) = repository
                .split_once('/')
                .context("--repository must be owner/name")?;
            let done = bootstrap(
                &pool,
                &Bootstrap {
                    workspace_name: workspace,
                    issue_prefix: prefix,
                    repository_owner: owner.to_string(),
                    repository_name: name.to_string(),
                    person_name: person,
                },
            )
            .await?;
            // The server loads the repository's workflow when it runs the job.
            let key = format!("workflow_load:bootstrap:{}", done.workspace_id);
            queue_workflow_load(&mut *pool.acquire().await?, done.workspace_id, &key).await?;
            eprintln!(
                "Created workspace {} and person {}. Save this as ~/.config/punchlist/config.toml; the token is not shown again.",
                done.workspace_id, done.actor_id
            );
            println!("server_url = \"{server_url}\"\ntoken = \"{}\"", done.token);
        }
        Command::Openapi => unreachable!("answered before connecting"),
    }
    Ok(())
}
