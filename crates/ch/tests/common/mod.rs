//! The HTTP API served in process over the memory adapters and the S3 object store, and users that
//! run the built `ch` against it with a state file of their own.
//!
//! Every helper panics when something is not as expected, as a failed assertion does.

use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine;

use adapter_http::{Pause, RunnerSettings};
use adapter_oidc::OidcIdentityVerification;
use adapter_s3::S3ObjectStore;
use app::domain::check::{CheckDefinition, Command, Timeout};
use app::domain::name::{OrganisationName, RepositoryName};
use app::domain::principal::Principal;
use app::domain::revert::Revert;
use app::port::driven::changeset_log::ChangesetLog;
use app::port::driven::identity_verification::IdentityVerification;
use app::port::driven::job_queue::JobQueue;
use app::port::driven::repository_directory::RepositoryDirectory;
use app::use_case::administration::Administration;
use serde_json::json;

pub use test_support::{ChUser, Run, env, unique};

/// The largest upload the server grants.
const MAX_UPLOAD_SIZE: u64 = 1024 * 1024;

/// How long the server holds a claim that finds no job.
const HOLD: Duration = Duration::from_millis(300);

/// The adapters of one server.
type Stores = test_support::Stores<S3ObjectStore>;

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
    stores: &'static Stores,
}

/// Serves the API on a port of `127.0.0.1` for as long as the test runs.
///
/// # Panics
///
/// Panics when the S3 variables are not set, or the socket cannot be bound.
pub async fn serve() -> Server {
    let stores = leak_stores();
    bind(stores, &stores.identity).await
}

/// Serves the API as [`serve`] does, but verifying credentials with the real OIDC adapter against
/// the test Keycloak, for the audience `codehub`.
///
/// # Panics
///
/// Panics when the Keycloak or S3 variables are not set, or the socket cannot be bound.
pub async fn serve_oidc() -> Server {
    let oidc: &'static OidcIdentityVerification = Box::leak(Box::new(
        OidcIdentityVerification::new(
            &env("CODEHUB_TEST_KEYCLOAK_ISSUER"),
            "codehub",
            Duration::from_secs(1),
        )
        .unwrap(),
    ));
    bind(leak_stores(), oidc).await
}

fn leak_stores() -> &'static Stores {
    Stores::leaked(
        S3ObjectStore::new(
            &env("CODEHUB_TEST_S3_ENDPOINT"),
            &env("CODEHUB_TEST_S3_REGION"),
            &env("CODEHUB_TEST_S3_BUCKET"),
            &env("CODEHUB_TEST_S3_ACCESS_KEY"),
            &env("CODEHUB_TEST_S3_SECRET_KEY"),
            MAX_UPLOAD_SIZE,
        )
        .unwrap(),
    )
}

async fn bind<I: IdentityVerification + Send + Sync + 'static>(
    stores: &'static Stores,
    identity: &'static I,
) -> Server {
    let assembly = stores.assembly(identity, Administration::BuiltIn, MAX_UPLOAD_SIZE);
    let router = adapter_http::api_router(
        assembly.authentication(),
        assembly.repositories(),
        assembly.members(),
        assembly.source_control(),
        assembly.runners(),
        assembly.jobs(),
        assembly.checks(),
        RunnerSettings {
            hold: HOLD,
            retry_interval: Duration::from_millis(50),
            heartbeat_interval: Duration::from_secs(10),
            pause,
        },
        (),
    );
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
