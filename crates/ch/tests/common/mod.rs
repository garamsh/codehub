//! The HTTP API served in process over the memory adapters and the S3 object store, and users that
//! run the built `ch` against it with a state file of their own.
//!
//! Every helper panics when something is not as expected, as a failed assertion does.

use std::ops::Deref;
use std::path::{Path, PathBuf};

use base64::Engine;

use adapter_memory::{
    MemoryAuthorisationDecision, MemoryChangesetLog, MemoryDerivedData, MemoryDraftStore,
    MemoryIdentityVerification, MemoryMembership, MemoryRepositoryDirectory,
};
use adapter_s3::S3ObjectStore;
use app::domain::principal::Principal;
use app::use_case::administration::Administration;
use app::use_case::authentication::AuthenticationService;
use app::use_case::members::MembersService;
use app::use_case::repositories::RepositoriesService;
use app::use_case::source_control::SourceControlService;

pub use test_support::{ChUser, Run, env, unique};

/// The largest upload the server grants.
const MAX_UPLOAD_SIZE: u64 = 1024 * 1024;

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
    let membership = MemoryMembership::default();
    let stores: &'static Stores = Box::leak(Box::new(Stores {
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
    }));
    let router = adapter_http::router(
        AuthenticationService::new(&stores.identity),
        RepositoriesService::new(
            &stores.directory,
            &stores.authorisation,
            &stores.membership,
            &(),
            Administration::BuiltIn,
        ),
        MembersService::new(
            &stores.directory,
            &stores.authorisation,
            &stores.membership,
            Administration::BuiltIn,
        ),
        SourceControlService::new(
            &stores.objects,
            &stores.log,
            &stores.derived,
            &stores.drafts,
            &stores.directory,
            &stores.authorisation,
            MAX_UPLOAD_SIZE,
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    Server { url, stores }
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
        self.user_with_token(&token)
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
