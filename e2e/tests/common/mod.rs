//! The built `codehub` and `ch` binaries, the database `codehub` runs against, and the approval of
//! a device code at Dex.
//!
//! Every helper panics when something is not as expected, as a failed assertion does. Every wait
//! is bounded.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use sqlx::{AssertSqlSafe, Connection, Executor, PgConnection};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};

/// The audience `codehub` accepts: the client `ch`, to which Dex issues the device grant's token.
const AUDIENCE: &str = "ch";

/// How long a start, a shutdown, a login or one run of `ch` may take.
const BOUND: Duration = Duration::from_secs(60);

/// The value of the environment variable `name`.
///
/// # Panics
///
/// Panics if it is not set.
pub fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} is not set; run through `make e2e`"))
}

/// A name no other test of any run uses: `prefix`, the process and a counter.
pub fn unique(prefix: &str) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    format!(
        "{prefix}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

/// The path of the built binary `name`, which sits beside the directory of the running test.
///
/// # Panics
///
/// Panics, naming the build command, if the binary is not there.
pub fn binary(name: &str) -> PathBuf {
    let test = std::env::current_exe().unwrap();
    let path = test.parent().and_then(Path::parent).unwrap().join(name);
    assert!(
        path.is_file(),
        "{} is missing; build it with `cargo build --locked -p codehub -p ch`",
        path.display()
    );
    path
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
    /// Creates an empty database named `codehub_45_<unique>`.
    ///
    /// # Panics
    ///
    /// Panics if the server cannot be reached or the database cannot be created.
    pub async fn new() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let name = format!("{}_{nanos}", unique("codehub_45").replace('-', "_"));
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
}

impl Server {
    /// Starts `codehub` on `127.0.0.1:0` against `database` and Dex, and waits for the line that
    /// names its address.
    ///
    /// # Panics
    ///
    /// Panics if the process does not print that line within the bound.
    pub fn start(database: &TestDatabase) -> Self {
        let mut child = Command::new(binary("codehub"))
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
        let pipe = child.stderr.take().unwrap();
        let (sender, lines) = std::sync::mpsc::channel();
        // The pipe is read to its end so the child never blocks writing to it.
        std::thread::spawn(move || {
            use std::io::BufRead;
            for line in std::io::BufReader::new(pipe).lines().map_while(Result::ok) {
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        let mut server = Self {
            child,
            base: String::new(),
        };
        let line = lines
            .recv_timeout(BOUND)
            .expect("codehub printed a line to stderr within the bound");
        let address = line
            .strip_prefix("codehub listening on ")
            .unwrap_or_else(|| panic!("codehub did not start: {line}"));
        server.base = format!("http://{address}");
        server
    }

    /// Sends SIGTERM to the process and returns how it exited.
    ///
    /// # Panics
    ///
    /// Panics if the signal cannot be sent or the process is still running after the bound.
    pub fn terminate(&mut self) -> ExitStatus {
        let sent = Command::new("kill")
            .args(["-TERM", &self.child.id().to_string()])
            .status()
            .unwrap();
        assert!(sent.success(), "kill -TERM failed");
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

/// What one run of `ch` did.
pub struct Run {
    /// The exit code.
    pub code: Option<i32>,
    /// What it wrote to stdout.
    pub stdout: String,
    /// What it wrote to stderr.
    pub stderr: String,
}

/// The issuer and subject `ch login --json` prints.
#[derive(Deserialize)]
pub struct Principal {
    /// The issuer URL.
    pub issuer: String,
    /// The subject at that issuer, which Dex makes opaque.
    pub subject: String,
}

/// One user of `ch`: a configuration directory of its own, and the Dex account it logs in as.
pub struct User {
    name: String,
    home: PathBuf,
}

impl User {
    /// A user whose Dex account is `<name>@example.test`, with no state yet.
    ///
    /// # Panics
    ///
    /// Panics if the directory cannot be created.
    pub fn new(name: &str) -> Self {
        let home = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(unique(&format!("e2e-{name}")));
        std::fs::create_dir_all(&home).unwrap();
        Self {
            name: name.to_owned(),
            home,
        }
    }

    /// The file `ch` keeps its state in.
    pub fn state_file(&self) -> PathBuf {
        self.home.join("ch").join("state.json")
    }

    fn command(&self, arguments: &[&str]) -> tokio::process::Command {
        let mut command = tokio::process::Command::new(binary("ch"));
        command
            .args(arguments)
            .env("XDG_CONFIG_HOME", &self.home)
            .env_remove("HOME")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        command
    }

    /// Runs `ch` with `arguments` and waits for it.
    ///
    /// # Panics
    ///
    /// Panics if it cannot be started or does not finish within the bound.
    pub async fn ch(&self, arguments: &[&str]) -> Run {
        let child = self.command(arguments).spawn().unwrap();
        let output = tokio::time::timeout(BOUND, child.wait_with_output())
            .await
            .unwrap_or_else(|_| panic!("ch {arguments:?} did not finish within the bound"))
            .unwrap();
        Run {
            code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }

    /// Runs `ch` with `arguments`, expects exit 0 and returns its stdout.
    ///
    /// # Panics
    ///
    /// Panics if `ch` exits with anything else.
    pub async fn ok(&self, arguments: &[&str]) -> String {
        let run = self.ch(arguments).await;
        assert_eq!(run.code, Some(0), "ch {arguments:?}: {}", run.stderr);
        run.stdout
    }

    /// Creates the organisation `org` and its repository `repo`, and selects the repository.
    ///
    /// # Panics
    ///
    /// Panics if a step fails.
    pub async fn create_repository(&self, org: &str, repo: &str) {
        self.ok(&["org", "create", org]).await;
        self.ok(&["repo", "create", &format!("{org}/{repo}")]).await;
        self.ok(&["use", &format!("{org}/{repo}")]).await;
    }

    /// Stages `content` at `path` in the draft `draft`, and returns the run.
    ///
    /// # Panics
    ///
    /// Panics if the file cannot be written.
    pub async fn put(&self, draft: &str, path: &str, content: &str) -> Run {
        let file = self.home.join(unique("upload"));
        std::fs::write(&file, content).unwrap();
        self.ch(&["put", path, file.to_str().unwrap(), "--draft", draft])
            .await
    }

    /// Stages `content` at `path` in the draft `draft` and expects it to succeed.
    ///
    /// # Panics
    ///
    /// Panics if `ch put` exits with anything but 0.
    pub async fn stage(&self, draft: &str, path: &str, content: &str) {
        let run = self.put(draft, path, content).await;
        assert_eq!(run.code, Some(0), "ch put {path}: {}", run.stderr);
    }

    /// Logs in at `server` with the device grant: reads the code `ch login` prints, approves it
    /// at Dex with this user's e-mail and password, and waits for `ch` to exit 0.
    ///
    /// # Panics
    ///
    /// Panics if the prompt, the approval or the exit is not as expected.
    pub async fn login(&self, server: &Server) -> Principal {
        let issuer = env("CODEHUB_TEST_OIDC_ISSUER");
        let mut child = self
            .command(&["login", &server.base, "--issuer", &issuer, "--json"])
            .spawn()
            .unwrap();
        let mut stderr = BufReader::new(child.stderr.take().unwrap());
        let mut stdout = child.stdout.take().unwrap();
        let flow = async {
            let mut prompt = String::new();
            stderr.read_line(&mut prompt).await.unwrap();
            self.approve(&prompt).await;
            let mut out = String::new();
            let mut err = String::new();
            stdout.read_to_string(&mut out).await.unwrap();
            stderr.read_to_string(&mut err).await.unwrap();
            let status = child.wait().await.unwrap();
            (status, out, err)
        };
        let (status, out, err) = tokio::time::timeout(BOUND, flow)
            .await
            .unwrap_or_else(|_| panic!("{}'s login did not finish within the bound", self.name));
        assert!(status.success(), "ch login: {err}");
        serde_json::from_str(&out).unwrap_or_else(|_| panic!("ch login printed {out}"))
    }

    /// Approves the device code in `prompt` (`To log in, open <uri> and enter the code <code>`)
    /// by submitting Dex's code form and then its local login form.
    async fn approve(&self, prompt: &str) {
        let (uri, code) = prompt
            .trim()
            .strip_prefix("To log in, open ")
            .and_then(|rest| rest.split_once(" and enter the code "))
            .unwrap_or_else(|| panic!("not a device-code prompt: {prompt}"));
        let password = env(&format!(
            "CODEHUB_TEST_OIDC_PASSWORD_{}",
            self.name.to_uppercase()
        ));
        let http = reqwest::Client::builder()
            .cookie_store(true)
            .timeout(BOUND)
            .build()
            .unwrap();
        let login = http
            .post(format!("{uri}/auth/verify_code"))
            .form(&[("user_code", code)])
            .send()
            .await
            .unwrap();
        assert!(login.status().is_success(), "Dex refused the code {code}");
        let page = login.url().clone();
        let approved = http
            .post(page)
            .form(&[
                ("login", format!("{}@example.test", self.name)),
                ("password", password),
            ])
            .send()
            .await
            .unwrap();
        assert!(
            approved.status().is_success(),
            "Dex refused {}'s login",
            self.name
        );
    }
}

impl Drop for User {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.home);
    }
}
