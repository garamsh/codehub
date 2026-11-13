//! The `codehub-runner` binary's library and its composition site: claims the checks of a
//! codehub server and runs them on this machine.

pub mod config;
pub mod core;
pub mod executor;
pub mod gaon;
pub mod host;
pub mod http_source;
pub mod job_source;
pub mod run_job;
pub mod token;

use std::fmt;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;

pub use crate::config::Config;
use crate::config::{Command, ExecutorKind, GaonConfig};
use crate::core::Mode;
use crate::executor::ExecutorError;
use crate::gaon::GaonExecutor;
use crate::gaon::api::Api;
use crate::gaon::results::Results;
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
    /// A setting the executor needs is not given.
    Missing(&'static str),
    /// The certificate authority file could not be used.
    CaFile {
        /// The file.
        path: PathBuf,
        /// What went wrong.
        reason: String,
    },
    /// The result listener could not bind its address.
    Listen {
        /// The address.
        address: SocketAddr,
        /// What went wrong.
        error: std::io::Error,
    },
    /// The gaon driver could not start.
    Gaon(ExecutorError),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Secret(name) => write!(f, "the environment variable {name} is not set"),
            Self::Claim(error) => write!(f, "claiming a job failed: {error}"),
            Self::Missing(flag) => write!(f, "{flag} is required"),
            Self::CaFile { path, reason } => {
                write!(f, "the CA file {} is unusable: {reason}", path.display())
            }
            Self::Listen { address, error } => {
                write!(f, "listening for results on {address} failed: {error}")
            }
            Self::Gaon(error) => write!(f, "starting the gaon driver failed: {error}"),
        }
    }
}

impl std::error::Error for Error {}

/// Runs the runner as `config` says.
///
/// This is the composition site: it builds the token source, the HTTP job source and the
/// executor `--executor` names, and hands them to the loop.
///
/// # Errors
///
/// An [`Error`] where a setting is missing or unusable, where the gaon driver cannot start, or
/// where `--once` could not claim.
pub async fn run(config: Config) -> Result<ExitCode, Error> {
    if let Some(Command::RunJob { job }) = &config.command {
        return Ok(run_job::run(job).await);
    }
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
    let server = config.server.ok_or(Error::Missing("--server"))?;
    let source = HttpJobSource::new(http.clone(), &server, tokens);
    let mode = if config.once {
        Mode::Once
    } else {
        Mode::Forever
    };
    match config.executor {
        ExecutorKind::Host => {
            let work_dir = config.work_dir.unwrap_or_else(std::env::temp_dir);
            let executor = HostExecutor::new(work_dir, http, hidden);
            core::run(&source, &executor, mode)
                .await
                .map_err(Error::Claim)?;
        }
        ExecutorKind::Gaon => {
            let executor = gaon_executor(config.gaon).await?;
            core::run(&source, &executor, mode)
                .await
                .map_err(Error::Claim)?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// Builds the gaon driver, starts its result listener, and removes what an earlier run left.
async fn gaon_executor(gaon: GaonConfig) -> Result<GaonExecutor, Error> {
    let url = gaon.url.ok_or(Error::Missing("--gaon-url"))?;
    let ca_file = gaon.ca_file.ok_or(Error::Missing("--gaon-ca-file"))?;
    let token_file = gaon.token_file.ok_or(Error::Missing("--gaon-token-file"))?;
    let image = gaon.image.ok_or(Error::Missing("--gaon-image"))?;
    let listen = gaon
        .result_listen
        .ok_or(Error::Missing("--gaon-result-listen"))?;
    let result_url = gaon.result_url.ok_or(Error::Missing("--gaon-result-url"))?;
    let runner = gaon
        .runner_name
        .ok_or(Error::Missing("--gaon-runner-name"))?;
    let ca_error = |reason: String| Error::CaFile {
        path: ca_file.clone(),
        reason,
    };
    let pem = tokio::fs::read(&ca_file)
        .await
        .map_err(|error| ca_error(error.to_string()))?;
    let certificate =
        reqwest::Certificate::from_pem(&pem).map_err(|error| ca_error(error.to_string()))?;
    let http = reqwest::Client::builder()
        .tls_certs_only([certificate])
        .build()
        .map_err(|error| ca_error(error.to_string()))?;
    let api = Api::new(http, &url, TokenSource::File(token_file), runner);
    let results = Results::default();
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .map_err(|error| Error::Listen {
            address: listen,
            error,
        })?;
    let app = results.router();
    tokio::spawn(async move {
        if let Err(error) = axum::serve(listener, app).await {
            eprintln!("the result listener ended: {error}");
        }
    });
    let executor = GaonExecutor::new(api, image, gaon.job_user, &result_url, results);
    executor.clean().await.map_err(Error::Gaon)?;
    Ok(executor)
}
