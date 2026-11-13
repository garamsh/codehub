//! The `codehub` server and its composition site: the one place that builds the adapters, hands
//! them to the use cases and serves the HTTP API over them (ADR 0024).

use std::fmt;
use std::io::Write;
use std::net::SocketAddr;
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use adapter_http::{Pause, RunnerSettings};
use adapter_toml::TomlCheckDefinitions;

use adapter_authzen::{AuthzenAuthorisationDecision, Profile};
use adapter_introspection::IntrospectionIdentityVerification;
use adapter_oidc::OidcIdentityVerification;
use adapter_postgres::{
    Database, DatabaseError, PostgresAuthorisationDecision, PostgresChangesetLog,
    PostgresDerivedData, PostgresDraftStore, PostgresJobQueue, PostgresMembership,
    PostgresRepositoryDirectory, PostgresRunnerRegistry,
};
use adapter_s3::S3ObjectStore;
use app::domain::credential::Credential;
use app::domain::path::PathError;
use app::domain::principal::Principal;
use app::port::driven::authorisation_decision::{
    Action, AuthorisationDecision, AuthorisationError, Decision, Resource,
};
use app::port::driven::identity_verification::{IdentityError, IdentityVerification};
use app::port::driven::object_store::ObjectStoreError;
use app::use_case::administration::Administration;
use app::use_case::authentication::AuthenticationService;
use app::use_case::checked_source_control::{CheckedAdapters, CheckedSourceControl};
use app::use_case::checks::{ChecksAdapters, ChecksService};
use app::use_case::jobs::{JobsAdapters, JobsService};
use app::use_case::landing_attempts::LandingAttempts;
use app::use_case::members::{MembersAdapters, MembersService};
use app::use_case::repositories::{RepositoriesAdapters, RepositoriesService};
use app::use_case::runners::{RunnersAdapters, RunnersService};
use app::use_case::source_control::{SourceControlAdapters, SourceControlService};
use oauth_client::{Authentication, InvalidClient, TokenClient};
use tokio::net::TcpListener;
use tokio::signal::unix::{SignalKind, signal};

mod readiness;
mod settings;

use readiness::Probe;
use settings::require;
pub use settings::{Authorisation, AuthzenClientAuthentication, AuthzenProfile, Config, Identity};

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

/// Builds the identity verification `config` selects.
fn verifier(config: &Config) -> Result<Verifier, Error> {
    match config.identity {
        Identity::Oidc => OidcIdentityVerification::new(
            require(&config.oidc_issuer, "--oidc-issuer (CODEHUB_OIDC_ISSUER)")?,
            require(
                &config.oidc_audience,
                "--oidc-audience (CODEHUB_OIDC_AUDIENCE)",
            )?,
            Duration::from_secs(config.jwks_min_refetch_seconds),
        )
        .map(|verifier| Verifier::Oidc(Box::new(verifier))),
        Identity::Introspection => IntrospectionIdentityVerification::new(
            require(
                &config.introspection_endpoint,
                "--introspection-endpoint (CODEHUB_INTROSPECTION_ENDPOINT)",
            )?,
            require(
                &config.introspection_client_id,
                "--introspection-client-id (CODEHUB_INTROSPECTION_CLIENT_ID)",
            )?,
            require(
                &config.introspection_client_secret,
                "--introspection-client-secret (CODEHUB_INTROSPECTION_CLIENT_SECRET)",
            )?,
            require(
                &config.introspection_issuer,
                "--introspection-issuer (CODEHUB_INTROSPECTION_ISSUER)",
            )?,
            require(
                &config.introspection_audience,
                "--introspection-audience (CODEHUB_INTROSPECTION_AUDIENCE)",
            )?,
        )
        .map(Verifier::Introspection),
    }
    .map_err(Error::Identity)
}

/// Builds the AuthZEN decision point's adapter where `config` selects it, with the one token
/// client it shares, before anything else is connected. The profile and the client
/// authentication are checked together: the `grn` profile speaks to a decision point that binds
/// each token to an organisation (ADR 0043), so it needs `private-key-jwt` (ADR 0042).
fn authzen(config: &Config) -> Result<Option<AuthzenAuthorisationDecision>, Error> {
    if config.authorisation != Authorisation::Authzen {
        return Ok(None);
    }
    let url = require(&config.authzen_url, "--authzen-url (CODEHUB_AUTHZEN_URL)")?;
    let token_endpoint = require(
        &config.authzen_token_endpoint,
        "--authzen-token-endpoint (CODEHUB_AUTHZEN_TOKEN_ENDPOINT)",
    )?;
    let client_id = require(
        &config.authzen_client_id,
        "--authzen-client-id (CODEHUB_AUTHZEN_CLIENT_ID)",
    )?;
    let authentication = match config.authzen_client_authentication {
        AuthzenClientAuthentication::ClientSecret => {
            if config.authzen_profile == AuthzenProfile::Grn {
                return Err(Error::IncompatibleSettings(
                    "CODEHUB_AUTHZEN_PROFILE=grn needs \
                     CODEHUB_AUTHZEN_CLIENT_AUTHENTICATION=private-key-jwt",
                ));
            }
            Authentication::ClientSecret {
                client_id: client_id.to_owned(),
                client_secret: require(
                    &config.authzen_client_secret,
                    "--authzen-client-secret (CODEHUB_AUTHZEN_CLIENT_SECRET)",
                )?
                .to_owned(),
            }
        }
        AuthzenClientAuthentication::PrivateKeyJwt => {
            let key_file = config
                .authzen_key_file
                .as_ref()
                .ok_or(Error::MissingSetting(
                    "--authzen-key-file (CODEHUB_AUTHZEN_KEY_FILE)",
                ))?;
            let issuer = require(
                &config.authzen_issuer,
                "--authzen-issuer (CODEHUB_AUTHZEN_ISSUER)",
            )?;
            let key_pem =
                std::fs::read(key_file).map_err(|error| Error::KeyFile(key_file.clone(), error))?;
            Authentication::PrivateKeyJwt {
                client_id_template: client_id.to_owned(),
                key_pem,
                key_id: config.authzen_key_id.clone(),
                issuer: issuer.to_owned(),
            }
        }
    };
    let tokens = TokenClient::new(
        token_endpoint,
        config.authzen_audience.clone(),
        authentication,
    )
    .map_err(Error::Client)?;
    let profile = match config.authzen_profile {
        AuthzenProfile::Plain => Profile::Plain,
        AuthzenProfile::Grn => Profile::Grn,
    };
    AuthzenAuthorisationDecision::new(url, Arc::new(tokens), profile)
        .map(|decider| Some(decider.with_access_evaluations(config.authzen_evaluations)))
        .map_err(Error::Authorisation)
}

/// The authorisation decision: the `authzen` adapter where [`authzen`] built one, else PostgreSQL
/// over `database`.
fn decider(database: &Database, authzen: Option<AuthzenAuthorisationDecision>) -> Decider {
    authzen.map_or_else(
        || Decider::Postgres(PostgresAuthorisationDecision::new(database)),
        |decider| Decider::Authzen(Box::new(decider)),
    )
}

/// Connects, migrates, binds and serves until SIGTERM or SIGINT, then finishes the requests in
/// flight and returns.
///
/// # Errors
///
/// An [`Error`] naming the step that failed.
pub async fn run(config: Config) -> Result<(), Error> {
    config.check_runner_lease()?;
    let identity = verifier(&config)?;
    let authzen = authzen(&config)?;
    let administration = match config.authorisation {
        Authorisation::Postgres => Administration::BuiltIn,
        Authorisation::Authzen => Administration::External,
    };
    let database = Database::connect(&config.postgres_url, config.postgres_pool_size)
        .await
        .map_err(Error::Database)?;
    database.migrate().await.map_err(Error::Database)?;
    let authorisation = decider(&database, authzen);
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
    }));
    let attempts = LandingAttempts::new(config.landing_attempts);
    let router = adapter_http::router(
        AuthenticationService::new(&adapters.identity),
        RepositoriesService::new(
            RepositoriesAdapters {
                directory: &adapters.directory,
                authorisation: &adapters.authorisation,
                membership: &adapters.membership,
                changes: &(),
            },
            administration,
        ),
        MembersService::new(
            MembersAdapters {
                directory: &adapters.directory,
                authorisation: &adapters.authorisation,
                membership: &adapters.membership,
            },
            administration,
        ),
        CheckedSourceControl::new(
            SourceControlService::new(
                SourceControlAdapters {
                    objects: &adapters.objects,
                    log: &adapters.log,
                    derived: &adapters.derived,
                    drafts: &adapters.drafts,
                    directory: &adapters.directory,
                    authorisation: &adapters.authorisation,
                },
                config.max_object_size,
            )
            .with_landing_attempts(attempts),
            CheckedAdapters {
                objects: &adapters.objects,
                derived: &adapters.derived,
                log: &adapters.log,
                directory: &adapters.directory,
                queue: &adapters.queue,
                definitions: &adapters.definitions,
            },
            attempts,
        ),
    )
    .merge(adapter_http::runner_router(
        AuthenticationService::new(&adapters.identity),
        RepositoriesService::new(
            RepositoriesAdapters {
                directory: &adapters.directory,
                authorisation: &adapters.authorisation,
                membership: &adapters.membership,
                changes: &(),
            },
            administration,
        ),
        RunnersService::new(RunnersAdapters {
            registry: &adapters.registry,
            directory: &adapters.directory,
            authorisation: &adapters.authorisation,
        }),
        JobsService::new(
            JobsAdapters {
                registry: &adapters.registry,
                queue: &adapters.queue,
                objects: &adapters.objects,
                derived: &adapters.derived,
                log: &adapters.log,
                directory: &adapters.directory,
                definitions: &adapters.definitions,
            },
            i64::from(config.runner_lease.get()),
            attempts,
        ),
        ChecksService::new(
            ChecksAdapters {
                queue: &adapters.queue,
                log: &adapters.log,
                directory: &adapters.directory,
                authorisation: &adapters.authorisation,
                objects: &adapters.objects,
                derived: &adapters.derived,
                definitions: &adapters.definitions,
            },
            attempts,
        ),
        RunnerSettings {
            hold: Duration::from_secs(config.runner_claim_hold),
            retry_interval: CLAIM_RETRY_INTERVAL,
            heartbeat_interval: Duration::from_secs(u64::from(
                config.runner_heartbeat_interval.get(),
            )),
            pause,
        },
    ))
    .merge(adapter_http::health_router(Probe {
        directory: &adapters.directory,
        objects: &adapters.objects,
    }));
    let listener = TcpListener::bind(config.listen)
        .await
        .map_err(|error| Error::Bind(config.listen, error))?;
    let address = listener.local_addr().map_err(Error::Serve)?;
    let mut terminate = signal(SignalKind::terminate()).map_err(Error::Serve)?;
    let mut interrupt = signal(SignalKind::interrupt()).map_err(Error::Serve)?;
    // Nowhere left to report a failed stderr write.
    let _ = writeln!(std::io::stderr(), "codehub listening on {address}");
    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            tokio::select! {
                _ = terminate.recv() => {}
                _ = interrupt.recv() => {}
            }
        })
        .await
        .map_err(Error::Serve)
}
