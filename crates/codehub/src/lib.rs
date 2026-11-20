//! The `codehub` server and its composition site: the one place that builds the adapters, hands
//! them to the use cases and serves the HTTP API over them (ADR 0024).

use std::fmt;
use std::io::Write;
use std::net::SocketAddr;
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::response::IntoResponse;

use adapter_http::{ApiState, Pause, RunnerSettings};
use adapter_toml::TomlCheckDefinitions;

use adapter_authzen::{AuthzenAuthorisationDecision, Profile};
use adapter_garam_directory::GaramDirectoryChanges;
use adapter_introspection::IntrospectionIdentityVerification;
use adapter_oidc::OidcIdentityVerification;
use adapter_postgres::{
    Database, DatabaseError, PostgresAuthorisationDecision, PostgresBacklog, PostgresCacheRefs,
    PostgresChangesetLog, PostgresDerivedData, PostgresDirectoryOutbox, PostgresDraftStore,
    PostgresJobQueue, PostgresMembership, PostgresRepositoryDirectory, PostgresRunnerRegistry,
};
use adapter_s3::S3ObjectStore;
use app::domain::path::PathError;
use app::observability::Observed;
use app::port::driven::authorisation_decision::{AuthorisationDecision, AuthorisationError};
use app::port::driven::backlog::Backlog;
use app::port::driven::directory_changes::DirectoryChanges;
use app::port::driven::identity_verification::{IdentityError, IdentityVerification};
use app::port::driven::object_store::ObjectStoreError;
use app::port::driving::registry_token::RegistryToken;
use app::use_case::assembly::Assembly;
use app::use_case::landing_attempts::LandingAttempts;
use app::use_case::relay::{DirectoryRelay, RelayEnvironment};
use oauth_client::{Authentication, InvalidClient, PinnedCa, TokenClient};
use tokio::net::TcpListener;
use tokio::signal::unix::{SignalKind, signal};

mod overload;
mod postgres;
mod readiness;
mod registry;
mod settings;
mod telemetry;

pub use overload::{OverloadOptions, TimingOptions};
pub use postgres::{MigrationOptions, PostgresSettings, ReplicaOptions};
use readiness::Probe;
pub use registry::{RegistryError, RegistryOptions};
pub use settings::{
    AllOptions, ApiOptions, Authorisation, AuthorisationOptions, AuthzenClientAuthentication,
    AuthzenClientOptions, AuthzenProfile, Cli, DatabaseOptions, DeliveryOptions,
    DirectoryChangesMode, DirectoryOptions, Identity, IdentityOptions, LandingOptions,
    ListenOptions, MigrateOptions, ObjectOptions, Role, RunnerApiOptions, RunnerOptions,
    WorkerOptions,
};
use settings::{
    AuthorisationMode, AuthzenMode, IdentityMode, Serving, UNUSED_DELIVERY_WAIT, UNUSED_RUNNER,
    select_delivery,
};
use telemetry::Telemetry;
pub use telemetry::{BacklogOptions, LogFormat, TelemetryOptions};

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
    /// A file named by `CODEHUB_AUTHZEN_TOKEN_CA_FILE` or `CODEHUB_AUTHZEN_CA_FILE` could not be
    /// read or holds no certificate.
    CaFile {
        /// The variable that names the file.
        setting: &'static str,
        /// The file.
        path: PathBuf,
        /// Why it was refused.
        reason: String,
    },
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
    /// The log, the trace export or the metrics could not be set up; carries a description.
    Telemetry(String),
    /// The registry token endpoint could not be configured.
    Registry(RegistryError),
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
            Self::CaFile {
                setting,
                path,
                reason,
            } => write!(
                f,
                "the CA file {} named by {setting} is refused: {reason}",
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
            Self::Telemetry(description) => {
                write!(f, "configuring telemetry failed: {description}")
            }
            Self::Registry(error) => error.fmt(f),
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

/// Builds the identity verification `selection` names, fetching an issuer's key set no more than
/// once in `jwks_min_refetch`.
fn verifier(
    selection: IdentityMode<'_>,
    jwks_min_refetch: Duration,
    timeout: Duration,
) -> Result<Arc<dyn IdentityVerification>, Error> {
    match selection {
        IdentityMode::Oidc { issuer, audience } => {
            OidcIdentityVerification::new(issuer, audience, jwks_min_refetch, timeout).map(
                |verifier| {
                    Arc::new(Observed::new("identity", verifier)) as Arc<dyn IdentityVerification>
                },
            )
        }
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
            timeout,
        )
        .map(|verifier| {
            Arc::new(Observed::new("identity", verifier)) as Arc<dyn IdentityVerification>
        }),
    }
    .map_err(Error::Identity)
}

/// The adapters the authorisation mode selects: the decision point, and where directory changes go.
type Selected = (Arc<dyn AuthorisationDecision>, Arc<dyn DirectoryChanges>);

/// Builds the token client the AuthZEN decision point and the garam directory changes share:
/// garam binds each token to an organisation, so the changes are sent under the `grn` profile's
/// clients (ADR 0043).
fn token_client(
    token_endpoint: &str,
    audience: Option<String>,
    authentication: Authentication,
    ca: Option<&PinnedCa>,
    timeout: Duration,
) -> Result<Arc<TokenClient>, Error> {
    TokenClient::new(token_endpoint, audience, authentication, ca, timeout)
        .map(Arc::new)
        .map_err(Error::Client)
}

/// Builds the garam directory changes over `tokens`.
fn garam_changes(
    url: &str,
    tokens: Arc<TokenClient>,
    ca: Option<&PinnedCa>,
    timeout: Duration,
) -> Result<Arc<dyn DirectoryChanges>, Error> {
    GaramDirectoryChanges::new(url, tokens, ca, timeout)
        .map(|changes| Arc::new(Observed::new("directory", changes)) as Arc<dyn DirectoryChanges>)
        .map_err(Error::DirectoryChanges)
}

/// Builds the AuthZEN decision point's adapter and where the directory changes go.
fn authzen(
    mode: AuthzenMode<'_>,
    audience: Option<String>,
    evaluations: bool,
    timeout: Duration,
) -> Result<Selected, Error> {
    let tokens = token_client(
        mode.token_endpoint,
        audience,
        mode.authentication,
        mode.token_ca.as_ref(),
        timeout,
    )?;
    let profile = match mode.profile {
        AuthzenProfile::Plain => Profile::Plain,
        AuthzenProfile::Grn => Profile::Grn,
    };
    let decider = AuthzenAuthorisationDecision::new(
        mode.url,
        Arc::clone(&tokens),
        profile,
        mode.policy_ca.as_ref(),
        timeout,
    )
    .map_err(Error::Authorisation)?
    .with_access_evaluations(evaluations);
    let changes: Arc<dyn DirectoryChanges> = match mode.garam_url {
        None => Arc::new(()),
        Some(url) => garam_changes(url, tokens, mode.policy_ca.as_ref(), timeout)?,
    };
    Ok((Arc::new(Observed::new("decision_point", decider)), changes))
}

/// Which routes a serving process answers.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Routes {
    /// `/v1` except `/v1/runner/`.
    Api,
    /// `/v1/runner/`.
    RunnerApi,
    /// Every route, after migrating, with the directory outbox relayed in the same process.
    All,
}

/// What a role that serves routes is built from.
struct Http<'a> {
    routes: Routes,
    listen: SocketAddr,
    database: &'a DatabaseOptions,
    /// The read replica serving the immutable reads, if any.
    read_url: Option<&'a str>,
    /// Where migrations are applied, for the role that migrates.
    migration: Option<&'a MigrationOptions>,
    objects: &'a ObjectOptions,
    landing: &'a LandingOptions,
    overload: &'a OverloadOptions,
    timing: &'a TimingOptions,
    /// How long answering a creation waits for its change, in seconds.
    delivery_wait: u64,
    serving: Serving<'a>,
    telemetry: &'a Telemetry,
    /// How often the backlog is read, in seconds, where this role runs the background work.
    backlog: Option<NonZeroU32>,
    /// The registry token endpoint, for a role that serves the user API.
    registry: Option<&'a RegistryOptions>,
}

/// Connects, and migrates where this is the `all` role, binds and serves until SIGTERM or SIGINT,
/// then finishes the requests in flight and returns.
///
/// # Errors
///
/// An [`Error`] naming the step that failed.
pub async fn run(cli: Cli) -> Result<(), Error> {
    let telemetry = Telemetry::install(cli.role.telemetry(), cli.role.name())?;
    let served = run_role(cli.role, &telemetry).await;
    telemetry.shutdown().await;
    served
}

/// Runs `role` until it stops.
async fn run_role(role: Role, telemetry: &Telemetry) -> Result<(), Error> {
    match role {
        Role::Migrate(options) => migrate(&options.database, &options.migration).await,
        Role::Worker(options) => work(&options, telemetry).await,
        Role::Api(options) => {
            serve(Http {
                routes: Routes::Api,
                listen: options.listen.listen,
                database: &options.database,
                read_url: options.replica.postgres_read_url.as_deref(),
                migration: None,
                objects: &options.objects,
                landing: &options.landing,
                overload: &options.overload,
                timing: &options.timing,
                delivery_wait: options.delivery.directory_delivery_wait,
                serving: options.serving(),
                telemetry,
                backlog: None,
                registry: Some(&options.registry),
            })
            .await
        }
        Role::RunnerApi(options) => {
            serve(Http {
                routes: Routes::RunnerApi,
                listen: options.listen.listen,
                database: &options.database,
                read_url: options.replica.postgres_read_url.as_deref(),
                migration: None,
                objects: &options.objects,
                landing: &options.landing,
                overload: &options.overload,
                timing: &options.timing,
                delivery_wait: UNUSED_DELIVERY_WAIT,
                serving: options.serving(),
                telemetry,
                backlog: None,
                registry: None,
            })
            .await
        }
        Role::All(options) => {
            serve(Http {
                routes: Routes::All,
                listen: options.listen.listen,
                database: &options.database,
                read_url: options.replica.postgres_read_url.as_deref(),
                migration: Some(&options.migration),
                objects: &options.objects,
                landing: &options.landing,
                overload: &options.overload,
                timing: &options.timing,
                delivery_wait: options.delivery.directory_delivery_wait,
                serving: options.serving(),
                telemetry,
                backlog: Some(options.backlog.backlog_interval),
                registry: Some(&options.registry),
            })
            .await
        }
    }
}

/// Opens the pool `options` names.
async fn connect(options: &DatabaseOptions) -> Result<Database, Error> {
    postgres::open(options, None).await
}

/// Applies the migrations the database has not had.
async fn migrate(options: &DatabaseOptions, migration: &MigrationOptions) -> Result<(), Error> {
    postgres::open_for_migration(options, migration)
        .await?
        .migrate()
        .await
        .map_err(Error::Database)
}

/// Builds the directory outbox's relay, which delivers through `changes`.
fn relay(
    database: &Database,
    changes: Arc<dyn DirectoryChanges>,
    delivery_wait: u64,
) -> Arc<DirectoryRelay<ServerEnvironment>> {
    Arc::new(DirectoryRelay::new(
        Arc::new(PostgresDirectoryOutbox::new(database)),
        changes,
        Arc::new(ServerEnvironment),
        Duration::from_secs(delivery_wait),
    ))
}

/// Delivers the directory outbox, and answers the probes on the listening address.
async fn work(options: &WorkerOptions, telemetry: &Telemetry) -> Result<(), Error> {
    let delivery = select_delivery(&options.authzen, &options.directory)?;
    let database = connect(&options.database).await?;
    let changes: Arc<dyn DirectoryChanges> = match delivery {
        None => Arc::new(()),
        Some(delivery) => {
            let tokens = token_client(
                delivery.token_endpoint,
                options.authzen.authzen_audience.clone(),
                delivery.authentication,
                delivery.token_ca.as_ref(),
                options.timing.authzen_timeout(),
            )?;
            garam_changes(
                delivery.url,
                tokens,
                delivery.policy_ca.as_ref(),
                options.timing.authzen_timeout(),
            )?
        }
    };
    let relay = relay(&database, changes, UNUSED_DELIVERY_WAIT);
    let _watching = Telemetry::watch(
        database.clone(),
        Some((
            Arc::new(PostgresBacklog::new(&database)) as Arc<dyn Backlog>,
            Duration::from_secs(u64::from(options.backlog.backlog_interval.get())),
        )),
    );
    let readiness = Probe {
        directory: Arc::new(PostgresRepositoryDirectory::new(&database)),
        schema: Arc::new(database),
        objects: None,
    };
    let router = adapter_http::health_router(Arc::new(readiness))
        .merge(adapter_http::metrics_router(telemetry.scrape()));
    listen_and_serve(
        options.listen.listen,
        router,
        Some(relay),
        options.timing.shutdown_deadline(),
    )
    .await
}

/// Builds the adapters and use cases a role that serves routes needs and serves them.
async fn serve(http: Http<'_>) -> Result<(), Error> {
    let Http {
        routes,
        listen,
        database: database_options,
        read_url,
        migration,
        objects: object_options,
        landing,
        overload,
        timing,
        delivery_wait,
        serving,
        telemetry,
        backlog,
        registry,
    } = http;
    let registry = registry
        .map(RegistryOptions::configure)
        .transpose()
        .map_err(Error::Registry)?
        .flatten();
    let selection = serving.select()?;
    let administration = selection.administration;
    let identity = verifier(
        selection.identity,
        Duration::from_secs(serving.identity.jwks_min_refetch_seconds),
        overload.identity_timeout(),
    )?;
    if let Some(migration) = migration {
        migrate(database_options, migration).await?;
    }
    let database = postgres::open(database_options, read_url).await?;
    let _watching = Telemetry::watch(
        database.clone(),
        backlog.map(|interval| {
            (
                Arc::new(PostgresBacklog::new(&database)) as Arc<dyn Backlog>,
                Duration::from_secs(u64::from(interval.get())),
            )
        }),
    );
    let (authorisation, changes): Selected = match selection.authorisation {
        AuthorisationMode::Postgres => (
            Arc::new(Observed::new(
                "decision_point",
                PostgresAuthorisationDecision::new(&database),
            )),
            Arc::new(()),
        ),
        AuthorisationMode::Authzen(mode) => authzen(
            mode,
            serving.client.authzen_audience.clone(),
            serving.authorisation.authzen_evaluations,
            timing.authzen_timeout(),
        )?,
    };
    let relay = relay(&database, Arc::clone(&changes), delivery_wait);
    let store = || {
        S3ObjectStore::new(
            &object_options.s3_endpoint,
            &object_options.s3_region,
            &object_options.s3_bucket,
            &object_options.s3_access_key,
            &object_options.s3_secret_key,
            object_options.max_object_size,
            overload.object_timeouts(),
        )
        .map(|store| store.with_expiry(Duration::from_secs(object_options.grant_expiry_seconds)))
        .map_err(Error::ObjectStore)
    };
    // The readiness probe asks the store itself, not the observed one: a probe every few seconds
    // would be a trace of its own and skew the store's latency.
    let probed = Arc::new(store()?);
    let objects = Arc::new(Observed::new("object_store", store()?));
    let runner = serving.runner.unwrap_or(&UNUSED_RUNNER);
    let assembly = Assembly {
        objects: objects.clone(),
        log: Arc::new(PostgresChangesetLog::new(&database)),
        derived: Arc::new(PostgresDerivedData::new(&database)),
        drafts: Arc::new(PostgresDraftStore::new(&database)),
        directory: Arc::new(PostgresRepositoryDirectory::new(&database)),
        authorisation,
        membership: Arc::new(PostgresMembership::new(&database)),
        queue: Arc::new(PostgresJobQueue::new(&database)),
        cache_refs: Arc::new(PostgresCacheRefs::new(&database)),
        registry: Arc::new(PostgresRunnerRegistry::new(&database)),
        definitions: Arc::new(TomlCheckDefinitions::new().map_err(Error::Definitions)?),
        changes,
        delivery: relay.clone(),
        identity,

        administration,
        max_upload_size: object_options.max_object_size,
        attempts: LandingAttempts::new(landing.landing_attempts),
        lease: i64::from(runner.runner_lease.get()),
    };
    let state = ApiState {
        authentication: Arc::new(assembly.authentication()),
        repositories: Arc::new(assembly.repositories()),
        members: Arc::new(assembly.members()),
        source_control: Arc::new(assembly.source_control()),
        runners: Arc::new(assembly.runners()),
        jobs: Arc::new(assembly.jobs()),
        checks: Arc::new(assembly.checks()),
        runs: Arc::new(assembly.runs()),
        registry_token: registry.map(|registry| {
            Arc::new(
                assembly.registry_token(registry.issuance, registry.service, || {
                    ServerEnvironment.now()
                }),
            ) as Arc<dyn RegistryToken>
        }),
        settings: RunnerSettings {
            hold: Duration::from_secs(runner.runner_claim_hold),
            retry_interval: CLAIM_RETRY_INTERVAL,
            heartbeat_interval: Duration::from_secs(u64::from(
                runner.runner_heartbeat_interval.get(),
            )),
            pause,
        },
        readiness: Arc::new(Probe {
            directory: Arc::new(PostgresRepositoryDirectory::new(&database)),
            schema: Arc::new(database),
            objects: Some(probed),
        }),
        limits: overload.limits(),
    };
    let (router, delivering) = match routes {
        Routes::Api => (adapter_http::api_router(state), None),
        Routes::RunnerApi => (adapter_http::runner_router(state), None),
        Routes::All => (adapter_http::router(state), Some(relay)),
    };
    let router = router.merge(adapter_http::metrics_router(telemetry.scrape()));
    listen_and_serve(listen, router, delivering, timing.shutdown_deadline()).await
}

/// Binds `listen` and serves `router`, with `relay` delivering alongside where given, until
/// SIGTERM or SIGINT; then lets the requests in flight finish for up to `deadline` and returns,
/// dropping any still open.
async fn listen_and_serve(
    listen: SocketAddr,
    router: axum::Router,
    relay: Option<Arc<DirectoryRelay<ServerEnvironment>>>,
    deadline: Duration,
) -> Result<(), Error> {
    let listener = TcpListener::bind(listen)
        .await
        .map_err(|error| Error::Bind(listen, error))?;
    let address = listener.local_addr().map_err(Error::Serve)?;
    let mut terminate = signal(SignalKind::terminate()).map_err(Error::Serve)?;
    let mut interrupt = signal(SignalKind::interrupt()).map_err(Error::Serve)?;
    // Nowhere left to report a failed stderr write.
    let _ = writeln!(std::io::stderr(), "codehub listening on {address}");
    let delivering = relay.map(|relay| tokio::spawn(async move { relay.run().await }));
    let served = serve_until(
        listener,
        router,
        async move {
            tokio::select! {
                _ = terminate.recv() => {}
                _ = interrupt.recv() => {}
            }
        },
        deadline,
    )
    .await
    .map_err(Error::Serve);
    if let Some(delivering) = delivering {
        delivering.abort();
    }
    served
}

/// The longest a stopping server goes on accepting connections while it answers `/readyz` with
/// 503, so that a load balancer probing it takes it out of rotation before it stops accepting.
const DRAIN_NOTICE: Duration = Duration::from_secs(5);

/// Serves `router` on `listener` until `stop` completes; from then, for the fifth of `deadline`
/// but at most [`DRAIN_NOTICE`], answers `/readyz` with 503 and serves on, then stops accepting,
/// waits for the requests in flight and returns at `deadline` from `stop` at the latest, leaving
/// any still open to be dropped with the runtime (ADR 0055).
async fn serve_until(
    listener: TcpListener,
    router: axum::Router,
    stop: impl Future<Output = ()> + Send + 'static,
    deadline: Duration,
) -> std::io::Result<()> {
    let (stopped, stopping) = tokio::sync::oneshot::channel();
    let draining = Arc::new(AtomicBool::new(false));
    let router = router.layer(axum::middleware::from_fn_with_state(
        Arc::clone(&draining),
        not_ready_while_draining,
    ));
    let serving = axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            stop.await;
            draining.store(true, Ordering::Release);
            // The receiver outlives the server, which is waiting for this.
            let _ = stopped.send(());
            tokio::time::sleep((deadline / 5).min(DRAIN_NOTICE)).await;
        })
        .into_future();
    tokio::pin!(serving);
    tokio::select! {
        served = &mut serving => served,
        Ok(()) = stopping => {
            tokio::select! {
                served = &mut serving => served,
                () = tokio::time::sleep(deadline) => Ok(()),
            }
        }
    }
}

/// Answers `/readyz` with 503 once `draining`, so that a load balancer probing a process that is
/// stopping stops sending requests to it.
async fn not_ready_while_draining(
    axum::extract::State(draining): axum::extract::State<Arc<AtomicBool>>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    if request.uri().path() == "/readyz" && draining.load(Ordering::Acquire) {
        return (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "the server is shutting down",
        )
            .into_response();
    }
    next.run(request).await
}

#[cfg(test)]
mod tests;
