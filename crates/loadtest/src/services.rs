//! What a scenario runs against: a fresh database and bucket, started `codehub` replicas, and
//! tokens from Keycloak. Everything the harness creates is named `loadtest` and removed at the
//! end.

use std::path::Path;
use std::process::{ExitStatus, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::Url;
use rusty_s3::{Credentials, S3Action, UrlStyle};
use sqlx::{AssertSqlSafe, Connection, Executor, PgConnection};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

use crate::Failure;

/// How long a start or a request to a service may take.
const BOUND: Duration = Duration::from_secs(60);

/// The value of the environment variable `name`, which `dev/check-env.sh` sets.
///
/// # Errors
///
/// Fails where it is not set.
pub fn env(name: &str) -> Result<String, Failure> {
    std::env::var(name)
        .map_err(|_| Failure::new(format!("{name} is not set; run through `make loadtest`")))
}

/// A name no other run uses: `prefix`, the process and the clock.
fn unique(prefix: &str, separator: char) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    format!(
        "{prefix}{separator}{}{separator}{nanos}",
        std::process::id()
    )
}

/// A database created for one scenario.
pub struct Database {
    server: String,
    name: String,
}

impl Database {
    /// Creates an empty database named `loadtest_<process>_<time>` on the server
    /// `CODEHUB_TEST_POSTGRES_URL` names.
    ///
    /// # Errors
    ///
    /// Fails where the variable is not set or the server cannot create the database.
    pub async fn create() -> Result<Self, Failure> {
        let server = env("CODEHUB_TEST_POSTGRES_URL")?;
        let name = unique("loadtest", '_');
        let mut admin = PgConnection::connect(&server).await?;
        admin
            .execute(AssertSqlSafe(format!("create database {name}")))
            .await?;
        Ok(Self { server, name })
    }

    /// The URL of the database: the server's with the database replaced.
    fn url(&self) -> Result<String, Failure> {
        let (base, _) = self
            .server
            .rsplit_once('/')
            .ok_or_else(|| Failure::new("CODEHUB_TEST_POSTGRES_URL names no database"))?;
        Ok(format!("{base}/{}", self.name))
    }

    /// Drops the database, closing the connections still open to it.
    ///
    /// # Errors
    ///
    /// Fails where the server cannot drop it.
    pub async fn remove(self) -> Result<(), Failure> {
        let mut admin = PgConnection::connect(&self.server).await?;
        admin
            .execute(AssertSqlSafe(format!(
                "drop database if exists {} with (force)",
                self.name
            )))
            .await?;
        Ok(())
    }
}

/// A bucket created for one scenario, on the S3 service of the check environment.
pub struct Bucket {
    http: reqwest::Client,
    bucket: rusty_s3::Bucket,
    credentials: Credentials,
}

impl Bucket {
    /// Creates a bucket named `loadtest-<process>-<time>`.
    ///
    /// # Errors
    ///
    /// Fails where a variable is not set or the service refuses.
    pub async fn create() -> Result<Self, Failure> {
        let endpoint: Url = env("CODEHUB_TEST_S3_ENDPOINT")?
            .parse()
            .map_err(|_| Failure::new("CODEHUB_TEST_S3_ENDPOINT is not a URL"))?;
        let bucket = rusty_s3::Bucket::new(
            endpoint,
            UrlStyle::Path,
            unique("loadtest", '-'),
            env("CODEHUB_TEST_S3_REGION")?,
        )
        .map_err(|error| Failure::new(format!("the S3 endpoint is not usable: {error}")))?;
        let credentials = Credentials::new(
            env("CODEHUB_TEST_S3_ACCESS_KEY")?,
            env("CODEHUB_TEST_S3_SECRET_KEY")?,
        );
        let this = Self {
            http: reqwest::Client::new(),
            bucket,
            credentials,
        };
        let url = this.bucket.create_bucket(&this.credentials).sign(BOUND);
        let answer = this.http.put(url).send().await?;
        if !answer.status().is_success() {
            return Err(Failure::new(format!(
                "the S3 service refused to create the bucket: {}",
                answer.status()
            )));
        }
        Ok(this)
    }

    /// The bucket's name.
    fn name(&self) -> &str {
        self.bucket.name()
    }

    /// Deletes the bucket with what it holds, by MinIO's force-delete header.
    ///
    /// # Errors
    ///
    /// Fails where the service refuses.
    pub async fn remove(self) -> Result<(), Failure> {
        let mut action = self.bucket.delete_bucket(&self.credentials);
        action.headers_mut().insert("x-minio-force-delete", "true");
        let url = action.sign(BOUND);
        let answer = self
            .http
            .delete(url)
            .header("x-minio-force-delete", "true")
            .send()
            .await?;
        if !answer.status().is_success() {
            return Err(Failure::new(format!(
                "the S3 service refused to delete the bucket {}: {}",
                self.name(),
                answer.status()
            )));
        }
        Ok(())
    }
}

/// A started `codehub` process, killed when dropped.
pub struct Replica {
    child: Child,
    /// The URL the replica answers on.
    pub base: String,
}

impl Replica {
    /// Starts `binary`, the built `codehub`, on `127.0.0.1:0` against `database` and `bucket`,
    /// with `settings` set after the check environment's, and waits for the line that names its
    /// address.
    ///
    /// # Errors
    ///
    /// Fails where a variable is not set, the process cannot start or its first line is not the
    /// one that names the address.
    pub async fn start(
        binary: &Path,
        database: &Database,
        bucket: &Bucket,
        settings: &[(&str, String)],
    ) -> Result<Self, Failure> {
        let mut child = Command::new(binary)
            .env("CODEHUB_LISTEN", "127.0.0.1:0")
            .env("CODEHUB_POSTGRES_URL", database.url()?)
            .env("CODEHUB_S3_ENDPOINT", env("CODEHUB_TEST_S3_ENDPOINT")?)
            .env("CODEHUB_S3_REGION", env("CODEHUB_TEST_S3_REGION")?)
            .env("CODEHUB_S3_BUCKET", bucket.name())
            .env("CODEHUB_S3_ACCESS_KEY", env("CODEHUB_TEST_S3_ACCESS_KEY")?)
            .env("CODEHUB_S3_SECRET_KEY", env("CODEHUB_TEST_S3_SECRET_KEY")?)
            .env("CODEHUB_OIDC_ISSUER", env("CODEHUB_TEST_OIDC_ISSUER")?)
            .envs(settings.iter().map(|(name, value)| (*name, value)))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let pipe = child
            .stderr
            .take()
            .ok_or_else(|| Failure::new("codehub has no stderr"))?;
        let mut lines = BufReader::new(pipe).lines();
        let line = tokio::time::timeout(BOUND, lines.next_line())
            .await
            .map_err(|_| Failure::new("codehub printed no line within the bound"))??
            .unwrap_or_default();
        let Some(address) = line.strip_prefix("codehub listening on ") else {
            return Err(Failure::new(format!("codehub did not start: {line}")));
        };
        // The pipe is read to its end so the child never blocks writing to it.
        tokio::spawn(async move { while let Ok(Some(_)) = lines.next_line().await {} });
        Ok(Self {
            child,
            base: format!("http://{address}"),
        })
    }

    /// How the process exited, where it is no longer running.
    ///
    /// # Errors
    ///
    /// Fails where the process cannot be inspected.
    pub fn exited(&mut self) -> Result<Option<ExitStatus>, Failure> {
        Ok(self.child.try_wait()?)
    }

    /// Kills the process and waits for it.
    pub async fn stop(mut self) {
        let _ = self.child.kill().await;
    }
}

/// An access token for `user` (`alice`, `bob`) from the password grant of the client `client`
/// (`TEST`) of the realm `CODEHUB_TEST_KEYCLOAK_ISSUER`, as `CODEHUB_TEST_KEYCLOAK_<client>_*`
/// name its variables.
///
/// # Errors
///
/// Fails where a variable is not set or Keycloak does not answer with an access token.
pub async fn token(client: &str, user: &str) -> Result<String, Failure> {
    let issuer = env("CODEHUB_TEST_KEYCLOAK_ISSUER")?;
    let password = env(&format!(
        "CODEHUB_TEST_KEYCLOAK_PASSWORD_{}",
        user.to_uppercase()
    ))?;
    let answer: serde_json::Value = reqwest::Client::new()
        .post(format!("{issuer}/protocol/openid-connect/token"))
        .basic_auth(
            env(&format!("CODEHUB_TEST_KEYCLOAK_{client}_CLIENT_ID"))?,
            Some(env(&format!(
                "CODEHUB_TEST_KEYCLOAK_{client}_CLIENT_SECRET"
            ))?),
        )
        .form(&[
            ("grant_type", "password"),
            ("username", user),
            ("password", &password),
        ])
        .send()
        .await?
        .json()
        .await?;
    answer
        .get("access_token")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| Failure::new(format!("Keycloak gave {user} no access token: {answer}")))
}
