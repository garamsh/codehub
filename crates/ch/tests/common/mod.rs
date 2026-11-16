//! The HTTP API served in process over the memory adapters and the S3 object store, and users that
//! run the built `ch` against it with a state file of their own.
//!
//! Every helper panics when something is not as expected, as a failed assertion does.

use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;

use adapter_http::{ApiState, Pause, RunnerSettings};
use adapter_oidc::OidcIdentityVerification;
use adapter_s3::S3ObjectStore;
use app::domain::address::Address;
use app::domain::check::{CheckDefinition, Command, Timeout};
use app::domain::name::{OrganisationName, RepositoryName};
use app::domain::principal::Principal;
use app::domain::revert::Revert;
use app::port::driven::changeset_log::ChangesetLog;
use app::port::driven::identity_verification::IdentityVerification;
use app::port::driven::job_queue::JobQueue;
use app::port::driven::repository_directory::RepositoryDirectory;
use serde_json::json;
use test_support::contract::Deployment;
use test_support::{MemoryDeployment, Taught};

pub use test_support::{ChUser, Run, env, unique};

/// The largest upload the server grants.
const MAX_UPLOAD_SIZE: u64 = 1024 * 1024;

/// How long the server holds a claim that finds no job.
const HOLD: Duration = Duration::from_millis(300);

/// The adapters of one server, but for its object store, which is the S3 one.
type Stores = MemoryDeployment<Taught>;

/// A user of `ch`: a configuration directory of its own, and so a state file of its own.
pub struct User(ChUser);

impl Deref for User {
    type Target = ChUser;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl User {
    /// A user who has not logged in.
    ///
    /// # Panics
    ///
    /// Panics when the directory cannot be created.
    #[must_use]
    pub fn logged_out() -> Self {
        Self(ChUser::new(
            Path::new(env!("CARGO_BIN_EXE_ch")),
            Path::new(env!("CARGO_TARGET_TMPDIR")),
            "ch-config",
        ))
    }
}

/// A server over fresh adapters.
pub struct Server {
    url: String,
    stores: Arc<Stores>,
}

/// Serves the API on a port of `127.0.0.1` for as long as the test runs.
///
/// # Panics
///
/// Panics when the S3 variables are not set, or the socket cannot be bound.
pub async fn serve() -> Server {
    let stores = Arc::new(Stores::default());
    let identity = stores.identity.clone();
    bind(stores, identity).await
}

/// Serves the API as [`serve`] does, but verifying credentials with the real OIDC adapter against
/// the test Keycloak, for the audience `codehub`.
///
/// # Panics
///
/// Panics when the Keycloak or S3 variables are not set, or the socket cannot be bound.
pub async fn serve_oidc() -> Server {
    let oidc = Arc::new(
        OidcIdentityVerification::new(
            &env("CODEHUB_TEST_KEYCLOAK_ISSUER"),
            "codehub",
            Duration::from_secs(1),
        )
        .unwrap(),
    );
    bind(Arc::default(), oidc).await
}

fn objects() -> S3ObjectStore {
    S3ObjectStore::new(
        &env("CODEHUB_TEST_S3_ENDPOINT"),
        &env("CODEHUB_TEST_S3_REGION"),
        &env("CODEHUB_TEST_S3_BUCKET"),
        &env("CODEHUB_TEST_S3_ACCESS_KEY"),
        &env("CODEHUB_TEST_S3_SECRET_KEY"),
        MAX_UPLOAD_SIZE,
    )
    .unwrap()
}

async fn bind(stores: Arc<Stores>, identity: Arc<dyn IdentityVerification>) -> Server {
    let mut assembly = stores.assembly();
    assembly.objects = Arc::new(objects());
    assembly.identity = identity;
    assembly.max_upload_size = MAX_UPLOAD_SIZE;
    let router = adapter_http::router(ApiState {
        authentication: Arc::new(assembly.authentication()),
        repositories: Arc::new(assembly.repositories()),
        members: Arc::new(assembly.members()),
        source_control: Arc::new(assembly.source_control()),
        runners: Arc::new(assembly.runners()),
        jobs: Arc::new(assembly.jobs()),
        checks: Arc::new(assembly.checks()),
        runs: Arc::new(assembly.runs()),
        registry_token: None,
        settings: RunnerSettings {
            hold: HOLD,
            retry_interval: Duration::from_millis(50),
            heartbeat_interval: Duration::from_secs(10),
            pause,
        },
        readiness: Arc::new(()),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    Server { url, stores }
}

fn pause(duration: Duration) -> Pause {
    Box::pin(tokio::time::sleep(duration))
}

impl Server {
    /// The issuer the server's users have.
    pub const ISSUER: &'static str = "https://id.test";

    /// The server's base URL.
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    /// A user named `name` whose credential the server accepts, logged in through a state file
    /// that holds the server's URL and that credential as the token.
    ///
    /// # Panics
    ///
    /// Panics when `name` is not a valid subject or the state file cannot be written.
    #[must_use]
    pub fn user(&self, name: &str) -> User {
        self.user_with_token(&self.token(name))
    }

    /// The credential the server accepts for the principal `name`: a JWT in shape, so that `ch`
    /// can read the issuer from it.
    ///
    /// # Panics
    ///
    /// Panics when `name` is not a valid subject.
    #[must_use]
    pub fn token(&self, name: &str) -> String {
        // A JWT in shape, so that `ch` can read the issuer from it; only the server's verification
        // means anything, and here that is the registration below.
        let claims = serde_json::json!({"iss": Self::ISSUER, "sub": name}).to_string();
        let token = format!(
            "e30.{}.sig",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(claims)
        );
        self.stores.identity.register(
            &token.parse().unwrap(),
            Principal::new(Self::ISSUER, name).unwrap(),
        );
        token
    }

    /// A user whose state file holds `token`, which the server may not accept.
    ///
    /// # Panics
    ///
    /// Panics when the state file cannot be written.
    #[must_use]
    pub fn user_with_token(&self, token: &str) -> User {
        let user = User::logged_out();
        let state = serde_json::json!({"server": self.url, "token": token});
        std::fs::create_dir_all(user.state_file().parent().unwrap()).unwrap();
        std::fs::write(user.state_file(), state.to_string()).unwrap();
        user
    }
}

/// Creates a repository `<org>/app` for `user` under a fresh organisation, selects it and returns
/// the organisation's name.
pub async fn repository(user: &User) -> String {
    let org = unique("org");
    user.ok(&["org", "create", &org]).await;
    user.ok(&["repo", "create", &format!("{org}/app")]).await;
    user.ok(&["use", &format!("{org}/app")]).await;
    org
}

/// Stages `content` at `path` in `draft`.
pub async fn put(user: &User, draft: &str, path: &str, content: &str) {
    user.ok(&["put", path, &file(content.as_bytes()), "--draft", draft])
        .await;
}

/// Writes `bytes` to a file of its own and returns the path.
///
/// # Panics
///
/// Panics when the file cannot be written.
#[must_use]
pub fn file(bytes: &[u8]) -> String {
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(unique("ch-file"));
    std::fs::write(&path, bytes).unwrap();
    path.to_str().unwrap().to_owned()
}

impl Server {
    /// The text of a check file that defines `checks`, each a name and whether it blocks, with no
    /// labels, and that the server's definitions read as those checks. A landing whose tree holds
    /// it at `.codehub/checks.toml` queues them.
    ///
    /// # Panics
    ///
    /// Panics when a name is not a valid check name.
    #[must_use]
    pub fn checks_file(&self, checks: &[(&str, bool)]) -> String {
        let text: String = checks
            .iter()
            .map(|(name, blocking)| format!("{name} {blocking}\n"))
            .collect();
        let definitions = checks
            .iter()
            .map(|(name, blocking)| {
                CheckDefinition::new(
                    name.parse().unwrap(),
                    Command::new("true").unwrap(),
                    std::collections::BTreeSet::new(),
                    *blocking,
                    Timeout::new(60).unwrap(),
                )
            })
            .collect();
        self.stores.definitions.teach(&text, Ok(definitions));
        text
    }

    /// From now on reads the check file whose text is `text` as `definitions`, which the memory
    /// definitions do not parse from a format of their own.
    pub fn teach_checks(&self, text: &str, definitions: Vec<CheckDefinition>) {
        self.stores.definitions.teach(text, Ok(definitions));
    }

    /// The message of the changeset `id` of `<org>/app`, as the server's log holds it.
    ///
    /// # Panics
    ///
    /// Panics when the repository or the changeset is not there.
    pub async fn changeset_message(&self, org: &str, id: &str) -> String {
        let organisation: OrganisationName = org.parse().unwrap();
        let repository: RepositoryName = "app".parse().unwrap();
        let repository = self
            .stores
            .directory
            .find_repository(&organisation, &repository)
            .await
            .unwrap();
        self.stores
            .log
            .get(repository, id.parse().unwrap())
            .await
            .unwrap()
            .message
    }

    /// Records the revert of the first job of `changeset` as failed with `reason`, in place of
    /// what was recorded, which stands in for a revert whose landing failed.
    ///
    /// # Panics
    ///
    /// Panics when the changeset has no job or the queue refuses the record.
    pub async fn record_failed_revert(&self, org: &str, changeset: &str, reason: &str) {
        let organisation: OrganisationName = org.parse().unwrap();
        let repository: RepositoryName = "app".parse().unwrap();
        let id = self
            .stores
            .directory
            .find_repository(&organisation, &repository)
            .await
            .unwrap();
        let jobs = self
            .stores
            .queue
            .of_changeset(id, changeset.parse().unwrap())
            .await
            .unwrap();
        let revert = Revert::Failed {
            reason: reason.to_owned(),
        };
        self.stores
            .queue
            .record_revert(jobs[0].id, revert)
            .await
            .unwrap();
    }

    /// Claims the oldest queued job as the runner holding `token` and ends it with `status`, one of
    /// `passed`, `failed`, `timed_out` and `cancelled`, and `exit_code`.
    ///
    /// # Panics
    ///
    /// Panics when there is no job to claim or the server refuses the result.
    pub async fn finish_job(&self, token: &str, status: &str, exit_code: Option<i32>) {
        let http = reqwest::Client::new();
        let claimed = http
            .post(format!("{}/v1/runner/jobs/claim", self.url))
            .bearer_auth(token)
            .send()
            .await
            .unwrap();
        assert_eq!(claimed.status(), 200, "no job to claim");
        let job: serde_json::Value = claimed.json().await.unwrap();
        let id = job["id"].as_u64().unwrap();
        let mut result = json!({"status": status, "output_tail": ""});
        if let Some(code) = exit_code {
            result["exit_code"] = json!(code);
        }
        let reported = http
            .post(format!("{}/v1/runner/jobs/{id}/result", self.url))
            .bearer_auth(token)
            .json(&result)
            .send()
            .await
            .unwrap();
        assert_eq!(reported.status(), 204);
    }
}

/// A change a runner reports a run made.
pub enum Change {
    /// A file the run left with `content`.
    File {
        /// The path in the repository.
        path: String,
        /// The file's bytes.
        content: Vec<u8>,
        /// Whether it is executable.
        executable: bool,
    },
    /// A file the run removed.
    Deleted(String),
}

/// A file a runner reports in a run's output tree.
pub struct Output {
    /// The path in the repository.
    pub path: String,
    /// The file's bytes.
    pub content: Vec<u8>,
    /// Whether it is executable.
    pub executable: bool,
}

/// What a runner reports of a run it held.
pub struct Report {
    /// How the command ended: `exited`, `timed_out`, `cancelled` or `executor_failed`.
    pub outcome: &'static str,
    /// The command's exit code, where it exited.
    pub exit_code: Option<i32>,
    /// What the command wrote to stdout.
    pub stdout: Option<String>,
    /// What the command wrote to stderr.
    pub stderr: Option<String>,
    /// What the command changed.
    pub changes: Vec<Change>,
    /// The files the command left under the run's outputs.
    pub outputs: Vec<Output>,
}

impl Report {
    /// A command that exited with `code` and changed nothing.
    #[must_use]
    pub fn exited(code: i32) -> Self {
        Self {
            outcome: "exited",
            exit_code: Some(code),
            stdout: None,
            stderr: None,
            changes: Vec::new(),
            outputs: Vec::new(),
        }
    }

    /// A run that ended with `outcome` and no exit code.
    #[must_use]
    pub fn stopped(outcome: &'static str) -> Self {
        Self {
            outcome,
            exit_code: None,
            ..Self::exited(0)
        }
    }
}

/// A run's job as a runner holds it.
pub struct Claimed {
    /// The job's, and the run's, identifier.
    pub id: u64,
    runner: String,
}

impl Server {
    /// Registers `name` as a runner of `admin`'s organisation and returns its credential.
    ///
    /// # Panics
    ///
    /// Panics when the registration fails.
    pub async fn register_runner(&self, admin: &User, name: &str) -> String {
        admin
            .ok(&["runner", "register", name, "--issuer", Self::ISSUER])
            .await;
        self.token(name)
    }

    /// Claims the next job as the host runner holding `runner`, waiting for one to be queued.
    ///
    /// # Panics
    ///
    /// Panics when no job is offered within a few seconds.
    pub async fn claim_run(&self, runner: &str) -> Claimed {
        let http = reqwest::Client::new();
        for _ in 0..100 {
            let claimed = http
                .post(format!("{}/v1/runner/jobs/claim", self.url))
                .bearer_auth(runner)
                .json(&json!({"executor": "host"}))
                .send()
                .await
                .unwrap();
            if claimed.status() == 200 {
                let job: serde_json::Value = claimed.json().await.unwrap();
                return Claimed {
                    id: job["id"].as_u64().unwrap(),
                    runner: runner.to_owned(),
                };
            }
        }
        panic!("no run was queued");
    }

    /// Stores `bytes` in `org/app` as `owner` and returns their address.
    ///
    /// # Panics
    ///
    /// Panics when the server refuses the upload or the object store the bytes.
    pub async fn store(&self, owner: &str, org: &str, bytes: &[u8]) -> String {
        let address = Address::of(bytes).to_string();
        let http = reqwest::Client::new();
        let grant: serde_json::Value = http
            .post(format!("{}/v1/orgs/{org}/repos/app/uploads", self.url))
            .bearer_auth(owner)
            .json(&json!({"address": address, "size": bytes.len()}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let mut put = http.put(grant["url"].as_str().unwrap());
        for header in grant["headers"].as_array().unwrap() {
            put = put.header(
                header["name"].as_str().unwrap(),
                header["value"].as_str().unwrap(),
            );
        }
        let stored = put.body(bytes.to_vec()).send().await.unwrap();
        assert!(stored.status().is_success(), "{}", stored.status());
        address
    }

    /// Reports `report` of the run `claimed` to the server, with its objects stored by `owner` in
    /// `org/app`.
    ///
    /// # Panics
    ///
    /// Panics when the server refuses the report.
    pub async fn report_run(&self, claimed: &Claimed, owner: &str, org: &str, report: &Report) {
        let stream = |address: Option<String>| address.map(|a| json!({"address": a, "cut": false}));
        let stdout = match &report.stdout {
            Some(text) => Some(self.store(owner, org, text.as_bytes()).await),
            None => None,
        };
        let stderr = match &report.stderr {
            Some(text) => Some(self.store(owner, org, text.as_bytes()).await),
            None => None,
        };
        let mut changes = Vec::new();
        for change in &report.changes {
            changes.push(match change {
                Change::File {
                    path,
                    content,
                    executable,
                } => json!({
                    "path": path,
                    "kind": if *executable { "executable" } else { "regular" },
                    "address": self.store(owner, org, content).await,
                }),
                Change::Deleted(path) => {
                    json!({"path": path, "kind": "deleted", "address": null})
                }
            });
        }
        let mut outputs = Vec::new();
        for output in &report.outputs {
            outputs.push(json!({
                "path": output.path,
                "address": self.store(owner, org, &output.content).await,
                "executable": output.executable,
            }));
        }
        let body = json!({
            "outcome": report.outcome,
            "exit_code": report.exit_code,
            "stdout": stream(stdout),
            "stderr": stream(stderr),
            "objects": {"downloaded": 0, "from_cache": 0},
            "outputs": outputs,
            "caches": [],
            "changes": changes,
            "refused": [],
        });
        let reported = reqwest::Client::new()
            .post(format!("{}/v1/runner/jobs/{}/result", self.url, claimed.id))
            .bearer_auth(&claimed.runner)
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(reported.status(), 204, "{}", reported.text().await.unwrap());
    }

    /// The version of `token`'s workspace `draft` of `org/app`, which grows each time its changes
    /// are recorded.
    ///
    /// # Panics
    ///
    /// Panics when the workspace cannot be read.
    pub async fn workspace_version(&self, token: &str, org: &str, draft: &str) -> u64 {
        let workspace: serde_json::Value = reqwest::Client::new()
            .get(format!(
                "{}/v1/orgs/{org}/repos/app/drafts/{draft}",
                self.url
            ))
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        workspace["version"].as_u64().unwrap()
    }

    /// The workspace `draft` of `org/app` as the server answers it, as JSON text.
    ///
    /// # Panics
    ///
    /// Panics when the workspace cannot be read.
    pub async fn workspace_json(&self, token: &str, org: &str, draft: &str) -> String {
        reqwest::Client::new()
            .get(format!(
                "{}/v1/orgs/{org}/repos/app/drafts/{draft}",
                self.url
            ))
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap()
    }
}
