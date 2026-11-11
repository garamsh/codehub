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
use app::use_case::members::MembersService;
use app::use_case::repositories::RepositoriesService;
use app::use_case::source_control::SourceControlService;
use clap::{Parser, ValueEnum};
use tokio::net::TcpListener;
use tokio::signal::unix::{SignalKind, signal};

/// How the server verifies the token a caller presents.
#[derive(Clone, Copy, PartialEq, Eq, Debug, ValueEnum)]
pub enum Identity {
    /// An OpenID Connect JWT, verified offline against the issuer's keys.
    Oidc,
    /// RFC 7662 introspection, asking the issuer on every request.
    Introspection,
}

/// Who decides what a caller may do, and so who administers members.
#[derive(Clone, Copy, PartialEq, Eq, Debug, ValueEnum)]
pub enum Authorisation {
    /// Grants kept and evaluated in PostgreSQL; codehub administers members.
    Postgres,
    /// An AuthZEN 1.0 decision point; members are administered outside codehub (ADR 0030).
    Authzen,
}

/// The server's settings. Each is a flag and has an environment variable of the same name,
/// upper-cased, with the prefix `CODEHUB_`.
#[derive(Parser)]
#[command(version, about = "The codehub server")]
pub struct Config {
    /// The address to listen on; port 0 takes any free port.
    #[arg(long, env = "CODEHUB_LISTEN", default_value = "127.0.0.1:8080")]
    pub listen: SocketAddr,
    /// The URL of the PostgreSQL database holding the metadata.
    #[arg(long, env = "CODEHUB_POSTGRES_URL")]
    pub postgres_url: String,
    /// The most connections held open to PostgreSQL.
    #[arg(long, env = "CODEHUB_POSTGRES_POOL_SIZE", default_value_t = 10)]
    pub postgres_pool_size: u32,
    /// The URL of the S3-compatible object store.
    #[arg(long, env = "CODEHUB_S3_ENDPOINT")]
    pub s3_endpoint: String,
    /// The object store's region.
    #[arg(long, env = "CODEHUB_S3_REGION")]
    pub s3_region: String,
    /// The bucket that holds the objects.
    #[arg(long, env = "CODEHUB_S3_BUCKET")]
    pub s3_bucket: String,
    /// The access key the object store is reached with.
    #[arg(long, env = "CODEHUB_S3_ACCESS_KEY")]
    pub s3_access_key: String,
    /// The secret key the object store is reached with.
    #[arg(long, env = "CODEHUB_S3_SECRET_KEY", hide_env_values = true)]
    pub s3_secret_key: String,
    /// The largest object a client may upload, in bytes.
    #[arg(long, env = "CODEHUB_MAX_OBJECT_SIZE", default_value_t = 1024 * 1024 * 1024)]
    pub max_object_size: u64,
    /// How long an upload or download grant lasts, in seconds.
    #[arg(long, env = "CODEHUB_GRANT_EXPIRY_SECONDS", default_value_t = 900)]
    pub grant_expiry_seconds: u64,
    /// How callers are verified: an OIDC JWT offline, or RFC 7662 introspection.
    #[arg(long, env = "CODEHUB_IDENTITY", value_enum, default_value_t = Identity::Oidc)]
    pub identity: Identity,
    /// The issuer of the tokens the server accepts; required with `--identity oidc`.
    #[arg(long, env = "CODEHUB_OIDC_ISSUER")]
    pub oidc_issuer: Option<String>,
    /// The audience the accepted tokens are issued for; required with `--identity oidc`.
    #[arg(long, env = "CODEHUB_OIDC_AUDIENCE")]
    pub oidc_audience: Option<String>,
    /// The shortest time between two fetches of the issuer's key set, in seconds.
    #[arg(long, env = "CODEHUB_JWKS_MIN_REFETCH_SECONDS", default_value_t = 60)]
    pub jwks_min_refetch_seconds: u64,
    /// The introspection endpoint of the issuer; required with `--identity introspection`.
    #[arg(long, env = "CODEHUB_INTROSPECTION_ENDPOINT")]
    pub introspection_endpoint: Option<String>,
    /// The client id the server authenticates to the introspection endpoint with; required with
    /// `--identity introspection`.
    #[arg(long, env = "CODEHUB_INTROSPECTION_CLIENT_ID")]
    pub introspection_client_id: Option<String>,
    /// The client secret the server authenticates to the introspection endpoint with; required
    /// with `--identity introspection`.
    #[arg(
        long,
        env = "CODEHUB_INTROSPECTION_CLIENT_SECRET",
        hide_env_values = true
    )]
    pub introspection_client_secret: Option<String>,
    /// The issuer named where an introspection response has no `iss`; required with
    /// `--identity introspection`.
    #[arg(long, env = "CODEHUB_INTROSPECTION_ISSUER")]
    pub introspection_issuer: Option<String>,
    /// The audience the accepted tokens are issued for; required with
    /// `--identity introspection`.
    #[arg(long, env = "CODEHUB_INTROSPECTION_AUDIENCE")]
    pub introspection_audience: Option<String>,
    /// How callers are authorised: grants in PostgreSQL, or an AuthZEN decision point in the
    /// external administration mode.
    #[arg(long, env = "CODEHUB_AUTHORISATION", value_enum, default_value_t = Authorisation::Postgres)]
    pub authorisation: Authorisation,
    /// The base URL of the AuthZEN decision point; required with `--authorisation authzen`.
    #[arg(long, env = "CODEHUB_AUTHZEN_URL")]
    pub authzen_url: Option<String>,
    /// The token endpoint the decision point's bearer token is obtained from by client
    /// credentials; required with `--authorisation authzen`.
    #[arg(long, env = "CODEHUB_AUTHZEN_TOKEN_ENDPOINT")]
    pub authzen_token_endpoint: Option<String>,
    /// The client id used at that token endpoint; required with `--authorisation authzen`.
    #[arg(long, env = "CODEHUB_AUTHZEN_CLIENT_ID")]
    pub authzen_client_id: Option<String>,
    /// The client secret used at that token endpoint; required with `--authorisation authzen`.
    #[arg(long, env = "CODEHUB_AUTHZEN_CLIENT_SECRET", hide_env_values = true)]
    pub authzen_client_secret: Option<String>,
}

/// Prints every field but the secrets, which print as `<redacted>`. The impl destructures
/// `Config` without `..`, so a field added later does not compile until it is listed here, and
/// listing it is the moment to decide whether it is a secret.
impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self {
            listen,
            postgres_url,
            postgres_pool_size,
            s3_endpoint,
            s3_region,
            s3_bucket,
            s3_access_key,
            s3_secret_key: _,
            max_object_size,
            grant_expiry_seconds,
            identity,
            oidc_issuer,
            oidc_audience,
            jwks_min_refetch_seconds,
            introspection_endpoint,
            introspection_client_id,
            introspection_client_secret,
            introspection_issuer,
            introspection_audience,
            authorisation,
            authzen_url,
            authzen_token_endpoint,
            authzen_client_id,
            authzen_client_secret,
        } = self;
        f.debug_struct("Config")
            .field("listen", listen)
            .field("postgres_url", &redact_url(postgres_url))
            .field("postgres_pool_size", postgres_pool_size)
            .field("s3_endpoint", s3_endpoint)
            .field("s3_region", s3_region)
            .field("s3_bucket", s3_bucket)
            .field("s3_access_key", s3_access_key)
            .field("s3_secret_key", &REDACTED)
            .field("max_object_size", max_object_size)
            .field("grant_expiry_seconds", grant_expiry_seconds)
            .field("identity", identity)
            .field("oidc_issuer", oidc_issuer)
            .field("oidc_audience", oidc_audience)
            .field("jwks_min_refetch_seconds", jwks_min_refetch_seconds)
            .field("introspection_endpoint", introspection_endpoint)
            .field("introspection_client_id", introspection_client_id)
            .field(
                "introspection_client_secret",
                &introspection_client_secret.as_ref().map(|_| REDACTED),
            )
            .field("introspection_issuer", introspection_issuer)
            .field("introspection_audience", introspection_audience)
            .field("authorisation", authorisation)
            .field("authzen_url", authzen_url)
            .field("authzen_token_endpoint", authzen_token_endpoint)
            .field("authzen_client_id", authzen_client_id)
            .field(
                "authzen_client_secret",
                &authzen_client_secret.as_ref().map(|_| REDACTED),
            )
            .finish()
    }
}

/// What stands where a secret would print.
const REDACTED: &str = "<redacted>";

/// The URL with the password of its userinfo, and any `password` query parameter, replaced by
/// `<redacted>`. A string that is not `scheme://authority...` is returned as `<redacted>` whole,
/// since it cannot be told where a secret sits in it.
fn redact_url(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return REDACTED.to_owned();
    };
    // The userinfo is everything up to the last `@`, so a password holding an unencoded `#`, `?`
    // or `/` is still inside it; the query and fragment are looked for only after the host.
    let (userinfo, rest) = match rest.rsplit_once('@') {
        Some((userinfo, rest)) => (format!("{}@", redact_userinfo(userinfo)), rest),
        None => (String::new(), rest),
    };
    let (location, fragment) = match rest.split_once('#') {
        Some((location, fragment)) => (location, Some(fragment)),
        None => (rest, None),
    };
    let (location, query) = match location.split_once('?') {
        Some((location, query)) => (location, Some(query)),
        None => (location, None),
    };
    let mut redacted = format!("{scheme}://{userinfo}{location}");
    if let Some(query) = query {
        let parameters: Vec<String> = query
            .split('&')
            .map(|parameter| match parameter.split_once('=') {
                Some((name, _)) if name.eq_ignore_ascii_case("password") => {
                    format!("{name}={REDACTED}")
                }
                _ => parameter.to_owned(),
            })
            .collect();
        redacted.push('?');
        redacted.push_str(&parameters.join("&"));
    }
    if let Some(fragment) = fragment {
        redacted.push('#');
        redacted.push_str(fragment);
    }
    redacted
}

/// The userinfo with everything after its first `:` replaced by `<redacted>`.
fn redact_userinfo(userinfo: &str) -> String {
    match userinfo.split_once(':') {
        Some((user, _)) => format!("{user}:{REDACTED}"),
        None => userinfo.to_owned(),
    }
}

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

fn require<'a>(value: &'a Option<String>, setting: &'static str) -> Result<&'a str, Error> {
    value.as_deref().ok_or(Error::MissingSetting(setting))
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
            &adapters.directory,
            &adapters.authorisation,
            &adapters.membership,
            &(),
            administration,
        ),
        MembersService::new(
            &adapters.directory,
            &adapters.authorisation,
            &adapters.membership,
            administration,
        ),
        SourceControlService::new(
            &adapters.objects,
            &adapters.log,
            &adapters.derived,
            &adapters.drafts,
            &adapters.directory,
            &adapters.authorisation,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn config(postgres_url: &str) -> Config {
        Config {
            listen: SocketAddr::from(([127, 0, 0, 1], 8080)),
            postgres_url: postgres_url.to_owned(),
            postgres_pool_size: 10,
            s3_endpoint: "http://objects.example:9000".to_owned(),
            s3_region: "eu-test-1".to_owned(),
            s3_bucket: "bucket-visible".to_owned(),
            s3_access_key: "access-visible".to_owned(),
            s3_secret_key: "s3-secret-value".to_owned(),
            max_object_size: 1,
            grant_expiry_seconds: 1,
            identity: Identity::Introspection,
            oidc_issuer: Some("http://issuer.example".to_owned()),
            oidc_audience: Some("audience-visible".to_owned()),
            jwks_min_refetch_seconds: 1,
            introspection_endpoint: Some("http://issuer.example/introspect".to_owned()),
            introspection_client_id: Some("introspection-client-visible".to_owned()),
            introspection_client_secret: Some("introspection-secret-value".to_owned()),
            introspection_issuer: Some("http://issuer.example".to_owned()),
            introspection_audience: Some("introspection-audience-visible".to_owned()),
            authorisation: Authorisation::Authzen,
            authzen_url: Some("http://pdp.example".to_owned()),
            authzen_token_endpoint: Some("http://issuer.example/token".to_owned()),
            authzen_client_id: Some("authzen-client-visible".to_owned()),
            authzen_client_secret: Some("authzen-secret-value".to_owned()),
        }
    }

    #[test]
    fn debug_output_hides_the_secrets_and_keeps_the_rest() {
        let shown = format!(
            "{:?}",
            config("postgres://dbuser:pg-secret-value@db.example:5432/codehub")
        );
        assert!(!shown.contains("s3-secret-value"), "{shown}");
        assert!(!shown.contains("pg-secret-value"), "{shown}");
        assert!(!shown.contains("introspection-secret-value"), "{shown}");
        assert!(!shown.contains("authzen-secret-value"), "{shown}");
        assert!(shown.contains("introspection-client-visible"), "{shown}");
        assert!(shown.contains("authzen-client-visible"), "{shown}");
        assert!(shown.contains("bucket-visible"), "{shown}");
        assert!(shown.contains("access-visible"), "{shown}");
        assert!(
            shown.contains("postgres://dbuser:<redacted>@db.example:5432/codehub"),
            "{shown}"
        );
    }

    #[test]
    fn redacts_a_password_given_as_a_query_parameter() {
        let shown = redact_url("postgres://db.example/codehub?sslmode=require&password=qp-secret");
        assert!(!shown.contains("qp-secret"), "{shown}");
        assert_eq!(
            shown,
            "postgres://db.example/codehub?sslmode=require&password=<redacted>"
        );
    }

    #[test]
    fn redacts_a_password_holding_an_unencoded_delimiter() {
        assert_eq!(
            redact_url("postgres://u:p#w@h/db"),
            "postgres://u:<redacted>@h/db"
        );
        assert_eq!(
            redact_url("postgres://u:p/w@h/db"),
            "postgres://u:<redacted>@h/db"
        );
        assert_eq!(
            redact_url("postgres://u:p?w@h/db"),
            "postgres://u:<redacted>@h/db"
        );
    }

    #[test]
    fn keeps_a_url_without_a_password_and_redacts_one_that_does_not_parse() {
        assert_eq!(
            redact_url("postgres://dbuser@db.example/codehub"),
            "postgres://dbuser@db.example/codehub"
        );
        assert_eq!(redact_url("not a url with pw-secret"), "<redacted>");
    }
}
