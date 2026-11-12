//! The runner's settings, read from flags and the environment.

use std::fmt;
use std::path::PathBuf;

use clap::{ArgGroup, Parser};

/// The runner's settings.
#[derive(Parser)]
#[command(
    name = "codehub-runner",
    about = "Runs the checks of a codehub server on this machine",
    group = ArgGroup::new("credentials").required(true).args(["token_file", "token_endpoint"]),
)]
pub struct Config {
    /// The server's base URL.
    #[arg(long, env = "CODEHUB_RUNNER_SERVER")]
    pub server: String,

    /// A file holding the bearer token, read again before each request.
    #[arg(long, env = "CODEHUB_RUNNER_TOKEN_FILE")]
    pub token_file: Option<PathBuf>,

    /// The OAuth token endpoint to ask for tokens with the client-credentials grant.
    #[arg(
        long,
        env = "CODEHUB_RUNNER_TOKEN_ENDPOINT",
        requires_all = ["client_id", "client_secret_env"]
    )]
    pub token_endpoint: Option<String>,

    /// The OAuth client id.
    #[arg(long, env = "CODEHUB_RUNNER_CLIENT_ID", requires = "token_endpoint")]
    pub client_id: Option<String>,

    /// The name of the environment variable that holds the OAuth client secret.
    #[arg(
        long,
        env = "CODEHUB_RUNNER_CLIENT_SECRET_ENV",
        requires = "token_endpoint"
    )]
    pub client_secret_env: Option<String>,

    /// Where jobs' directories are made; the system's temporary directory where absent.
    #[arg(long, env = "CODEHUB_RUNNER_WORK_DIR")]
    pub work_dir: Option<PathBuf>,

    /// Claim once, run the job it returns, and exit.
    #[arg(long)]
    pub once: bool,
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self {
            server,
            token_file,
            token_endpoint,
            client_id,
            client_secret_env,
            work_dir,
            once,
        } = self;
        f.debug_struct("Config")
            .field("server", server)
            .field("token_file", token_file)
            .field("token_endpoint", token_endpoint)
            .field("client_id", client_id)
            .field("client_secret_env", client_secret_env)
            .field("work_dir", work_dir)
            .field("once", once)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Config, clap::Error> {
        Config::try_parse_from(std::iter::once("codehub-runner").chain(args.iter().copied()))
    }

    #[test]
    fn a_token_file_is_enough() {
        let config = parse(&["--server", "http://s", "--token-file", "/t", "--once"]).unwrap();
        assert!(config.once);
        assert_eq!(config.token_file, Some(PathBuf::from("/t")));
    }

    #[test]
    fn a_token_endpoint_needs_its_client() {
        let args = ["--server", "http://s", "--token-endpoint", "http://t"];
        assert!(parse(&args).is_err());
        let full = [
            "--server",
            "http://s",
            "--token-endpoint",
            "http://t",
            "--client-id",
            "id",
            "--client-secret-env",
            "SECRET",
        ];
        assert!(parse(&full).is_ok());
    }

    #[test]
    fn a_source_of_tokens_is_required_and_only_one_is_allowed() {
        assert!(parse(&["--server", "http://s"]).is_err());
        let both = [
            "--server",
            "http://s",
            "--token-file",
            "/t",
            "--token-endpoint",
            "http://t",
            "--client-id",
            "id",
            "--client-secret-env",
            "S",
        ];
        assert!(parse(&both).is_err());
    }
}
