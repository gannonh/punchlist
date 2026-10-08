use anyhow::Context;
use clap::{Parser, Subcommand};
use punchlist_server::{AppState, Bootstrap, MIGRATOR, bootstrap, router};
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
    },
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
        Command::Serve { bind } => {
            let app = router(AppState { pool }).layer(TraceLayer::new_for_http());
            let listener = tokio::net::TcpListener::bind(&bind)
                .await
                .with_context(|| format!("listen on {bind}"))?;
            tracing::info!("listening on http://{}", listener.local_addr()?);
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = tokio::signal::ctrl_c().await;
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
            eprintln!(
                "Created workspace {} and person {}. Save this as ~/.config/punchlist/config.toml; the token is not shown again.",
                done.workspace_id, done.actor_id
            );
            println!("server_url = \"{server_url}\"\ntoken = \"{}\"", done.token);
        }
    }
    Ok(())
}
