use std::process::ExitCode;

use clap::Parser;
use punchlist_cli::{Cli, run};

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    // Not locked: `pl mcp` writes to stdout from another thread.
    let mut stdout = std::io::stdout();
    match run(cli, &mut stdout).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}
