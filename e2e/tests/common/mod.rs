//! The built `codehub` and `ch` binaries, the database `codehub` runs against, and the approval of
//! a device code at Dex.
//!
//! Every helper panics when something is not as expected, as a failed assertion does. Every wait
//! is bounded.

use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::time::Duration;

use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};

pub use test_support::{ChUser, Run, TestDatabase, env, unique};

/// The audience `codehub` accepts: the client `ch`, to which Dex issues the device grant's token.
const AUDIENCE: &str = "ch";

/// How long a start, a shutdown, a login or one run of `ch` may take.
const BOUND: Duration = Duration::from_secs(60);

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

/// A started `codehub` process, killed when dropped.
pub struct Server(test_support::Server);

impl Server {
    /// Starts the built `codehub` against `database` and Dex, and waits for the line that names
    /// its address.
    ///
    /// # Panics
    ///
    /// Panics if the process does not print that line within the bound.
    pub fn start(database: &TestDatabase) -> Self {
        Self(test_support::Server::start(
            &binary("codehub"),
            database,
            AUDIENCE,
        ))
    }

    /// Sends SIGTERM to the process and returns how it exited.
    ///
    /// # Panics
    ///
    /// Panics if the signal cannot be sent or the process is still running after the bound.
    pub fn terminate(&mut self) -> ExitStatus {
        self.0.signal("TERM")
    }
}

impl Deref for Server {
    type Target = test_support::Server;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Server {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
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
    ch: ChUser,
}

impl Deref for User {
    type Target = ChUser;

    fn deref(&self) -> &Self::Target {
        &self.ch
    }
}

impl User {
    /// A user whose Dex account is `<name>@example.test`, with no state yet.
    ///
    /// # Panics
    ///
    /// Panics if the directory cannot be created.
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            ch: ChUser::new(
                &binary("ch"),
                Path::new(env!("CARGO_TARGET_TMPDIR")),
                &format!("e2e-{name}"),
            ),
        }
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
        let file = self.home().join(unique("upload"));
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
