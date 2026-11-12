//! The `codehub-runner` binary's library and its composition site: claims the checks of a
//! codehub server and runs them on this machine.

pub mod config;
pub mod core;
pub mod executor;
pub mod host;
pub mod http_source;
pub mod job_source;
pub mod token;

use std::fmt;
use std::path::PathBuf;

pub use crate::config::Config;
use crate::core::Mode;
use crate::host::HostExecutor;
use crate::http_source::HttpJobSource;
use crate::job_source::SourceError;
use crate::token::{ClientCredentials, Secret, TokenSource};

/// Why the runner stopped.
#[derive(Debug)]
pub enum Error {
    /// The environment variable that should hold the client secret is not set.
    Secret(String),
    /// A claim failed in `--once` mode.
    Claim(SourceError),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Secret(name) => write!(f, "the environment variable {name} is not set"),
            Self::Claim(error) => write!(f, "claiming a job failed: {error}"),
        }
    }
}

impl std::error::Error for Error {}

/// Runs the runner as `config` says.
///
/// This is the composition site: it builds the token source, the HTTP job source and the host
/// executor, and hands them to the loop.
///
/// # Errors
///
/// An [`Error`] where the client secret is missing, or where `--once` could not claim.
pub async fn run(config: Config) -> Result<(), Error> {
    let http = reqwest::Client::new();
    let mut hidden = None;
    let tokens = match (config.token_file, config.token_endpoint) {
        (Some(path), _) => TokenSource::File(path),
        (None, Some(endpoint)) => {
            // The command line requires both; an absent one cannot get here.
            let id = config.client_id.unwrap_or_default();
            let name = config.client_secret_env.unwrap_or_default();
            let secret = std::env::var(&name).map_err(|_| Error::Secret(name.clone()))?;
            hidden = Some(name);
            TokenSource::Credentials(ClientCredentials::new(
                http.clone(),
                endpoint,
                id,
                Secret::new(secret),
            ))
        }
        (None, None) => TokenSource::File(PathBuf::new()),
    };
    let source = HttpJobSource::new(http.clone(), &config.server, tokens);
    let work_dir = config.work_dir.unwrap_or_else(std::env::temp_dir);
    let executor = HostExecutor::new(work_dir, http, hidden);
    let mode = if config.once {
        Mode::Once
    } else {
        Mode::Forever
    };
    core::run(&source, &executor, mode)
        .await
        .map_err(Error::Claim)
}
