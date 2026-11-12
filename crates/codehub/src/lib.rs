//! The `codehub` server and its composition site: the one place that builds the adapters, hands
//! them to the use cases and serves the HTTP API over them (ADR 0024).

use std::fmt;
use std::io::Write;
use std::net::SocketAddr;
use std::time::Duration;

use adapter_authzen::AuthzenAuthorisationDecision;
use adapter_introspection::IntrospectionIdentityVerification;
use adapter_oidc::OidcIdentityVerification;
use adapter_postgres::{
    Database, DatabaseError, PostgresAuthorisationDecision, PostgresChangesetLog,
    PostgresDerivedData, PostgresDraftStore, PostgresMembership, PostgresRepositoryDirectory,
};
use adapter_s3::S3ObjectStore;
use app::domain::credential::Credential;
use app::domain::principal::Principal;
use app::port::driven::authorisation_decision::{
    Action, AuthorisationDecision, AuthorisationError, Decision, Resource,
};
use app::port::driven::identity_verification::{IdentityError, IdentityVerification};
use app::port::driven::object_store::ObjectStoreError;
use app::use_case::administration::Administration;
use app::use_case::authentication::AuthenticationService;
use app::use_case::members::{MembersAdapters, MembersService};
use app::use_case::repositories::{RepositoriesAdapters, RepositoriesService};
use app::use_case::source_control::{SourceControlAdapters, SourceControlService};
use tokio::net::TcpListener;
use tokio::signal::unix::{SignalKind, signal};

mod settings;

use settings::require;
pub use settings::{Authorisation, Config, Identity};

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
            Self::Bind(address, error) => write!(f, "binding {address} failed: {error}"),
            Self::Serve(error) => write!(f, "serving failed: {error}"),
        }
    }
}

impl std::error::Error for Error {}

/// The adapters of the deployment.
struct Adapters {
    objects: S3ObjectStore,
    log: PostgresChangesetLog,
    derived: PostgresDerivedData,
    drafts: PostgresDraftStore,
    directory: PostgresRepositoryDirectory,
    membership: PostgresMembership,
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
    Authzen(AuthzenAuthorisationDecision),
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

/// Builds the authorisation decision `config` selects, over `database` where it is PostgreSQL.
fn decider(config: &Config, database: &Database) -> Result<Decider, Error> {
    match config.authorisation {
        Authorisation::Postgres => Ok(Decider::Postgres(PostgresAuthorisationDecision::new(
            database,
        ))),
        Authorisation::Authzen => AuthzenAuthorisationDecision::new(
            require(&config.authzen_url, "--authzen-url (CODEHUB_AUTHZEN_URL)")?,
            require(
                &config.authzen_token_endpoint,
                "--authzen-token-endpoint (CODEHUB_AUTHZEN_TOKEN_ENDPOINT)",
            )?,
            require(
                &config.authzen_client_id,
                "--authzen-client-id (CODEHUB_AUTHZEN_CLIENT_ID)",
            )?,
            require(
                &config.authzen_client_secret,
                "--authzen-client-secret (CODEHUB_AUTHZEN_CLIENT_SECRET)",
            )?,
        )
        .map(Decider::Authzen)
        .map_err(Error::Authorisation),
    }
}

/// Connects, migrates, binds and serves until SIGTERM or SIGINT, then finishes the requests in
/// flight and returns.
///
/// # Errors
///
/// An [`Error`] naming the step that failed.
pub async fn run(config: Config) -> Result<(), Error> {
    let identity = verifier(&config)?;
    let administration = match config.authorisation {
        Authorisation::Postgres => Administration::BuiltIn,
        Authorisation::Authzen => Administration::External,
    };
    let database = Database::connect(&config.postgres_url, config.postgres_pool_size)
        .await
        .map_err(Error::Database)?;
    database.migrate().await.map_err(Error::Database)?;
    let authorisation = decider(&config, &database)?;
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
        authorisation,
        identity,
    }));
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
        ),
    );
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
