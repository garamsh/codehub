//! The `codehub` binary: parses the configuration and hands it to the composition site.

use std::io::Write;
use std::process::ExitCode;

use codehub::Cli;

#[tokio::main]
async fn main() -> ExitCode {
    match codehub::run(Cli::from_args()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // Nowhere left to report a failed stderr write.
            let _ = writeln!(std::io::stderr(), "codehub: {error}");
            ExitCode::FAILURE
        }
    }
}
