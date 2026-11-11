//! The running `codehub` binary, the database it runs against, and an HTTP client for its API.
//!
//! Every helper panics when something is not as expected, as a failed assertion does. Every wait
//! is bounded.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use app::domain::address::Address;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use sqlx::{AssertSqlSafe, Connection, Executor, PgConnection};

/// The audience the server accepts, which Dex gives every access token its client asks for.
const AUDIENCE: &str = "codehub-test";

/// How long a start, a shutdown or a request may take.
const BOUND: Duration = Duration::from_secs(30);

/// The value of the environment variable `name`.
///
/// # Panics
///
/// Panics if it is not set.
pub fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} is not set; run through `make test`"))
}

/// A database created for one test and dropped with it.
pub struct TestDatabase {
    server: String,
    name: String,
}

/// `server` with its database replaced by `database`.
fn url(server: &str, database: &str) -> String {
    let (base, _) = server.rsplit_once('/').expect("the URL names a database");
    format!("{base}/{database}")
}

impl TestDatabase {
    /// Creates an empty database named `codehub_43_<unique>`.
    ///
    /// # Panics
    ///
    /// Panics if the server cannot be reached or the database cannot be created.
    pub async fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let name = format!(
            "codehub_43_{}_{}_{nanos}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let server = env("CODEHUB_TEST_POSTGRES_URL");
        let mut admin = PgConnection::connect(&server).await.unwrap();
        admin
            .execute(AssertSqlSafe(format!("create database {name}")))
            .await
            .unwrap();
        Self { server, name }
    }

    /// The URL of the database.
    pub fn url(&self) -> String {
        url(&self.server, &self.name)
    }
}

impl Drop for TestDatabase {
    fn drop(&mut self) {
        let (server, name) = (self.server.clone(), self.name.clone());
        // A thread of its own, since the test's runtime may be shutting down.
        let removed = std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async {
                    let mut admin = PgConnection::connect(&server).await.unwrap();
                    admin
                        .execute(AssertSqlSafe(format!(
                            "drop database if exists {name} with (force)"
                        )))
                        .await
                        .unwrap();
                });
        });
        let _ = removed.join();
    }
}

/// A started `codehub` process, killed when dropped.
pub struct Server {
    child: Child,
    /// The URL the server answers on.
    pub base: String,
    _stderr: Receiver<String>,
}

impl Server {
    /// Starts `codehub` on `127.0.0.1:0` against `database`, configured by environment variables,
    /// and waits for the line that names its address.
    ///
    /// # Panics
    ///
    /// Panics if the process does not print that line within the bound.
    pub fn start(database: &TestDatabase) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_codehub"))
            .env("CODEHUB_LISTEN", "127.0.0.1:0")
            .env("CODEHUB_POSTGRES_URL", database.url())
            .env("CODEHUB_S3_ENDPOINT", env("CODEHUB_TEST_S3_ENDPOINT"))
            .env("CODEHUB_S3_REGION", env("CODEHUB_TEST_S3_REGION"))
            .env("CODEHUB_S3_BUCKET", env("CODEHUB_TEST_S3_BUCKET"))
            .env("CODEHUB_S3_ACCESS_KEY", env("CODEHUB_TEST_S3_ACCESS_KEY"))
            .env("CODEHUB_S3_SECRET_KEY", env("CODEHUB_TEST_S3_SECRET_KEY"))
            .env("CODEHUB_OIDC_ISSUER", env("CODEHUB_TEST_OIDC_ISSUER"))
            .env("CODEHUB_OIDC_AUDIENCE", AUDIENCE)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let (sender, stderr) = channel();
        let pipe = child.stderr.take().unwrap();
        // The pipe is read to its end so the child never blocks writing to it.
        std::thread::spawn(move || {
            for line in BufReader::new(pipe).lines().map_while(Result::ok) {
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        let mut server = Self {
            child,
            base: String::new(),
            _stderr: stderr,
        };
        let line = server
            ._stderr
            .recv_timeout(BOUND)
            .expect("codehub printed a line to stderr within the bound");
        let address = line
            .strip_prefix("codehub listening on ")
            .unwrap_or_else(|| panic!("codehub did not start: {line}"));
        server.base = format!("http://{address}");
        server
    }

    /// Sends the signal `name` (`TERM`, `INT`) to the process and returns how it exited.
    ///
    /// # Panics
    ///
    /// Panics if the signal cannot be sent or the process is still running after the bound.
    pub fn signal(&mut self, name: &str) -> ExitStatus {
        let sent = Command::new("kill")
            .args([format!("-{name}"), self.child.id().to_string()])
            .status()
            .unwrap();
        assert!(sent.success(), "kill -{name} failed");
        let deadline = Instant::now() + BOUND;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            assert!(Instant::now() < deadline, "codehub is still running");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if self.child.try_wait().unwrap().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// An access token for `user` (`alice`, `bob`, `carol`) from Dex's password grant.
///
/// # Panics
///
/// Panics if Dex refuses the request.
pub async fn token(user: &str) -> String {
    let issuer = env("CODEHUB_TEST_OIDC_ISSUER");
    let password = env(&format!(
        "CODEHUB_TEST_OIDC_PASSWORD_{}",
        user.to_uppercase()
    ));
    let body = format!(
        "grant_type=password&username={user}%40example.test&password={password}&scope=openid"
    );
    let response = reqwest::Client::new()
        .post(format!("{issuer}/token"))
        .basic_auth(
            env("CODEHUB_TEST_OIDC_TEST_CLIENT_ID"),
            Some(env("CODEHUB_TEST_OIDC_TEST_CLIENT_SECRET")),
        )
        .header("content-type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .unwrap();
    assert!(
        response.status().is_success(),
        "Dex refused {user}'s token request"
    );
    let answer: Value = response.json().await.unwrap();
    answer["access_token"].as_str().unwrap().to_owned()
}

/// A client of the server's API, presenting one bearer token or none.
pub struct Client {
    http: reqwest::Client,
    base: String,
    token: Option<String>,
}

impl Client {
    /// A client of the server at `base` presenting `token`.
    ///
    /// # Panics
    ///
    /// Panics if the HTTP client cannot be built.
    pub fn new(base: &str, token: Option<String>) -> Self {
        Self {
            http: reqwest::Client::builder().timeout(BOUND).build().unwrap(),
            base: base.to_owned(),
            token,
        }
    }

    /// Sends `method` `path` with the JSON `body`, and returns the status and the JSON answer
    /// (`null` where the body is empty).
    ///
    /// # Panics
    ///
    /// Panics if the request fails or the answer is not JSON.
    pub async fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut request = self.http.request(method, format!("{}{path}", self.base));
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        let status = response.status();
        let bytes = response.bytes().await.unwrap();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap()
        };
        (status, body)
    }

    /// Creates the organisation `org` holding the repository `repo`.
    ///
    /// # Panics
    ///
    /// Panics if either is refused.
    pub async fn create_repository(&self, org: &str, repo: &str) {
        let (status, _) = self
            .send(Method::POST, "/v1/orgs", Some(json!({"name": org})))
            .await;
        assert_eq!(status, StatusCode::CREATED);
        let (status, _) = self
            .send(
                Method::POST,
                &format!("/v1/orgs/{org}/repos"),
                Some(json!({"name": repo})),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED);
    }

    /// Uploads `content` through an upload grant, stages it at `path` in the draft `draft` and
    /// lands the draft, in the repository `org`/`repo`.
    ///
    /// # Panics
    ///
    /// Panics if a step is refused.
    pub async fn land_file(&self, org: &str, repo: &str, draft: &str, path: &str, content: &str) {
        let address = Address::of(content.as_bytes()).to_string();
        let (status, grant) = self
            .send(
                Method::POST,
                &format!("/v1/orgs/{org}/repos/{repo}/uploads"),
                Some(json!({"address": address, "size": content.len()})),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        let mut upload = self
            .http
            .put(grant["url"].as_str().unwrap())
            .body(content.to_owned());
        for header in grant["headers"].as_array().unwrap() {
            upload = upload.header(
                header["name"].as_str().unwrap(),
                header["value"].as_str().unwrap(),
            );
        }
        assert!(upload.send().await.unwrap().status().is_success());
        let (status, _) = self
            .send(
                Method::PUT,
                &format!("/v1/orgs/{org}/repos/{repo}/drafts/{draft}/files/{path}"),
                Some(json!({"kind": "regular", "content": address})),
            )
            .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (status, _) = self
            .send(
                Method::POST,
                &format!("/v1/orgs/{org}/repos/{repo}/drafts/{draft}/land"),
                Some(json!({"message": "add a file"})),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
    }

    /// The content of the file at `path` in the repository `org`/`repo`, read through a download
    /// grant; `None` where the server answers 404.
    ///
    /// # Panics
    ///
    /// Panics if the server answers anything else, or the download fails.
    pub async fn read_file(&self, org: &str, repo: &str, path: &str) -> Option<String> {
        let (status, file) = self
            .send(
                Method::GET,
                &format!("/v1/orgs/{org}/repos/{repo}/files/{path}"),
                None,
            )
            .await;
        if status == StatusCode::NOT_FOUND {
            return None;
        }
        assert_eq!(status, StatusCode::OK);
        let download = self
            .http
            .get(file["download"]["url"].as_str().unwrap())
            .send()
            .await
            .unwrap();
        assert!(download.status().is_success());
        Some(download.text().await.unwrap())
    }
}
