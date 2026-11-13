//! The `codehub` server and its composition site: the one place that builds the adapters, hands
//! them to the use cases and serves the HTTP API over them (ADR 0024).

use std::fmt;
use std::io::Write;
use std::net::SocketAddr;
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use adapter_http::{Pause, RunnerSettings};
use adapter_toml::TomlCheckDefinitions;

use adapter_authzen::{AuthzenAuthorisationDecision, Profile};
use adapter_garam_directory::GaramDirectoryChanges;
use adapter_introspection::IntrospectionIdentityVerification;
use adapter_oidc::OidcIdentityVerification;
use adapter_postgres::{
    Database, DatabaseError, PostgresAuthorisationDecision, PostgresChangesetLog,
    PostgresDerivedData, PostgresDirectoryOutbox, PostgresDraftStore, PostgresJobQueue,
    PostgresMembership, PostgresRepositoryDirectory, PostgresRunnerRegistry,
};
use adapter_s3::S3ObjectStore;
use app::domain::credential::Credential;
use app::domain::path::PathError;
use app::domain::principal::Principal;
use app::port::driven::authorisation_decision::{
    Action, AuthorisationDecision, AuthorisationError, Decision, Resource,
};
use app::port::driven::directory_changes::{
    DirectoryChanges, OrganisationChange, RepositoryChange,
};
use app::port::driven::identity_verification::{IdentityError, IdentityVerification};
use app::port::driven::object_store::ObjectStoreError;
use app::use_case::assembly::Assembly;
use app::use_case::landing_attempts::LandingAttempts;
use app::use_case::relay::{DirectoryRelay, RelayEnvironment};
use oauth_client::{InvalidClient, TokenClient};
use tokio::net::TcpListener;
use tokio::signal::unix::{SignalKind, signal};

mod readiness;
mod settings;

use readiness::Probe;
pub use settings::{
    Authorisation, AuthzenClientAuthentication, AuthzenProfile, Config, DirectoryChangesMode,
    Identity,
};
use settings::{AuthorisationMode, AuthzenMode, IdentityMode};

/// Why the server did not start or stopped serving.
#[derive(Debug)]
pub enum Error {
    /// The database could not be reached or migrated.
    Database(DatabaseError),
    /// The object store could not be configured.
    ObjectStore(ObjectStoreError),
    /// Identity verification could not be configured.
    Identity(IdentityError),
    /// Authorisation could not be configured.
    Authorisation(AuthorisationError),
    /// A setting the selected identity or authorisation mode needs is missing; carries its flag
    /// and variable.
    MissingSetting(&'static str),
    /// Settings that cannot go together; carries what to change.
    IncompatibleSettings(&'static str),
    /// The file named by `CODEHUB_AUTHZEN_KEY_FILE` could not be read.
    KeyFile(PathBuf, std::io::Error),
    /// The AuthZEN client could not be built from its settings.
    Client(InvalidClient),
    /// The check definitions reader could not be built.
    Definitions(PathError),
    /// The garam directory adapter could not be built; carries a description.
    DirectoryChanges(String),
    /// The runner lease does not exceed the heartbeat interval; carries both, in seconds.
    RunnerLease {
        /// `CODEHUB_RUNNER_LEASE`.
        lease: NonZeroU32,
        /// `CODEHUB_RUNNER_HEARTBEAT_INTERVAL`.
        heartbeat: NonZeroU32,
    },
    /// The listening address could not be bound.
    Bind(SocketAddr, std::io::Error),
    /// Signal handlers could not be installed, the address read or the server run.
    Serve(std::io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Database(error) => error.fmt(f),
            Self::ObjectStore(error) => write!(f, "configuring the object store failed: {error}"),
            Self::Identity(error) => {
                write!(f, "configuring identity verification failed: {error}")
            }
            Self::Authorisation(error) => {
                write!(f, "configuring authorisation failed: {error}")
            }
            Self::MissingSetting(setting) => {
                write!(
                    f,
                    "the selected identity or authorisation mode needs {setting}"
                )
            }
            Self::IncompatibleSettings(reason) => f.write_str(reason),
            Self::KeyFile(path, error) => write!(
                f,
                "reading the key file {} named by CODEHUB_AUTHZEN_KEY_FILE failed: {error}",
                path.display()
            ),
            Self::Client(error) => write!(
                f,
                "the AuthZEN client settings (CODEHUB_AUTHZEN_CLIENT_ID, \
                 CODEHUB_AUTHZEN_KEY_FILE) are invalid: {error}"
            ),
            Self::Definitions(error) => {
                write!(f, "configuring the check definitions failed: {error}")
            }
            Self::DirectoryChanges(description) => {
                write!(f, "configuring the directory changes failed: {description}")
            }
            Self::RunnerLease { lease, heartbeat } => write!(
                f,
                "CODEHUB_RUNNER_LEASE ({lease} s) must be greater than \
                 CODEHUB_RUNNER_HEARTBEAT_INTERVAL ({heartbeat} s)"
            ),
            Self::Bind(address, error) => write!(f, "binding {address} failed: {error}"),
            Self::Serve(error) => write!(f, "serving failed: {error}"),
        }
    }
}

impl std::error::Error for Error {}

/// How long a claim that found no job waits before it looks again.
const CLAIM_RETRY_INTERVAL: Duration = Duration::from_millis(500);

/// Waits for `duration` on the server's runtime.
fn pause(duration: Duration) -> Pause {
    Box::pin(tokio::time::sleep(duration))
}

/// The adapters of the deployment.
struct Adapters {
    objects: S3ObjectStore,
    log: PostgresChangesetLog,
    derived: PostgresDerivedData,
    drafts: PostgresDraftStore,
    directory: PostgresRepositoryDirectory,
    membership: PostgresMembership,
    registry: PostgresRunnerRegistry,
    queue: PostgresJobQueue,
    definitions: TomlCheckDefinitions,
    authorisation: Decider,
    identity: Verifier,
    changes: Arc<Changes>,
    relay: Arc<DirectoryRelay<PostgresDirectoryOutbox, Changes, ServerEnvironment>>,
}

/// Where the directory changes go, as the configuration selected.
enum Changes {
    /// Nowhere: every repository change is accepted at once.
    Nowhere,
    /// garam's `ApplyEntityChanges`.
    Garam(Box<GaramDirectoryChanges>),
}

impl DirectoryChanges for Changes {
    async fn repository_changed(&self, change: &RepositoryChange) -> bool {
        match self {
            Self::Nowhere => ().repository_changed(change).await,
            Self::Garam(changes) => changes.repository_changed(change).await,
        }
    }

    async fn organisation_changed(&self, change: &OrganisationChange) {
        match self {
            Self::Nowhere => ().organisation_changed(change).await,
            Self::Garam(changes) => changes.organisation_changed(change).await,
        }
    }
}

/// What the relay takes from the process: the system clock, the runtime's timer and stderr.
struct ServerEnvironment;

impl RelayEnvironment for ServerEnvironment {
    fn now(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| {
                i64::try_from(since.as_secs()).unwrap_or(i64::MAX)
            })
    }

    fn sleep(&self, duration: Duration) -> impl Future<Output = ()> + Send {
        tokio::time::sleep(duration)
    }

    fn log(&self, line: &str) {
        // Nowhere left to report a failed stderr write.
        let _ = writeln!(std::io::stderr(), "directory relay: {line}");
    }
}

/// The identity verification the configuration selected.
enum Verifier {
    Oidc(Box<OidcIdentityVerification>),
    Introspection(IntrospectionIdentityVerification),
}

impl IdentityVerification for Verifier {
    async fn verify(&self, credential: &Credential) -> Result<Principal, IdentityError> {
        match self {
            Self::Oidc(verifier) => verifier.verify(credential).await,
            Self::Introspection(verifier) => verifier.verify(credential).await,
        }
    }
}

/// The authorisation decision the configuration selected.
enum Decider {
    Postgres(PostgresAuthorisationDecision),
    Authzen(Box<AuthzenAuthorisationDecision>),
}

impl AuthorisationDecision for Decider {
    async fn decide(
        &self,
        subject: &Principal,
        action: Action,
        resource: &Resource,
    ) -> Result<Decision, AuthorisationError> {
        match self {
            Self::Postgres(decider) => decider.decide(subject, action, resource).await,
            Self::Authzen(decider) => decider.decide(subject, action, resource).await,
        }
    }

    async fn decide_all(
        &self,
        subject: &Principal,
        action: Action,
        resources: &[Resource],
    ) -> Result<Vec<Decision>, AuthorisationError> {
        match self {
            Self::Postgres(decider) => decider.decide_all(subject, action, resources).await,
            Self::Authzen(decider) => decider.decide_all(subject, action, resources).await,
        }
    }
}

/// Builds the identity verification `selection` names.
fn verifier(selection: IdentityMode<'_>, config: &Config) -> Result<Verifier, Error> {
    match selection {
        IdentityMode::Oidc { issuer, audience } => OidcIdentityVerification::new(
            issuer,
            audience,
            Duration::from_secs(config.jwks_min_refetch_seconds),
        )
        .map(|verifier| Verifier::Oidc(Box::new(verifier))),
        IdentityMode::Introspection {
            endpoint,
            client_id,
            client_secret,
            issuer,
            audience,
        } => IntrospectionIdentityVerification::new(
            endpoint,
            client_id,
            client_secret,
            issuer,
            audience,
        )
        .map(Verifier::Introspection),
    }
    .map_err(Error::Identity)
}

/// Builds the AuthZEN decision point's adapter and where the directory changes go, with the one
/// token client they share: garam binds each token to an organisation, so the changes are sent
/// under the `grn` profile's clients (ADR 0043).
fn authzen(
    mode: AuthzenMode<'_>,
    config: &Config,
) -> Result<(AuthzenAuthorisationDecision, Changes), Error> {
    let tokens = Arc::new(
        TokenClient::new(
            mode.token_endpoint,
            config.authzen_audience.clone(),
            mode.authentication,
        )
        .map_err(Error::Client)?,
    );
    let profile = match mode.profile {
        AuthzenProfile::Plain => Profile::Plain,
        AuthzenProfile::Grn => Profile::Grn,
    };
    let decider = AuthzenAuthorisationDecision::new(mode.url, Arc::clone(&tokens), profile)
        .map_err(Error::Authorisation)?
        .with_access_evaluations(config.authzen_evaluations);
    let changes = match mode.garam_url {
        None => Changes::Nowhere,
        Some(url) => GaramDirectoryChanges::new(url, tokens)
            .map(|changes| Changes::Garam(Box::new(changes)))
            .map_err(Error::DirectoryChanges)?,
    };
    Ok((decider, changes))
}

/// Connects, migrates, binds and serves until SIGTERM or SIGINT, then finishes the requests in
/// flight and returns.
///
/// # Errors
///
/// An [`Error`] naming the step that failed.
pub async fn run(config: Config) -> Result<(), Error> {
    let selection = config.select()?;
    let administration = selection.administration;
    let identity = verifier(selection.identity, &config)?;
    let (authzen, changes) = match selection.authorisation {
        AuthorisationMode::Postgres => (None, Changes::Nowhere),
        AuthorisationMode::Authzen(mode) => {
            let (decider, changes) = authzen(mode, &config)?;
            (Some(decider), changes)
        }
    };
    let changes = Arc::new(changes);
    let database = Database::connect(&config.postgres_url, config.postgres_pool_size)
        .await
        .map_err(Error::Database)?;
    database.migrate().await.map_err(Error::Database)?;
    let authorisation = authzen.map_or_else(
        || Decider::Postgres(PostgresAuthorisationDecision::new(&database)),
        |decider| Decider::Authzen(Box::new(decider)),
    );
    let relay = Arc::new(DirectoryRelay::new(
        Arc::new(PostgresDirectoryOutbox::new(&database)),
        Arc::clone(&changes),
        Arc::new(ServerEnvironment),
        Duration::from_secs(config.directory_delivery_wait),
    ));
    let objects = S3ObjectStore::new(
        &config.s3_endpoint,
        &config.s3_region,
        &config.s3_bucket,
        &config.s3_access_key,
        &config.s3_secret_key,
        config.max_object_size,
    )
    .map_err(Error::ObjectStore)?
    .with_expiry(Duration::from_secs(config.grant_expiry_seconds));
    // The use cases borrow the adapters and the router holds its ports for `'static`; the
    // adapters live as long as the process.
    let adapters: &'static Adapters = Box::leak(Box::new(Adapters {
        objects,
        log: PostgresChangesetLog::new(&database),
        derived: PostgresDerivedData::new(&database),
        drafts: PostgresDraftStore::new(&database),
        directory: PostgresRepositoryDirectory::new(&database),
        membership: PostgresMembership::new(&database),
        registry: PostgresRunnerRegistry::new(&database),
        queue: PostgresJobQueue::new(&database),
        definitions: TomlCheckDefinitions::new().map_err(Error::Definitions)?,
        authorisation,
        identity,
        changes,
        relay,
    }));
    let assembly = Assembly {
        objects: &adapters.objects,
        log: &adapters.log,
        derived: &adapters.derived,
        drafts: &adapters.drafts,
        directory: &adapters.directory,
        authorisation: &adapters.authorisation,
        membership: &adapters.membership,
        queue: &adapters.queue,
        registry: &adapters.registry,
        definitions: &adapters.definitions,
        changes: &*adapters.changes,
        delivery: &*adapters.relay,
        identity: &adapters.identity,

        administration,
        max_upload_size: config.max_object_size,
        attempts: LandingAttempts::new(config.landing_attempts),
        lease: i64::from(config.runner_lease.get()),
    };
    let router = adapter_http::api_router(
        &assembly,
        RunnerSettings {
            hold: Duration::from_secs(config.runner_claim_hold),
            retry_interval: CLAIM_RETRY_INTERVAL,
            heartbeat_interval: Duration::from_secs(u64::from(
                config.runner_heartbeat_interval.get(),
            )),
            pause,
        },
        Probe {
            directory: &adapters.directory,
            objects: &adapters.objects,
        },
    );
    let listener = TcpListener::bind(config.listen)
        .await
        .map_err(|error| Error::Bind(config.listen, error))?;
    let address = listener.local_addr().map_err(Error::Serve)?;
    let mut terminate = signal(SignalKind::terminate()).map_err(Error::Serve)?;
    let mut interrupt = signal(SignalKind::interrupt()).map_err(Error::Serve)?;
    // Nowhere left to report a failed stderr write.
    let _ = writeln!(std::io::stderr(), "codehub listening on {address}");
    let relay = Arc::clone(&adapters.relay);
    let delivering = tokio::spawn(async move { relay.run().await });
    let served = axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            tokio::select! {
                _ = terminate.recv() => {}
                _ = interrupt.recv() => {}
            }
        })
        .await
        .map_err(Error::Serve);
    delivering.abort();
    served
}
