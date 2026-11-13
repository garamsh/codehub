//! The HTTP API served in process over the memory adapters and the S3 object store, and users that
//! run the built `ch` against it with a state file of their own.
//!
//! Every helper panics when something is not as expected, as a failed assertion does.

use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine;

use adapter_http::{Pause, RunnerSettings};
use adapter_memory::{
    MemoryAuthorisationDecision, MemoryChangesetLog, MemoryCheckDefinitions, MemoryDerivedData,
    MemoryDraftStore, MemoryIdentityVerification, MemoryJobQueue, MemoryMembership,
    MemoryRepositoryDirectory, MemoryRunnerRegistry,
};
use adapter_oidc::OidcIdentityVerification;
use adapter_s3::S3ObjectStore;
use app::domain::check::{CheckDefinition, Command, Timeout};
use app::domain::job::NewJob;
use app::domain::name::{OrganisationName, RepositoryName};
use app::domain::principal::Principal;
use app::domain::revert::Revert;
use app::port::driven::identity_verification::IdentityVerification;
use app::port::driven::job_queue::JobQueue;
use app::port::driven::repository_directory::RepositoryDirectory;
use app::port::driving::repositories::Repositories;
use app::use_case::administration::Administration;
use app::use_case::authentication::AuthenticationService;
use app::use_case::checks::{ChecksAdapters, ChecksService};
use app::use_case::jobs::{JobsAdapters, JobsService};
use app::use_case::landing_attempts::LandingAttempts;
use app::use_case::members::{MembersAdapters, MembersService};
use app::use_case::repositories::{RepositoriesAdapters, RepositoriesService};
use app::use_case::runners::{RunnersAdapters, RunnersService};
use app::use_case::source_control::{SourceControlAdapters, SourceControlService};
use serde_json::json;

pub use test_support::{ChUser, Run, env, unique};

/// The largest upload the server grants.
const MAX_UPLOAD_SIZE: u64 = 1024 * 1024;

/// How long the server holds a claim that finds no job.
const HOLD: Duration = Duration::from_millis(300);

/// How long the server's jobs service holds a job between heartbeats, in seconds.
const LEASE: i64 = 60;

/// The adapters of one server.
struct Stores {
    objects: S3ObjectStore,
    log: MemoryChangesetLog,
    derived: MemoryDerivedData,
    drafts: MemoryDraftStore,
    directory: MemoryRepositoryDirectory,
    identity: MemoryIdentityVerification,
    membership: MemoryMembership,
    authorisation: MemoryAuthorisationDecision,
    registry: MemoryRunnerRegistry,
    queue: MemoryJobQueue,
    definitions: MemoryCheckDefinitions,
}

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
    let membership = MemoryMembership::default();
    Box::leak(Box::new(Stores {
        objects: S3ObjectStore::new(
            &env("CODEHUB_TEST_S3_ENDPOINT"),
            &env("CODEHUB_TEST_S3_REGION"),
            &env("CODEHUB_TEST_S3_BUCKET"),
            &env("CODEHUB_TEST_S3_ACCESS_KEY"),
            &env("CODEHUB_TEST_S3_SECRET_KEY"),
            MAX_UPLOAD_SIZE,
        )
        .unwrap(),
        log: MemoryChangesetLog::default(),
        derived: MemoryDerivedData::default(),
        drafts: MemoryDraftStore::default(),
        directory: MemoryRepositoryDirectory::default(),
        identity: MemoryIdentityVerification::default(),
        authorisation: membership.authorisation_decision(),
        membership,
        registry: MemoryRunnerRegistry::default(),
        queue: MemoryJobQueue::default(),
        definitions: MemoryCheckDefinitions::new(".codehub/checks.toml".parse().unwrap()),
    }))
}

async fn bind<I: IdentityVerification + Send + Sync + 'static>(
    stores: &'static Stores,
    identity: &'static I,
) -> Server {
    let router = adapter_http::router(
        AuthenticationService::new(identity),
        repositories(stores),
        MembersService::new(
            MembersAdapters {
                directory: &stores.directory,
                authorisation: &stores.authorisation,
                membership: &stores.membership,
            },
            Administration::BuiltIn,
        ),
        SourceControlService::new(
            SourceControlAdapters {
                objects: &stores.objects,
                log: &stores.log,
                derived: &stores.derived,
                drafts: &stores.drafts,
                directory: &stores.directory,
                authorisation: &stores.authorisation,
            },
            MAX_UPLOAD_SIZE,
        ),
    )
    .merge(runner_router(stores, identity));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    Server { url, stores }
}

fn repositories(stores: &'static Stores) -> impl Repositories + Send + Sync + 'static {
    RepositoriesService::new(
        RepositoriesAdapters {
            directory: &stores.directory,
            authorisation: &stores.authorisation,
            membership: &stores.membership,
            changes: &(),
            delivery: &(),
        },
        Administration::BuiltIn,
    )
}

fn pause(duration: Duration) -> Pause {
    Box::pin(tokio::time::sleep(duration))
}

fn runner_router<I: IdentityVerification + Send + Sync + 'static>(
    stores: &'static Stores,
    identity: &'static I,
) -> axum::Router {
    adapter_http::runner_router(
        AuthenticationService::new(identity),
        repositories(stores),
        RunnersService::new(RunnersAdapters {
            registry: &stores.registry,
            directory: &stores.directory,
            authorisation: &stores.authorisation,
        }),
        JobsService::new(
            JobsAdapters {
                registry: &stores.registry,
                queue: &stores.queue,
                objects: &stores.objects,
                derived: &stores.derived,
                log: &stores.log,
                directory: &stores.directory,
                definitions: &stores.definitions,
            },
            LEASE,
            LandingAttempts::default(),
        ),
        ChecksService::new(
            ChecksAdapters {
                queue: &stores.queue,
                log: &stores.log,
                directory: &stores.directory,
                authorisation: &stores.authorisation,
                objects: &stores.objects,
                derived: &stores.derived,
                definitions: &stores.definitions,
            },
            LandingAttempts::default(),
        ),
        RunnerSettings {
            hold: HOLD,
            retry_interval: Duration::from_millis(50),
            heartbeat_interval: Duration::from_secs(10),
            pause,
        },
    )
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
    /// Queues the check `name` against `changeset` of `org`/`app`, with no labels, as a blocking
    /// check where `blocking`.
    ///
    /// # Panics
    ///
    /// Panics when the queue refuses it.
    pub async fn queue_check(&self, org: &str, changeset: &str, name: &str, blocking: bool) {
        let organisation: OrganisationName = org.parse().unwrap();
        let repository: RepositoryName = "app".parse().unwrap();
        let job = NewJob {
            repository: self
                .stores
                .directory
                .find_repository(&organisation, &repository)
                .await
                .unwrap(),
            organisation,
            changeset: changeset.parse().unwrap(),
            check: CheckDefinition {
                name: name.parse().unwrap(),
                command: Command::new("true").unwrap(),
                labels: std::collections::BTreeSet::new(),
                blocking,
                timeout: Timeout::new(60).unwrap(),
            },
        };
        self.stores.queue.queue(vec![job]).await.unwrap();
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
