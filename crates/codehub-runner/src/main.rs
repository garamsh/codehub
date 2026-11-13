//! The `codehub-runner` binary: parses the configuration and hands it to the composition site.

use std::io::Write;
use std::process::ExitCode;

use clap::Parser;
use codehub_runner::Config;

#[tokio::main]
async fn main() -> ExitCode {
    match codehub_runner::run(Config::parse()).await {
        Ok(code) => code,
        Err(error) => {
            // Nowhere left to report a failed stderr write.
            let _ = writeln!(std::io::stderr(), "codehub-runner: {error}");
            ExitCode::FAILURE
        }
    }
}
