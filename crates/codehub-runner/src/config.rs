//! The runner's settings, read from flags and the environment.

use std::fmt;
use std::path::PathBuf;

use std::net::SocketAddr;

use clap::{ArgGroup, Args, Parser, Subcommand, ValueEnum};

/// The label every object the gaon driver creates carries, with the runner's name as its value.
pub const RUNNER_LABEL: &str = "runner.codehub.garam.sh/name";

/// Where the runner runs a job's command.
#[derive(Clone, Copy, PartialEq, Eq, Debug, ValueEnum)]
pub enum ExecutorKind {
    /// As a process of this machine.
    Host,
    /// In a MicroVM on gaon.
    Gaon,
}

/// What the runner does instead of claiming jobs.
#[derive(Subcommand, Debug)]
pub enum Command {
    /// Runs the one job a file describes and posts its end to the runner that wrote the file; the
    /// gaon driver runs this inside a MicroVM.
    RunJob {
        /// The job description.
        #[arg(long)]
        job: PathBuf,
    },
}

/// The gaon driver's settings, each required where the executor is `gaon`.
#[derive(Args, Debug)]
pub struct GaonConfig {
    /// The gaon API server's root URL: `https://`, ending in `/apis/compute.gaon.garam.sh/v1alpha1`.
    #[arg(
        long = "gaon-url",
        env = "CODEHUB_RUNNER_GAON_URL",
        value_parser = https_url,
        required_if_eq("executor", "gaon")
    )]
    pub url: Option<String>,

    /// The PEM file of the certificate authority that signed the API server's certificate.
    #[arg(
        long = "gaon-ca-file",
        env = "CODEHUB_RUNNER_GAON_CA_FILE",
        required_if_eq("executor", "gaon")
    )]
    pub ca_file: Option<PathBuf>,

    /// A file holding the gaon service-account token, read again before each request.
    #[arg(
        id = "gaon_token_file",
        long = "gaon-token-file",
        env = "CODEHUB_RUNNER_GAON_TOKEN_FILE",
        required_if_eq("executor", "gaon")
    )]
    pub token_file: Option<PathBuf>,

    /// The image jobs run in, `<repository>@sha256:<digest>`.
    #[arg(
        long = "gaon-image",
        env = "CODEHUB_RUNNER_GAON_IMAGE",
        value_parser = image_by_digest,
        required_if_eq("executor", "gaon")
    )]
    pub image: Option<String>,

    /// The address the runner listens on for the results of jobs.
    #[arg(
        long = "gaon-result-listen",
        env = "CODEHUB_RUNNER_GAON_RESULT_LISTEN",
        required_if_eq("executor", "gaon")
    )]
    pub result_listen: Option<SocketAddr>,

    /// The URL a job reaches the result listener at.
    #[arg(
        long = "gaon-result-url",
        env = "CODEHUB_RUNNER_GAON_RESULT_URL",
        value_parser = http_url,
        required_if_eq("executor", "gaon")
    )]
    pub result_url: Option<String>,

    /// The name that labels what this runner creates on gaon: a valid label value, unique among
    /// the runners of one gaon cluster.
    #[arg(
        long = "gaon-runner-name",
        env = "CODEHUB_RUNNER_GAON_RUNNER_NAME",
        value_parser = label_value,
        required_if_eq("executor", "gaon")
    )]
    pub runner_name: Option<String>,

    /// The numeric `uid:gid` a job's MicroVM runs as: it owns the description file, and must be
    /// the user of the image.
    #[arg(
        long = "gaon-job-user",
        env = "CODEHUB_RUNNER_GAON_JOB_USER",
        default_value = "10001:10001"
    )]
    pub job_user: JobUser,
}

/// A numeric `uid:gid`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobUser(String);

impl JobUser {
    /// The text gaon takes as `spec.user`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::str::FromStr for JobUser {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, String> {
        let numeric = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
        match text.split_once(':') {
            Some((uid, gid)) if numeric(uid) && numeric(gid) => Ok(Self(text.to_owned())),
            _ => Err("a job user is a numeric <uid>:<gid>, as 10001:10001".to_owned()),
        }
    }
}

fn https_url(text: &str) -> Result<String, String> {
    if text.starts_with("https://") {
        Ok(text.to_owned())
    } else {
        Err("a bearer token goes only over https".to_owned())
    }
}

fn http_url(text: &str) -> Result<String, String> {
    if text.starts_with("http://") || text.starts_with("https://") {
        Ok(text.to_owned())
    } else {
        Err("a URL starts with http:// or https://".to_owned())
    }
}

fn image_by_digest(text: &str) -> Result<String, String> {
    match text.rsplit_once("@sha256:") {
        Some((repository, hex))
            if !repository.is_empty()
                && hex.len() == 64
                && hex.bytes().all(|b| b.is_ascii_hexdigit()) =>
        {
            Ok(text.to_owned())
        }
        _ => Err("an image is <repository>@sha256:<64 hex digits>".to_owned()),
    }
}

/// A gaon label value, not empty: at most 63 characters of alphanumerics, `-`, `_` and `.`,
/// beginning and ending with an alphanumeric.
fn label_value(text: &str) -> Result<String, String> {
    let edge = |c: Option<char>| c.is_some_and(|c| c.is_ascii_alphanumeric());
    let body = text
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if text.len() <= 63 && body && edge(text.chars().next()) && edge(text.chars().next_back()) {
        Ok(text.to_owned())
    } else {
        Err("a runner name is 1 to 63 alphanumerics, '-', '_' or '.', beginning and ending with an alphanumeric".to_owned())
    }
}

/// The runner's settings.
#[derive(Parser)]
#[command(
    name = "codehub-runner",
    about = "Runs the checks of a codehub server on this machine",
    subcommand_negates_reqs = true,
    group = ArgGroup::new("credentials").required(true).args(["token_file", "token_endpoint"]),
)]
pub struct Config {
    /// A mode other than claiming jobs.
    #[command(subcommand)]
    pub command: Option<Command>,

    /// The server's base URL.
    #[arg(long, env = "CODEHUB_RUNNER_SERVER", required = true)]
    pub server: Option<String>,

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

    /// Where a job's command runs.
    #[arg(
        long,
        env = "CODEHUB_RUNNER_EXECUTOR",
        value_enum,
        default_value = "host"
    )]
    pub executor: ExecutorKind,

    /// The gaon driver's settings.
    #[command(flatten)]
    pub gaon: GaonConfig,
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self {
            command,
            server,
            token_file,
            token_endpoint,
            client_id,
            client_secret_env,
            work_dir,
            once,
            executor,
            gaon,
        } = self;
        f.debug_struct("Config")
            .field("command", command)
            .field("server", server)
            .field("token_file", token_file)
            .field("token_endpoint", token_endpoint)
            .field("client_id", client_id)
            .field("client_secret_env", client_secret_env)
            .field("work_dir", work_dir)
            .field("once", once)
            .field("executor", executor)
            .field("gaon", gaon)
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
    fn the_client_credentials_come_whole() {
        let without_secret = [
            "--server",
            "http://s",
            "--token-endpoint",
            "http://t",
            "--client-id",
            "id",
        ];
        assert!(parse(&without_secret).is_err());
        let without_id = [
            "--server",
            "http://s",
            "--token-endpoint",
            "http://t",
            "--client-secret-env",
            "S",
        ];
        assert!(parse(&without_id).is_err());
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

    const IMAGE: &str =
        "r.example/runner@sha256:0000000000000000000000000000000000000000000000000000000000000000";

    /// The gaon options and their values, in the order of the flags.
    fn gaon() -> Vec<(&'static str, &'static str)> {
        vec![
            (
                "--gaon-url",
                "https://gaon/apis/compute.gaon.garam.sh/v1alpha1",
            ),
            ("--gaon-ca-file", "/ca.pem"),
            ("--gaon-token-file", "/gaon-token"),
            ("--gaon-image", IMAGE),
            ("--gaon-result-listen", "0.0.0.0:8090"),
            ("--gaon-result-url", "http://runner:8090"),
            ("--gaon-runner-name", "runner-a"),
        ]
    }

    fn with_gaon(options: &[(&str, &str)]) -> Result<Config, clap::Error> {
        let mut args = vec![
            "--server",
            "http://s",
            "--token-file",
            "/t",
            "--executor",
            "gaon",
        ];
        for (flag, value) in options {
            args.extend([*flag, *value]);
        }
        parse(&args)
    }

    #[test]
    fn the_host_executor_is_the_default_and_needs_no_gaon_setting() {
        let config = parse(&["--server", "http://s", "--token-file", "/t"]).unwrap();
        assert_eq!(config.executor, ExecutorKind::Host);
        assert!(config.gaon.url.is_none());
    }

    #[test]
    fn the_gaon_executor_takes_all_its_settings() {
        let config = with_gaon(&gaon()).unwrap();
        assert_eq!(config.executor, ExecutorKind::Gaon);
        assert_eq!(config.gaon.runner_name.as_deref(), Some("runner-a"));
        assert_eq!(config.gaon.image.as_deref(), Some(IMAGE));
        assert_eq!(config.gaon.token_file, Some(PathBuf::from("/gaon-token")));
    }

    #[test]
    fn the_gaon_executor_is_refused_without_any_one_of_its_settings() {
        for missing in 0..gaon().len() {
            let mut options = gaon();
            let (flag, _) = options.remove(missing);
            let error = with_gaon(&options).unwrap_err().to_string();
            assert!(error.contains(flag), "{flag} is named: {error}");
        }
    }

    #[test]
    fn a_gaon_url_not_over_https_an_image_not_by_digest_and_a_bad_name_are_refused() {
        let refused = |flag: &str, value: &'static str| {
            let mut options = gaon();
            for option in &mut options {
                if option.0 == flag {
                    option.1 = value;
                }
            }
            with_gaon(&options).is_err()
        };
        let long = "a".repeat(64).leak();
        assert!(refused("--gaon-url", "http://gaon/apis"));
        assert!(refused("--gaon-image", "r.example/runner:latest"));
        assert!(refused("--gaon-image", "r.example/runner@sha256:abc"));
        assert!(refused("--gaon-runner-name", "-edge"));
        assert!(refused("--gaon-runner-name", "has space"));
        assert!(refused("--gaon-runner-name", long));
        assert!(!refused("--gaon-runner-name", "ok.name_1-b"));
    }

    #[test]
    fn the_job_user_defaults_to_the_image_user_and_takes_only_a_numeric_uid_and_gid() {
        let config = with_gaon(&gaon()).unwrap();
        assert_eq!(config.gaon.job_user.as_str(), "10001:10001");
        let with_user = |user: &'static str| {
            let mut options = gaon();
            options.push(("--gaon-job-user", user));
            with_gaon(&options)
        };
        assert_eq!(
            with_user("1000:2000").unwrap().gaon.job_user.as_str(),
            "1000:2000"
        );
        for refused in [
            "root",
            "root:root",
            "1000",
            "1000:",
            ":1000",
            "1000:wheel",
            "-1:0",
        ] {
            assert!(with_user(refused).is_err(), "{refused} is refused");
        }
    }

    #[test]
    fn run_job_needs_neither_a_server_nor_credentials() {
        let config = parse(&["run-job", "--job", "/j.json"]).unwrap();
        assert!(matches!(
            config.command,
            Some(Command::RunJob { job }) if job.as_os_str() == "/j.json"
        ));
        assert!(config.server.is_none());
        assert!(parse(&["run-job"]).is_err(), "the job file is required");
    }
}
