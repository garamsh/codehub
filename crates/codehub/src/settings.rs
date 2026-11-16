//! The server's settings: the flags and variables of each role, the checks that a mode's settings
//! are given and go together, and the redaction of secrets where they are printed.
//!
//! A role takes the groups of settings it uses and no others, so a flag a role does not use is
//! refused by the parser and absent from its `--help` (ADR 0052).

use std::ffi::OsString;
use std::fmt;
use std::net::SocketAddr;
use std::num::NonZeroU32;
use std::path::PathBuf;

use app::use_case::administration::Administration;
use app::use_case::landing_attempts::LandingAttempts;
use clap::{Args, Parser, Subcommand, ValueEnum};
use oauth_client::Authentication;

use crate::Error;
use crate::overload::{OverloadOptions, TimingOptions};

/// The default of `--runner-claim-hold`, in seconds.
const DEFAULT_CLAIM_HOLD: u64 = 30;

/// The default of `--runner-heartbeat-interval`, in seconds.
const DEFAULT_HEARTBEAT_INTERVAL: NonZeroU32 = NonZeroU32::MIN.saturating_add(4);

/// The default of `--runner-lease`, in seconds.
const DEFAULT_LEASE: NonZeroU32 = NonZeroU32::MIN.saturating_add(29);

/// The default of `--directory-delivery-wait`, in seconds.
const DEFAULT_DELIVERY_WAIT: u64 = 2;

/// The runner settings of a role that serves no runner: its values are never read.
pub(crate) const UNUSED_RUNNER: RunnerOptions = RunnerOptions {
    runner_claim_hold: DEFAULT_CLAIM_HOLD,
    runner_heartbeat_interval: DEFAULT_HEARTBEAT_INTERVAL,
    runner_lease: DEFAULT_LEASE,
};

/// The creation wait of a role that answers no creation: its value is never read.
pub(crate) const UNUSED_DELIVERY_WAIT: u64 = DEFAULT_DELIVERY_WAIT;

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

/// What the AuthZEN adapter sends: the names of its decision point (ADR 0043).
#[derive(Clone, Copy, PartialEq, Eq, Debug, ValueEnum)]
pub enum AuthzenProfile {
    /// codehub's own names for subjects, actions and resources.
    Plain,
    /// garam's names: GRNs for subjects and resources, `codehub:` actions.
    Grn,
}

/// How the AuthZEN adapter authenticates to the token endpoint (ADR 0042).
#[derive(Clone, Copy, PartialEq, Eq, Debug, ValueEnum)]
pub enum AuthzenClientAuthentication {
    /// A client id and secret, one token for every organisation.
    ClientSecret,
    /// A signed assertion (`private_key_jwt`), one client and token per organisation.
    PrivateKeyJwt,
}

/// Where repository and organisation changes are delivered (ADR 0044).
#[derive(Clone, Copy, PartialEq, Eq, Debug, ValueEnum)]
pub enum DirectoryChangesMode {
    /// Nowhere: the outbox is still written, and its entries are discarded as they come due.
    None,
    /// garam's `ApplyEntityChanges`, with the organisation's token.
    Garam,
}

/// The server: one binary, one role per process (ADR 0052). A command line that names no role is
/// the `all` role, so `codehub` alone runs as it always did.
#[derive(Parser, Debug)]
#[command(version, about = "The codehub server")]
pub struct Cli {
    /// The role this process runs.
    #[command(subcommand)]
    pub role: Role,
}

impl Cli {
    /// Parses the command line `args` holds, the first being the program's name, taking the role
    /// `all` where none is named.
    ///
    /// # Errors
    ///
    /// The parser's error, which prints as its usage message.
    pub fn try_from_args<I>(args: I) -> Result<Self, clap::Error>
    where
        I: IntoIterator<Item = OsString>,
    {
        Self::try_parse_from(with_default_role(args))
    }

    /// Parses the process's command line, printing the message and exiting where it is refused,
    /// and where help or the version is asked for.
    #[must_use]
    pub fn from_args() -> Self {
        Self::try_from_args(std::env::args_os()).unwrap_or_else(|error| error.exit())
    }
}

/// `args` with the role `all` inserted after the program's name where the next argument is an
/// option other than the help and version flags, or there is none.
fn with_default_role<I>(args: I) -> Vec<OsString>
where
    I: IntoIterator<Item = OsString>,
{
    let mut args: Vec<_> = args.into_iter().collect();
    let names_no_role = args.get(1).is_none_or(|first| {
        let first = first.to_string_lossy();
        first.starts_with('-') && !matches!(&*first, "-h" | "--help" | "-V" | "--version")
    });
    if names_no_role && !args.is_empty() {
        args.insert(1, "all".into());
    }
    args
}

/// What one process of the server does.
#[derive(Subcommand, Debug)]
pub enum Role {
    /// Serve the `/v1` user and agent API, except `/v1/runner/`.
    Api(ApiOptions),
    /// Serve `/v1/runner/`: claim, heartbeat, result and uploads.
    RunnerApi(RunnerApiOptions),
    /// Run the background work: deliver the directory outbox.
    Worker(WorkerOptions),
    /// Apply the database migrations and exit.
    Migrate(MigrateOptions),
    /// Migrate, then run every role in this process.
    All(AllOptions),
}

/// The options of `api`.
#[derive(Args, Debug)]
pub struct ApiOptions {
    #[command(flatten)]
    pub(crate) listen: ListenOptions,
    #[command(flatten)]
    pub(crate) database: DatabaseOptions,
    #[command(flatten)]
    pub(crate) objects: ObjectOptions,
    #[command(flatten)]
    pub(crate) identity: IdentityOptions,
    #[command(flatten)]
    pub(crate) authorisation: AuthorisationOptions,
    #[command(flatten)]
    pub(crate) authzen: AuthzenClientOptions,
    #[command(flatten)]
    pub(crate) landing: LandingOptions,
    #[command(flatten)]
    pub(crate) directory: DirectoryOptions,
    #[command(flatten)]
    pub(crate) delivery: DeliveryOptions,
    #[command(flatten)]
    pub(crate) overload: OverloadOptions,
    #[command(flatten)]
    pub(crate) timing: TimingOptions,
}

/// The options of `runner-api`.
#[derive(Args, Debug)]
pub struct RunnerApiOptions {
    #[command(flatten)]
    pub(crate) listen: ListenOptions,
    #[command(flatten)]
    pub(crate) database: DatabaseOptions,
    #[command(flatten)]
    pub(crate) objects: ObjectOptions,
    #[command(flatten)]
    pub(crate) identity: IdentityOptions,
    #[command(flatten)]
    pub(crate) authorisation: AuthorisationOptions,
    #[command(flatten)]
    pub(crate) authzen: AuthzenClientOptions,
    #[command(flatten)]
    pub(crate) landing: LandingOptions,
    #[command(flatten)]
    pub(crate) runner: RunnerOptions,
    #[command(flatten)]
    pub(crate) overload: OverloadOptions,
    #[command(flatten)]
    pub(crate) timing: TimingOptions,
}

/// The options of `worker`.
#[derive(Args, Debug)]
pub struct WorkerOptions {
    #[command(flatten)]
    pub(crate) listen: ListenOptions,
    #[command(flatten)]
    pub(crate) database: DatabaseOptions,
    #[command(flatten)]
    pub(crate) authzen: AuthzenClientOptions,
    #[command(flatten)]
    pub(crate) directory: DirectoryOptions,
    #[command(flatten)]
    pub(crate) timing: TimingOptions,
}

/// The options of `migrate`.
#[derive(Args, Debug)]
pub struct MigrateOptions {
    #[command(flatten)]
    pub(crate) database: DatabaseOptions,
}

/// The options of `all`, which is every option there is.
#[derive(Args, Debug)]
pub struct AllOptions {
    #[command(flatten)]
    pub(crate) listen: ListenOptions,
    #[command(flatten)]
    pub(crate) database: DatabaseOptions,
    #[command(flatten)]
    pub(crate) objects: ObjectOptions,
    #[command(flatten)]
    pub(crate) identity: IdentityOptions,
    #[command(flatten)]
    pub(crate) authorisation: AuthorisationOptions,
    #[command(flatten)]
    pub(crate) authzen: AuthzenClientOptions,
    #[command(flatten)]
    pub(crate) landing: LandingOptions,
    #[command(flatten)]
    pub(crate) runner: RunnerOptions,
    #[command(flatten)]
    pub(crate) directory: DirectoryOptions,
    #[command(flatten)]
    pub(crate) delivery: DeliveryOptions,
    #[command(flatten)]
    pub(crate) overload: OverloadOptions,
    #[command(flatten)]
    pub(crate) timing: TimingOptions,
}

/// Where a serving role listens.
#[derive(Args, Debug)]
pub struct ListenOptions {
    /// The address to listen on; port 0 takes any free port.
    #[arg(long, env = "CODEHUB_LISTEN", default_value = "127.0.0.1:8080")]
    pub listen: SocketAddr,
}

/// The database every role reaches.
#[derive(Args)]
pub struct DatabaseOptions {
    /// The URL of the PostgreSQL database holding the metadata.
    #[arg(long, env = "CODEHUB_POSTGRES_URL", hide_env_values = true)]
    pub postgres_url: String,
    /// The most connections this server process holds open to PostgreSQL. The sum over all server
    /// processes must stay below PostgreSQL's `max_connections`, less the connections other
    /// clients use (ADR 0037).
    #[arg(long, env = "CODEHUB_POSTGRES_POOL_SIZE", default_value_t = 40)]
    pub postgres_pool_size: u32,
}

/// The object store, and what a client may put in it.
#[derive(Args)]
pub struct ObjectOptions {
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
}

/// How callers are verified.
#[derive(Args)]
pub struct IdentityOptions {
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
}

/// Who decides what a caller may do.
#[derive(Args, Debug)]
pub struct AuthorisationOptions {
    /// How callers are authorised: grants in PostgreSQL, or an AuthZEN decision point in the
    /// external administration mode.
    #[arg(long, env = "CODEHUB_AUTHORISATION", value_enum, default_value_t = Authorisation::Postgres)]
    pub authorisation: Authorisation,
    /// The base URL of the AuthZEN decision point; required with `--authorisation authzen`.
    #[arg(long, env = "CODEHUB_AUTHZEN_URL")]
    pub authzen_url: Option<String>,
    /// Sends one AuthZEN Access Evaluations request for many resources where the decision point
    /// advertises it, instead of one evaluation per resource. Turn it on only where the decision
    /// point decides each evaluation by its own resource properties (ADR 0041).
    #[arg(long, env = "CODEHUB_AUTHZEN_EVALUATIONS")]
    pub authzen_evaluations: bool,
}

/// The client that gets the AuthZEN decision point's token, which delivering changes to garam
/// shares (ADR 0043).
#[derive(Args)]
pub struct AuthzenClientOptions {
    /// The token endpoint the decision point's bearer token is obtained from by client
    /// credentials; required with `--authorisation authzen`.
    #[arg(long, env = "CODEHUB_AUTHZEN_TOKEN_ENDPOINT")]
    pub authzen_token_endpoint: Option<String>,
    /// The client id used at that token endpoint; required with `--authorisation authzen`. With
    /// `--authzen-client-authentication private-key-jwt` it is a template holding
    /// `{organisation}`, such as `grn:{organisation}/iam/service/codehub`.
    #[arg(long, env = "CODEHUB_AUTHZEN_CLIENT_ID")]
    pub authzen_client_id: Option<String>,
    /// The client secret used at that token endpoint; required with
    /// `--authzen-client-authentication client-secret`.
    #[arg(long, env = "CODEHUB_AUTHZEN_CLIENT_SECRET", hide_env_values = true)]
    pub authzen_client_secret: Option<String>,
    /// The names the AuthZEN adapter sends: codehub's own (`plain`) or garam's (`grn`, which needs
    /// `--authorisation authzen` and `--authzen-client-authentication private-key-jwt`).
    #[arg(long, env = "CODEHUB_AUTHZEN_PROFILE", value_enum, default_value_t = AuthzenProfile::Plain)]
    pub authzen_profile: AuthzenProfile,
    /// How the adapter authenticates to the token endpoint: by client secret, or by a signed
    /// assertion of one client per organisation (`private-key-jwt`).
    #[arg(long, env = "CODEHUB_AUTHZEN_CLIENT_AUTHENTICATION", value_enum, default_value_t = AuthzenClientAuthentication::ClientSecret)]
    pub authzen_client_authentication: AuthzenClientAuthentication,
    /// The path of the PEM PKCS#8 private key that signs the assertions; required with
    /// `--authzen-client-authentication private-key-jwt`. The key is read from the file, never
    /// from a setting's value.
    #[arg(long, env = "CODEHUB_AUTHZEN_KEY_FILE")]
    pub authzen_key_file: Option<PathBuf>,
    /// The `kid` header of the assertions, such as the key's RFC 7638 thumbprint; optional.
    #[arg(long, env = "CODEHUB_AUTHZEN_KEY_ID")]
    pub authzen_key_id: Option<String>,
    /// The issuer identifier of the token endpoint's authorization server, the assertions'
    /// audience; required with `--authzen-client-authentication private-key-jwt`.
    #[arg(long, env = "CODEHUB_AUTHZEN_ISSUER")]
    pub authzen_issuer: Option<String>,
    /// Sent as the token request's `audience` parameter where given.
    #[arg(long, env = "CODEHUB_AUTHZEN_AUDIENCE")]
    pub authzen_audience: Option<String>,
}

/// How a landing is retried.
#[derive(Args, Debug)]
pub struct LandingOptions {
    /// The most times a landing is tried where `main` keeps moving under it, at least one; when
    /// they are spent the landing is answered as moved (ADR 0036).
    #[arg(long, env = "CODEHUB_LANDING_ATTEMPTS", default_value_t = LandingAttempts::DEFAULT)]
    pub landing_attempts: NonZeroU32,
}

/// How runners claim jobs.
#[derive(Args, Debug)]
pub struct RunnerOptions {
    /// How long a runner's claim is held waiting for a job before it is answered with none, in
    /// seconds.
    #[arg(long, env = "CODEHUB_RUNNER_CLAIM_HOLD", default_value_t = DEFAULT_CLAIM_HOLD)]
    pub runner_claim_hold: u64,
    /// How often a runner is told to heartbeat, in seconds, at least one.
    #[arg(long, env = "CODEHUB_RUNNER_HEARTBEAT_INTERVAL", default_value_t = DEFAULT_HEARTBEAT_INTERVAL)]
    pub runner_heartbeat_interval: NonZeroU32,
    /// How long a claim or a heartbeat holds a job for its runner, in seconds; it must exceed the
    /// heartbeat interval, or a runner loses its job between two heartbeats.
    #[arg(long, env = "CODEHUB_RUNNER_LEASE", default_value_t = DEFAULT_LEASE)]
    pub runner_lease: NonZeroU32,
}

/// Where repository and organisation changes are delivered.
#[derive(Args, Debug)]
pub struct DirectoryOptions {
    /// Where repository and organisation changes are delivered: nowhere (`none`) or to garam's
    /// `ApplyEntityChanges` (`garam`, which needs `--authzen-profile grn`). Every repository change
    /// is written to the outbox either way (ADR 0044).
    #[arg(long, env = "CODEHUB_DIRECTORY_CHANGES", value_enum, default_value_t = DirectoryChangesMode::None)]
    pub directory_changes: DirectoryChangesMode,
    /// The base URL of garam's policy service, which the entity changes are sent to; required with
    /// `--directory-changes garam`.
    #[arg(long, env = "CODEHUB_DIRECTORY_GARAM_URL")]
    pub directory_garam_url: Option<String>,
}

/// How long answering a creation waits for its change.
#[derive(Args, Debug)]
pub struct DeliveryOptions {
    /// How long creating a repository waits for its change to be delivered, in seconds. The
    /// creation is answered once it elapses whether or not the change was delivered.
    #[arg(long, env = "CODEHUB_DIRECTORY_DELIVERY_WAIT", default_value_t = DEFAULT_DELIVERY_WAIT, value_parser = clap::value_parser!(u64).range(..=3600))]
    pub directory_delivery_wait: u64,
}

/// Each impl below destructures its struct without `..`, so a field added later does not compile
/// until it is listed, and listing it is the moment to decide whether it is a secret. Secrets
/// print as `<redacted>`.
impl fmt::Debug for DatabaseOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self {
            postgres_url,
            postgres_pool_size,
        } = self;
        f.debug_struct("DatabaseOptions")
            .field("postgres_url", &redact_url(postgres_url))
            .field("postgres_pool_size", postgres_pool_size)
            .finish()
    }
}

impl fmt::Debug for ObjectOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self {
            s3_endpoint,
            s3_region,
            s3_bucket,
            s3_access_key,
            s3_secret_key: _,
            max_object_size,
            grant_expiry_seconds,
        } = self;
        f.debug_struct("ObjectOptions")
            .field("s3_endpoint", s3_endpoint)
            .field("s3_region", s3_region)
            .field("s3_bucket", s3_bucket)
            .field("s3_access_key", s3_access_key)
            .field("s3_secret_key", &REDACTED)
            .field("max_object_size", max_object_size)
            .field("grant_expiry_seconds", grant_expiry_seconds)
            .finish()
    }
}

impl fmt::Debug for IdentityOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self {
            identity,
            oidc_issuer,
            oidc_audience,
            jwks_min_refetch_seconds,
            introspection_endpoint,
            introspection_client_id,
            introspection_client_secret,
            introspection_issuer,
            introspection_audience,
        } = self;
        f.debug_struct("IdentityOptions")
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
            .finish()
    }
}

impl fmt::Debug for AuthzenClientOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self {
            authzen_token_endpoint,
            authzen_client_id,
            authzen_client_secret,
            authzen_profile,
            authzen_client_authentication,
            authzen_key_file,
            authzen_key_id,
            authzen_issuer,
            authzen_audience,
        } = self;
        f.debug_struct("AuthzenClientOptions")
            .field("authzen_token_endpoint", authzen_token_endpoint)
            .field("authzen_client_id", authzen_client_id)
            .field(
                "authzen_client_secret",
                &authzen_client_secret.as_ref().map(|_| REDACTED),
            )
            .field("authzen_profile", authzen_profile)
            .field(
                "authzen_client_authentication",
                authzen_client_authentication,
            )
            .field("authzen_key_file", authzen_key_file)
            .field("authzen_key_id", authzen_key_id)
            .field("authzen_issuer", authzen_issuer)
            .field("authzen_audience", authzen_audience)
            .finish()
    }
}

/// The settings of a serving role after every check: what each mode needs, given, and the modes
/// that go together. The composition site builds adapters from it and decides nothing more.
pub(crate) struct Selection<'a> {
    /// The identity verification to build.
    pub(crate) identity: IdentityMode<'a>,
    /// The authorisation decision to build.
    pub(crate) authorisation: AuthorisationMode<'a>,
    /// Who administers members, which the authorisation decision selects.
    pub(crate) administration: Administration,
}

/// The identity verification selected, with the settings it needs.
pub(crate) enum IdentityMode<'a> {
    /// An OpenID Connect JWT, verified offline.
    Oidc { issuer: &'a str, audience: &'a str },
    /// RFC 7662 introspection.
    Introspection {
        endpoint: &'a str,
        client_id: &'a str,
        client_secret: &'a str,
        issuer: &'a str,
        audience: &'a str,
    },
}

/// The authorisation decision selected, with the settings it needs.
pub(crate) enum AuthorisationMode<'a> {
    /// Grants kept in PostgreSQL; repository changes go nowhere.
    Postgres,
    /// An AuthZEN decision point.
    Authzen(AuthzenMode<'a>),
}

/// The AuthZEN decision point and the changes that share its token client.
pub(crate) struct AuthzenMode<'a> {
    pub(crate) url: &'a str,
    pub(crate) token_endpoint: &'a str,
    /// How the token client authenticates, the key file already read.
    pub(crate) authentication: Authentication,
    pub(crate) profile: AuthzenProfile,
    /// The base URL of garam's policy service where changes are delivered to garam, none where
    /// they go nowhere.
    pub(crate) garam_url: Option<&'a str>,
}

/// Delivering changes to garam, for the role that delivers and decides nothing.
pub(crate) struct GaramDelivery<'a> {
    pub(crate) token_endpoint: &'a str,
    /// How the token client authenticates, the key file already read.
    pub(crate) authentication: Authentication,
    pub(crate) url: &'a str,
}

/// The groups of settings a role that serves callers is checked over.
pub(crate) struct Serving<'a> {
    pub(crate) identity: &'a IdentityOptions,
    pub(crate) authorisation: &'a AuthorisationOptions,
    pub(crate) client: &'a AuthzenClientOptions,
    /// Where changes are delivered, none for a role that delivers none.
    pub(crate) directory: Option<&'a DirectoryOptions>,
    /// The runner settings, none for a role that serves no runner.
    pub(crate) runner: Option<&'a RunnerOptions>,
}

impl<'a> Serving<'a> {
    /// Checks the settings against each other and against the modes selected, and reads the
    /// AuthZEN key file where the selected authentication names one.
    ///
    /// The `grn` profile speaks to a decision point that binds each token to an organisation
    /// (ADR 0043), so it needs the AuthZEN adapter and `private-key-jwt` (ADR 0042), and so does
    /// delivering changes to garam.
    ///
    /// # Errors
    ///
    /// [`Error::RunnerLease`] where the lease does not exceed the heartbeat interval,
    /// [`Error::MissingSetting`] where a selected mode lacks a setting,
    /// [`Error::IncompatibleSettings`] where settings cannot go together and
    /// [`Error::KeyFile`] where the key file cannot be read.
    pub(crate) fn select(&self) -> Result<Selection<'a>, Error> {
        if let Some(runner) = self.runner {
            check_runner_lease(runner)?;
        }
        let identity = select_identity(self.identity)?;
        let (authorisation, administration) = match self.authorisation.authorisation {
            Authorisation::Postgres => {
                if self.client.authzen_profile == AuthzenProfile::Grn {
                    return Err(Error::IncompatibleSettings(
                        "CODEHUB_AUTHZEN_PROFILE=grn needs CODEHUB_AUTHORISATION=authzen",
                    ));
                }
                if self.directory.is_some_and(|directory| {
                    directory.directory_changes == DirectoryChangesMode::Garam
                }) {
                    return Err(garam_needs_grn());
                }
                (AuthorisationMode::Postgres, Administration::BuiltIn)
            }
            Authorisation::Authzen => (
                AuthorisationMode::Authzen(self.select_authzen()?),
                Administration::External,
            ),
        };
        Ok(Selection {
            identity,
            authorisation,
            administration,
        })
    }

    fn select_authzen(&self) -> Result<AuthzenMode<'a>, Error> {
        let url = require(
            &self.authorisation.authzen_url,
            "--authzen-url (CODEHUB_AUTHZEN_URL)",
        )?;
        let (token_endpoint, authentication) = select_client(self.client)?;
        let garam_url = match self.directory {
            Some(directory) if directory.directory_changes == DirectoryChangesMode::Garam => {
                Some(select_garam_url(self.client, directory)?)
            }
            _ => None,
        };
        Ok(AuthzenMode {
            url,
            token_endpoint,
            authentication,
            profile: self.client.authzen_profile,
            garam_url,
        })
    }
}

/// Checks the settings the worker delivers changes with, and reads the AuthZEN key file; none
/// where changes go nowhere.
///
/// # Errors
///
/// [`Error::MissingSetting`] where delivering lacks a setting, [`Error::IncompatibleSettings`]
/// where the settings cannot go together and [`Error::KeyFile`] where the key file cannot be read.
pub(crate) fn select_delivery<'a>(
    client: &'a AuthzenClientOptions,
    directory: &'a DirectoryOptions,
) -> Result<Option<GaramDelivery<'a>>, Error> {
    if directory.directory_changes == DirectoryChangesMode::None {
        return Ok(None);
    }
    let url = select_garam_url(client, directory)?;
    let (token_endpoint, authentication) = select_client(client)?;
    Ok(Some(GaramDelivery {
        token_endpoint,
        authentication,
        url,
    }))
}

/// Checks that the lease outlasts the heartbeat interval.
fn check_runner_lease(runner: &RunnerOptions) -> Result<(), Error> {
    if runner.runner_lease > runner.runner_heartbeat_interval {
        Ok(())
    } else {
        Err(Error::RunnerLease {
            lease: runner.runner_lease,
            heartbeat: runner.runner_heartbeat_interval,
        })
    }
}

fn select_identity(options: &IdentityOptions) -> Result<IdentityMode<'_>, Error> {
    Ok(match options.identity {
        Identity::Oidc => IdentityMode::Oidc {
            issuer: require(&options.oidc_issuer, "--oidc-issuer (CODEHUB_OIDC_ISSUER)")?,
            audience: require(
                &options.oidc_audience,
                "--oidc-audience (CODEHUB_OIDC_AUDIENCE)",
            )?,
        },
        Identity::Introspection => IdentityMode::Introspection {
            endpoint: require(
                &options.introspection_endpoint,
                "--introspection-endpoint (CODEHUB_INTROSPECTION_ENDPOINT)",
            )?,
            client_id: require(
                &options.introspection_client_id,
                "--introspection-client-id (CODEHUB_INTROSPECTION_CLIENT_ID)",
            )?,
            client_secret: require(
                &options.introspection_client_secret,
                "--introspection-client-secret (CODEHUB_INTROSPECTION_CLIENT_SECRET)",
            )?,
            issuer: require(
                &options.introspection_issuer,
                "--introspection-issuer (CODEHUB_INTROSPECTION_ISSUER)",
            )?,
            audience: require(
                &options.introspection_audience,
                "--introspection-audience (CODEHUB_INTROSPECTION_AUDIENCE)",
            )?,
        },
    })
}

/// The token endpoint and how the token client authenticates to it, the key file read.
fn select_client(client: &AuthzenClientOptions) -> Result<(&str, Authentication), Error> {
    let token_endpoint = require(
        &client.authzen_token_endpoint,
        "--authzen-token-endpoint (CODEHUB_AUTHZEN_TOKEN_ENDPOINT)",
    )?;
    let client_id = require(
        &client.authzen_client_id,
        "--authzen-client-id (CODEHUB_AUTHZEN_CLIENT_ID)",
    )?;
    let authentication = match client.authzen_client_authentication {
        AuthzenClientAuthentication::ClientSecret => {
            if client.authzen_profile == AuthzenProfile::Grn {
                return Err(Error::IncompatibleSettings(
                    "CODEHUB_AUTHZEN_PROFILE=grn needs \
                     CODEHUB_AUTHZEN_CLIENT_AUTHENTICATION=private-key-jwt",
                ));
            }
            Authentication::ClientSecret {
                client_id: client_id.to_owned(),
                client_secret: require(
                    &client.authzen_client_secret,
                    "--authzen-client-secret (CODEHUB_AUTHZEN_CLIENT_SECRET)",
                )?
                .to_owned(),
            }
        }
        AuthzenClientAuthentication::PrivateKeyJwt => {
            let key_file = client
                .authzen_key_file
                .as_ref()
                .ok_or(Error::MissingSetting(
                    "--authzen-key-file (CODEHUB_AUTHZEN_KEY_FILE)",
                ))?;
            let issuer = require(
                &client.authzen_issuer,
                "--authzen-issuer (CODEHUB_AUTHZEN_ISSUER)",
            )?;
            let key_pem =
                std::fs::read(key_file).map_err(|error| Error::KeyFile(key_file.clone(), error))?;
            Authentication::PrivateKeyJwt {
                client_id_template: client_id.to_owned(),
                key_pem,
                key_id: client.authzen_key_id.clone(),
                issuer: issuer.to_owned(),
            }
        }
    };
    Ok((token_endpoint, authentication))
}

/// The URL changes are delivered to garam at, refused under any profile but `grn`.
fn select_garam_url<'a>(
    client: &AuthzenClientOptions,
    directory: &'a DirectoryOptions,
) -> Result<&'a str, Error> {
    if client.authzen_profile != AuthzenProfile::Grn {
        return Err(garam_needs_grn());
    }
    require(
        &directory.directory_garam_url,
        "--directory-garam-url (CODEHUB_DIRECTORY_GARAM_URL)",
    )
}

impl AllOptions {
    /// The groups of settings the role is checked over.
    pub(crate) fn serving(&self) -> Serving<'_> {
        Serving {
            identity: &self.identity,
            authorisation: &self.authorisation,
            client: &self.authzen,
            directory: Some(&self.directory),
            runner: Some(&self.runner),
        }
    }
}

impl ApiOptions {
    /// The groups of settings the role is checked over.
    pub(crate) fn serving(&self) -> Serving<'_> {
        Serving {
            identity: &self.identity,
            authorisation: &self.authorisation,
            client: &self.authzen,
            directory: Some(&self.directory),
            runner: None,
        }
    }
}

impl RunnerApiOptions {
    /// The groups of settings the role is checked over.
    pub(crate) fn serving(&self) -> Serving<'_> {
        Serving {
            identity: &self.identity,
            authorisation: &self.authorisation,
            client: &self.authzen,
            directory: None,
            runner: Some(&self.runner),
        }
    }
}

/// Delivering changes to garam is refused under any profile but `grn`.
fn garam_needs_grn() -> Error {
    Error::IncompatibleSettings("CODEHUB_DIRECTORY_CHANGES=garam needs CODEHUB_AUTHZEN_PROFILE=grn")
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

/// The value of an optional setting a mode needs, or [`Error::MissingSetting`] naming `setting`.
fn require<'a>(value: &'a Option<String>, setting: &'static str) -> Result<&'a str, Error> {
    value.as_deref().ok_or(Error::MissingSetting(setting))
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;
    use std::sync::OnceLock;

    use clap::CommandFactory;

    use super::*;

    fn config(postgres_url: &str) -> AllOptions {
        AllOptions {
            listen: ListenOptions {
                listen: SocketAddr::from(([127, 0, 0, 1], 8080)),
            },
            database: DatabaseOptions {
                postgres_url: postgres_url.to_owned(),
                postgres_pool_size: 10,
            },
            landing: LandingOptions {
                landing_attempts: LandingAttempts::DEFAULT,
            },
            runner: RunnerOptions {
                runner_claim_hold: 1,
                runner_heartbeat_interval: NonZeroU32::MIN,
                runner_lease: NonZeroU32::MIN.saturating_add(1),
            },
            objects: ObjectOptions {
                s3_endpoint: "http://objects.example:9000".to_owned(),
                s3_region: "eu-test-1".to_owned(),
                s3_bucket: "bucket-visible".to_owned(),
                s3_access_key: "access-visible".to_owned(),
                s3_secret_key: "s3-secret-value".to_owned(),
                max_object_size: 1,
                grant_expiry_seconds: 1,
            },
            identity: IdentityOptions {
                identity: Identity::Introspection,
                oidc_issuer: Some("http://issuer.example".to_owned()),
                oidc_audience: Some("audience-visible".to_owned()),
                jwks_min_refetch_seconds: 1,
                introspection_endpoint: Some("http://issuer.example/introspect".to_owned()),
                introspection_client_id: Some("introspection-client-visible".to_owned()),
                introspection_client_secret: Some("introspection-secret-value".to_owned()),
                introspection_issuer: Some("http://issuer.example".to_owned()),
                introspection_audience: Some("introspection-audience-visible".to_owned()),
            },
            authorisation: AuthorisationOptions {
                authorisation: Authorisation::Authzen,
                authzen_url: Some("http://pdp.example".to_owned()),
                authzen_evaluations: false,
            },
            authzen: AuthzenClientOptions {
                authzen_token_endpoint: Some("http://issuer.example/token".to_owned()),
                authzen_client_id: Some("authzen-client-visible".to_owned()),
                authzen_client_secret: Some("authzen-secret-value".to_owned()),
                authzen_profile: AuthzenProfile::Plain,
                authzen_client_authentication: AuthzenClientAuthentication::ClientSecret,
                authzen_key_file: Some(PathBuf::from("/keys/authzen-key-visible.pem")),
                authzen_key_id: Some("authzen-kid-visible".to_owned()),
                authzen_issuer: Some("http://garam.example".to_owned()),
                authzen_audience: Some("authzen-audience-visible".to_owned()),
            },
            directory: DirectoryOptions {
                directory_changes: DirectoryChangesMode::None,
                directory_garam_url: Some("http://garam-visible.example".to_owned()),
            },
            delivery: DeliveryOptions {
                directory_delivery_wait: 2,
            },
            overload: OverloadOptions {
                rate_limit_per_second: NonZeroU32::MIN,
                rate_limit_burst: NonZeroU32::MIN,
                max_body_size: NonZeroUsize::MIN,
                request_timeout_seconds: 1,
                max_in_flight: NonZeroUsize::MIN,
                identity_timeout_seconds: 1,
                s3_connect_timeout_seconds: 1,
                s3_timeout_seconds: 1,
            },
            timing: TimingOptions {
                authzen_timeout_seconds: 1,
                shutdown_deadline_seconds: 1,
            },
        }
    }

    /// The options of `all` that `args` parse to, given after the program's name alone.
    fn parse(args: &[&str]) -> AllOptions {
        let args = ["codehub"].into_iter().chain(args.iter().copied());
        match Cli::try_from_args(args.map(OsString::from)).unwrap().role {
            Role::All(options) => options,
            other @ (Role::Api(_) | Role::RunnerApi(_) | Role::Worker(_) | Role::Migrate(_)) => {
                panic!("not the all role: {other:?}")
            }
        }
    }

    #[test]
    fn leaves_access_evaluations_off_by_default_and_turns_them_on_by_flag() {
        let required = [
            "--postgres-url=postgres://db.example/codehub",
            "--s3-endpoint=http://objects.example:9000",
            "--s3-region=eu-test-1",
            "--s3-bucket=bucket",
            "--s3-access-key=access",
            "--s3-secret-key=secret",
        ];

        let default = parse(&required);
        let on = parse(&[&required[..], &["--authzen-evaluations"]].concat());

        assert!(!default.authorisation.authzen_evaluations);
        assert!(on.authorisation.authzen_evaluations);
    }

    /// The command line of `words`, after the program's name.
    fn command_line(words: &[&str]) -> Vec<OsString> {
        ["codehub"]
            .iter()
            .chain(words)
            .map(OsString::from)
            .collect()
    }

    #[test]
    fn takes_the_all_role_where_the_command_line_names_none() {
        let with = |words: &[&str]| {
            with_default_role(command_line(words))
                .iter()
                .skip(1)
                .map(|word| word.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(with(&[]), ["all"]);
        // A flag's value that is a role's name is a value, not a role.
        assert_eq!(with(&["--identity", "api"]), ["all", "--identity", "api"]);
        assert_eq!(
            with(&["--listen=127.0.0.1:1"]),
            ["all", "--listen=127.0.0.1:1"]
        );
        // A role, the help and the version are left as they are.
        assert_eq!(
            with(&["api", "--listen=127.0.0.1:1"]),
            ["api", "--listen=127.0.0.1:1"]
        );
        assert_eq!(with(&["--help"]), ["--help"]);
        assert_eq!(with(&["-h"]), ["-h"]);
        assert_eq!(with(&["--version"]), ["--version"]);
        assert_eq!(with(&["-V"]), ["-V"]);
        assert_eq!(with(&["help", "api"]), ["help", "api"]);
    }

    /// The long names of the options `role` takes, sorted.
    fn options_of(role: &str) -> Vec<String> {
        let command = Cli::command();
        let role = command.find_subcommand(role).unwrap();
        let mut names: Vec<String> = role
            .get_arguments()
            .filter_map(|argument| argument.get_long())
            .filter(|name| !matches!(*name, "help" | "version"))
            .map(str::to_owned)
            .collect();
        names.sort();
        names
    }

    #[test]
    fn each_role_takes_the_options_it_uses_and_no_others() {
        let all = options_of("all");
        assert_eq!(all.len(), 48, "{all:?}");
        let without = |role: &str, left_out: &[&str]| {
            let taken = options_of(role);
            let expected: Vec<String> = all
                .iter()
                .filter(|name| !left_out.contains(&name.as_str()))
                .cloned()
                .collect();
            assert_eq!(taken, expected, "{role}");
        };
        let object_settings = [
            "s3-endpoint",
            "s3-region",
            "s3-bucket",
            "s3-access-key",
            "s3-secret-key",
            "max-object-size",
            "grant-expiry-seconds",
        ];
        let identity_settings = [
            "identity",
            "oidc-issuer",
            "oidc-audience",
            "jwks-min-refetch-seconds",
            "introspection-endpoint",
            "introspection-client-id",
            "introspection-client-secret",
            "introspection-issuer",
            "introspection-audience",
        ];
        let decision_settings = ["authorisation", "authzen-url", "authzen-evaluations"];
        let runner_settings = [
            "runner-claim-hold",
            "runner-heartbeat-interval",
            "runner-lease",
        ];
        let directory_settings = ["directory-changes", "directory-garam-url"];
        let creation_wait = ["directory-delivery-wait"];
        let landing = ["landing-attempts"];
        let overload_settings = [
            "rate-limit-per-second",
            "rate-limit-burst",
            "max-body-size",
            "request-timeout-seconds",
            "max-in-flight",
            "identity-timeout-seconds",
            "s3-connect-timeout-seconds",
            "s3-timeout-seconds",
        ];
        let concat = |parts: &[&[&'static str]]| -> Vec<&'static str> {
            parts.iter().flat_map(|part| part.iter().copied()).collect()
        };
        without("api", &concat(&[&runner_settings]));
        without(
            "runner-api",
            &concat(&[&directory_settings, &creation_wait]),
        );
        without(
            "worker",
            &concat(&[
                &object_settings,
                &identity_settings,
                &decision_settings,
                &runner_settings,
                &creation_wait,
                &landing,
                &overload_settings,
            ]),
        );
        assert_eq!(
            options_of("migrate"),
            ["postgres-pool-size", "postgres-url"]
        );
    }

    #[test]
    fn a_role_refuses_an_option_it_does_not_use_and_takes_it_where_it_does() {
        let database = "--postgres-url=postgres://db.example/codehub";
        let refused = |role: &str, option: &str| {
            Cli::try_from_args(command_line(&[role, database, option]))
                .err()
                .map(|error| error.kind())
        };
        // Each control is a role that does take the option, parsed the same way.
        let controls = |role: &str, option: &str| {
            let mut words = vec![role, database, option];
            let required = [
                "--s3-endpoint=http://objects.example:9000",
                "--s3-region=eu-test-1",
                "--s3-bucket=bucket",
                "--s3-access-key=access",
                "--s3-secret-key=secret",
                "--oidc-issuer=http://issuer.example",
                "--oidc-audience=audience",
            ];
            if role != "worker" && role != "migrate" {
                words.extend(
                    required
                        .into_iter()
                        .filter(|word| word.split('=').next() != option.split('=').next()),
                );
            }
            Cli::try_from_args(command_line(&words)).is_ok()
        };
        assert_eq!(
            refused("migrate", "--listen=127.0.0.1:1"),
            Some(clap::error::ErrorKind::UnknownArgument)
        );
        assert!(controls("worker", "--listen=127.0.0.1:1"));
        assert_eq!(
            refused("worker", "--s3-bucket=bucket"),
            Some(clap::error::ErrorKind::UnknownArgument)
        );
        assert!(controls("api", "--s3-bucket=bucket"));
        assert_eq!(
            refused("api", "--runner-lease=9"),
            Some(clap::error::ErrorKind::UnknownArgument)
        );
        assert!(controls("runner-api", "--runner-lease=9"));
        assert_eq!(
            refused("runner-api", "--directory-changes=none"),
            Some(clap::error::ErrorKind::UnknownArgument)
        );
        assert!(controls("api", "--directory-changes=none"));
    }

    #[test]
    fn the_worker_checks_what_delivering_to_garam_needs() {
        let parse = |words: &[&str]| {
            let line = command_line(
                &[
                    &["worker", "--postgres-url=postgres://db.example/codehub"][..],
                    words,
                ]
                .concat(),
            );
            match Cli::try_from_args(line).unwrap().role {
                Role::Worker(options) => options,
                other @ (Role::Api(_) | Role::RunnerApi(_) | Role::Migrate(_) | Role::All(_)) => {
                    panic!("not the worker role: {other:?}")
                }
            }
        };
        let refusal = |words: &[&str]| {
            let options = parse(words);
            select_delivery(&options.authzen, &options.directory)
                .err()
                .map(|error| error.to_string())
        };
        // Delivering nowhere needs no setting at all.
        assert_eq!(refusal(&[]), None);
        assert_eq!(
            refusal(&["--directory-changes=garam"]).as_deref(),
            Some("CODEHUB_DIRECTORY_CHANGES=garam needs CODEHUB_AUTHZEN_PROFILE=grn")
        );
        let message =
            refusal(&["--directory-changes=garam", "--authzen-profile=grn"]).unwrap_or_default();
        assert!(message.contains("--directory-garam-url"), "{message}");
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

    #[test]
    fn debug_output_redacts_a_password_query_parameter_and_keeps_a_url_without_one() {
        let with_parameter = format!(
            "{:?}",
            config("postgres://db.example/codehub?sslmode=require&password=qp-secret")
        );
        assert!(!with_parameter.contains("qp-secret"), "{with_parameter}");
        assert!(
            with_parameter
                .contains("postgres://db.example/codehub?sslmode=require&password=<redacted>"),
            "{with_parameter}"
        );
        let without = format!("{:?}", config("postgres://db.example/codehub"));
        assert!(
            without.contains("postgres://db.example/codehub"),
            "{without}"
        );
        assert!(!without.contains("<redacted>@"), "{without}");
    }

    /// A change to the settings of a case.
    type Change = fn(&mut AllOptions);

    /// The message `select` refuses `config` with after `change`, none where it selects.
    fn refusal(change: impl FnOnce(&mut AllOptions)) -> Option<String> {
        let mut config = config("postgres://db.example/codehub");
        change(&mut config);
        config
            .serving()
            .select()
            .err()
            .map(|error| error.to_string())
    }

    /// Settings of the `grn` profile with `private-key-jwt`, delivering to garam.
    fn grn(config: &mut AllOptions) {
        config.authzen.authzen_profile = AuthzenProfile::Grn;
        config.authzen.authzen_client_authentication = AuthzenClientAuthentication::PrivateKeyJwt;
        config.authzen.authzen_key_file = Some(key_file());
        config.directory.directory_changes = DirectoryChangesMode::Garam;
    }

    /// A readable file in the temporary directory, written once so that parallel tests read it
    /// whole.
    fn key_file() -> PathBuf {
        static PATH: OnceLock<PathBuf> = OnceLock::new();
        PATH.get_or_init(|| {
            let path = std::env::temp_dir().join(format!("codehub-key-{}.pem", std::process::id()));
            std::fs::write(&path, "key-bytes").unwrap();
            path
        })
        .clone()
    }

    #[test]
    fn a_lease_must_exceed_the_heartbeat_interval() {
        let at = |lease: u32| {
            refusal(|config| {
                config.runner.runner_heartbeat_interval = NonZeroU32::MIN.saturating_add(4);
                config.runner.runner_lease = NonZeroU32::new(lease).unwrap();
            })
        };
        for lease in [1, 4, 5] {
            let message = at(lease).unwrap_or_default();
            assert!(
                message.contains("CODEHUB_RUNNER_LEASE"),
                "{lease}: {message}"
            );
        }
        assert_eq!(at(6), None);
    }

    #[test]
    fn names_the_setting_a_selected_mode_lacks() {
        assert_eq!(refusal(|_| {}), None);
        assert_eq!(refusal(grn), None);
        let cases: [(&str, Change, &str); 13] = [
            (
                "oidc issuer",
                |config| {
                    config.identity.identity = Identity::Oidc;
                    config.identity.oidc_issuer = None;
                },
                "--oidc-issuer (CODEHUB_OIDC_ISSUER)",
            ),
            (
                "oidc audience",
                |config| {
                    config.identity.identity = Identity::Oidc;
                    config.identity.oidc_audience = None;
                },
                "--oidc-audience (CODEHUB_OIDC_AUDIENCE)",
            ),
            (
                "introspection client secret",
                |config| config.identity.introspection_client_secret = None,
                "--introspection-client-secret (CODEHUB_INTROSPECTION_CLIENT_SECRET)",
            ),
            (
                "introspection client id",
                |config| config.identity.introspection_client_id = None,
                "--introspection-client-id (CODEHUB_INTROSPECTION_CLIENT_ID)",
            ),
            (
                "introspection issuer",
                |config| config.identity.introspection_issuer = None,
                "--introspection-issuer (CODEHUB_INTROSPECTION_ISSUER)",
            ),
            (
                "introspection audience",
                |config| config.identity.introspection_audience = None,
                "--introspection-audience (CODEHUB_INTROSPECTION_AUDIENCE)",
            ),
            (
                "authzen url",
                |config| config.authorisation.authzen_url = None,
                "--authzen-url (CODEHUB_AUTHZEN_URL)",
            ),
            (
                "authzen token endpoint",
                |config| config.authzen.authzen_token_endpoint = None,
                "--authzen-token-endpoint (CODEHUB_AUTHZEN_TOKEN_ENDPOINT)",
            ),
            (
                "authzen client id",
                |config| config.authzen.authzen_client_id = None,
                "--authzen-client-id (CODEHUB_AUTHZEN_CLIENT_ID)",
            ),
            (
                "authzen client secret",
                |config| config.authzen.authzen_client_secret = None,
                "--authzen-client-secret (CODEHUB_AUTHZEN_CLIENT_SECRET)",
            ),
            (
                "authzen key file",
                |config| {
                    grn(config);
                    config.authzen.authzen_key_file = None;
                },
                "--authzen-key-file (CODEHUB_AUTHZEN_KEY_FILE)",
            ),
            (
                "authzen issuer",
                |config| {
                    grn(config);
                    config.authzen.authzen_issuer = None;
                },
                "--authzen-issuer (CODEHUB_AUTHZEN_ISSUER)",
            ),
            (
                "garam url",
                |config| {
                    grn(config);
                    config.directory.directory_garam_url = None;
                },
                "--directory-garam-url (CODEHUB_DIRECTORY_GARAM_URL)",
            ),
        ];
        for (case, change, setting) in cases {
            let message = refusal(change).unwrap_or_default();
            assert!(message.contains(setting), "{case}: {message}");
        }
    }

    #[test]
    fn leaves_settings_of_a_mode_that_is_not_selected_unchecked() {
        assert_eq!(
            refusal(|config| {
                config.authorisation.authorisation = Authorisation::Postgres;
                config.authorisation.authzen_url = None;
                config.authzen.authzen_client_secret = None;
            }),
            None
        );
        assert_eq!(
            refusal(|config| {
                config.identity.identity = Identity::Oidc;
                config.identity.introspection_endpoint = None;
            }),
            None
        );
    }

    #[test]
    fn refuses_settings_that_cannot_go_together_and_accepts_the_grn_profile_beside_them() {
        assert_eq!(refusal(grn), None);
        let cases: [(&str, Change, &str); 4] = [
            (
                "grn with built-in authorisation",
                |config| {
                    grn(config);
                    config.authorisation.authorisation = Authorisation::Postgres;
                },
                "CODEHUB_AUTHZEN_PROFILE=grn needs CODEHUB_AUTHORISATION=authzen",
            ),
            (
                "grn with a client secret",
                |config| {
                    grn(config);
                    config.authzen.authzen_client_authentication =
                        AuthzenClientAuthentication::ClientSecret;
                },
                "CODEHUB_AUTHZEN_PROFILE=grn needs \
                 CODEHUB_AUTHZEN_CLIENT_AUTHENTICATION=private-key-jwt",
            ),
            (
                "garam under the plain profile",
                |config| {
                    grn(config);
                    config.authzen.authzen_profile = AuthzenProfile::Plain;
                    config.authzen.authzen_client_authentication =
                        AuthzenClientAuthentication::ClientSecret;
                },
                "CODEHUB_DIRECTORY_CHANGES=garam needs CODEHUB_AUTHZEN_PROFILE=grn",
            ),
            (
                "garam with built-in authorisation",
                |config| {
                    config.authorisation.authorisation = Authorisation::Postgres;
                    config.directory.directory_changes = DirectoryChangesMode::Garam;
                },
                "CODEHUB_DIRECTORY_CHANGES=garam needs CODEHUB_AUTHZEN_PROFILE=grn",
            ),
        ];
        for (case, change, expected) in cases {
            assert_eq!(refusal(change).as_deref(), Some(expected), "{case}");
        }
    }

    #[test]
    fn reads_the_key_file_and_names_one_it_cannot_read() {
        let mut config = config("postgres://db.example/codehub");
        grn(&mut config);
        let Ok(Selection {
            authorisation: AuthorisationMode::Authzen(mode),
            ..
        }) = config.serving().select()
        else {
            panic!("the readable key file was refused");
        };
        assert!(matches!(
            mode.authentication,
            Authentication::PrivateKeyJwt { ref key_pem, .. } if key_pem == b"key-bytes"
        ));

        let missing = std::env::temp_dir().join("codehub-no-such-key-file.pem");
        assert!(!missing.exists(), "{} exists", missing.display());
        config.authzen.authzen_key_file = Some(missing.clone());
        let Some(Error::KeyFile(path, _)) = config.serving().select().err() else {
            panic!("the missing key file was accepted");
        };
        assert_eq!(path, missing);
    }

    #[test]
    fn selects_who_administers_members_by_the_authorisation_mode() {
        let administration = |change: Change| {
            let mut config = config("postgres://db.example/codehub");
            change(&mut config);
            config
                .serving()
                .select()
                .map(|selection| selection.administration)
                .ok()
        };
        assert_eq!(
            administration(|config| config.authorisation.authorisation = Authorisation::Postgres),
            Some(Administration::BuiltIn)
        );
        assert_eq!(
            administration(|config| config.authorisation.authorisation = Authorisation::Authzen),
            Some(Administration::External)
        );
    }
}
