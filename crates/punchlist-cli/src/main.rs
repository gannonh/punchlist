use std::process::ExitCode;

use clap::Parser;
use punchlist_cli::{Cli, run};

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let mut stdout = std::io::stdout().lock();
    match run(cli, &mut stdout).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}
