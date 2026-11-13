//! The server's settings: the flags and variables, the check that a mode's settings are given,
//! and the redaction of secrets where they are printed.

use std::fmt;
use std::net::SocketAddr;
use std::num::NonZeroU32;
use std::path::PathBuf;

use app::use_case::landing_attempts::LandingAttempts;
use clap::{Parser, ValueEnum};

use crate::Error;

/// The default of `--runner-heartbeat-interval`, in seconds.
const DEFAULT_HEARTBEAT_INTERVAL: NonZeroU32 = NonZeroU32::MIN.saturating_add(4);

/// The default of `--runner-lease`, in seconds.
const DEFAULT_LEASE: NonZeroU32 = NonZeroU32::MIN.saturating_add(29);

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

/// The server's settings. Each is a flag and has an environment variable of the same name,
/// upper-cased, with the prefix `CODEHUB_`.
#[derive(Parser)]
#[command(version, about = "The codehub server")]
pub struct Config {
    /// The address to listen on; port 0 takes any free port.
    #[arg(long, env = "CODEHUB_LISTEN", default_value = "127.0.0.1:8080")]
    pub listen: SocketAddr,
    /// The URL of the PostgreSQL database holding the metadata.
    #[arg(long, env = "CODEHUB_POSTGRES_URL", hide_env_values = true)]
    pub postgres_url: String,
    /// The most connections this server process holds open to PostgreSQL. The sum over all server
    /// processes must stay below PostgreSQL's `max_connections`, less the connections other
    /// clients use (ADR 0037).
    #[arg(long, env = "CODEHUB_POSTGRES_POOL_SIZE", default_value_t = 40)]
    pub postgres_pool_size: u32,
    /// The most times a landing is tried where `main` keeps moving under it, at least one; when
    /// they are spent the landing is answered as moved (ADR 0036).
    #[arg(long, env = "CODEHUB_LANDING_ATTEMPTS", default_value_t = LandingAttempts::DEFAULT)]
    pub landing_attempts: NonZeroU32,
    /// How long a runner's claim is held waiting for a job before it is answered with none, in
    /// seconds.
    #[arg(long, env = "CODEHUB_RUNNER_CLAIM_HOLD", default_value_t = 30)]
    pub runner_claim_hold: u64,
    /// How often a runner is told to heartbeat, in seconds, at least one.
    #[arg(long, env = "CODEHUB_RUNNER_HEARTBEAT_INTERVAL", default_value_t = DEFAULT_HEARTBEAT_INTERVAL)]
    pub runner_heartbeat_interval: NonZeroU32,
    /// How long a claim or a heartbeat holds a job for its runner, in seconds; it must exceed the
    /// heartbeat interval, or a runner loses its job between two heartbeats.
    #[arg(long, env = "CODEHUB_RUNNER_LEASE", default_value_t = DEFAULT_LEASE)]
    pub runner_lease: NonZeroU32,
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
    /// The client id used at that token endpoint; required with `--authorisation authzen`. With
    /// `--authzen-client-authentication private-key-jwt` it is a template holding
    /// `{organisation}`, such as `grn:{organisation}/iam/service/codehub`.
    #[arg(long, env = "CODEHUB_AUTHZEN_CLIENT_ID")]
    pub authzen_client_id: Option<String>,
    /// The client secret used at that token endpoint; required with
    /// `--authzen-client-authentication client-secret`.
    #[arg(long, env = "CODEHUB_AUTHZEN_CLIENT_SECRET", hide_env_values = true)]
    pub authzen_client_secret: Option<String>,
    /// Sends one AuthZEN Access Evaluations request for many resources where the decision point
    /// advertises it, instead of one evaluation per resource. Turn it on only where the decision
    /// point decides each evaluation by its own resource properties (ADR 0041).
    #[arg(long, env = "CODEHUB_AUTHZEN_EVALUATIONS")]
    pub authzen_evaluations: bool,
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
    /// Where repository and organisation changes are delivered: nowhere (`none`) or to garam's
    /// `ApplyEntityChanges` (`garam`, which needs `--authzen-profile grn`). Every repository change
    /// is written to the outbox either way (ADR 0044).
    #[arg(long, env = "CODEHUB_DIRECTORY_CHANGES", value_enum, default_value_t = DirectoryChangesMode::None)]
    pub directory_changes: DirectoryChangesMode,
    /// The base URL of garam's policy service, which the entity changes are sent to; required with
    /// `--directory-changes garam`.
    #[arg(long, env = "CODEHUB_DIRECTORY_GARAM_URL")]
    pub directory_garam_url: Option<String>,
    /// How long creating a repository waits for its change to be delivered, in seconds. The
    /// creation is answered once it elapses whether or not the change was delivered.
    #[arg(long, env = "CODEHUB_DIRECTORY_DELIVERY_WAIT", default_value_t = 2)]
    pub directory_delivery_wait: u64,
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
            landing_attempts,
            runner_claim_hold,
            runner_heartbeat_interval,
            runner_lease,
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
            authzen_evaluations,
            authzen_profile,
            authzen_client_authentication,
            authzen_key_file,
            authzen_key_id,
            authzen_issuer,
            authzen_audience,
            directory_changes,
            directory_garam_url,
            directory_delivery_wait,
        } = self;
        f.debug_struct("Config")
            .field("listen", listen)
            .field("postgres_url", &redact_url(postgres_url))
            .field("postgres_pool_size", postgres_pool_size)
            .field("landing_attempts", landing_attempts)
            .field("runner_claim_hold", runner_claim_hold)
            .field("runner_heartbeat_interval", runner_heartbeat_interval)
            .field("runner_lease", runner_lease)
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
            .field("authzen_evaluations", authzen_evaluations)
            .field("authzen_profile", authzen_profile)
            .field(
                "authzen_client_authentication",
                authzen_client_authentication,
            )
            .field("authzen_key_file", authzen_key_file)
            .field("authzen_key_id", authzen_key_id)
            .field("authzen_issuer", authzen_issuer)
            .field("authzen_audience", authzen_audience)
            .field("directory_changes", directory_changes)
            .field("directory_garam_url", directory_garam_url)
            .field("directory_delivery_wait", directory_delivery_wait)
            .finish()
    }
}

impl Config {
    /// Checks that the lease outlasts the heartbeat interval.
    ///
    /// # Errors
    ///
    /// [`Error::RunnerLease`] where it does not.
    pub(crate) fn check_runner_lease(&self) -> Result<(), Error> {
        if self.runner_lease > self.runner_heartbeat_interval {
            Ok(())
        } else {
            Err(Error::RunnerLease {
                lease: self.runner_lease,
                heartbeat: self.runner_heartbeat_interval,
            })
        }
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

/// The value of an optional setting a mode needs, or [`Error::MissingSetting`] naming `setting`.
pub(crate) fn require<'a>(
    value: &'a Option<String>,
    setting: &'static str,
) -> Result<&'a str, Error> {
    value.as_deref().ok_or(Error::MissingSetting(setting))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(postgres_url: &str) -> Config {
        Config {
            listen: SocketAddr::from(([127, 0, 0, 1], 8080)),
            postgres_url: postgres_url.to_owned(),
            postgres_pool_size: 10,
            landing_attempts: LandingAttempts::DEFAULT,
            runner_claim_hold: 1,
            runner_heartbeat_interval: NonZeroU32::MIN,
            runner_lease: NonZeroU32::MIN.saturating_add(1),
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
            authzen_evaluations: false,
            authzen_profile: AuthzenProfile::Plain,
            authzen_client_authentication: AuthzenClientAuthentication::ClientSecret,
            authzen_key_file: Some(PathBuf::from("/keys/authzen-key-visible.pem")),
            authzen_key_id: Some("authzen-kid-visible".to_owned()),
            authzen_issuer: Some("http://garam.example".to_owned()),
            authzen_audience: Some("authzen-audience-visible".to_owned()),
            directory_changes: DirectoryChangesMode::None,
            directory_garam_url: Some("http://garam-visible.example".to_owned()),
            directory_delivery_wait: 2,
        }
    }

    #[test]
    fn leaves_access_evaluations_off_by_default_and_turns_them_on_by_flag() {
        let required = [
            "codehub",
            "--postgres-url=postgres://db.example/codehub",
            "--s3-endpoint=http://objects.example:9000",
            "--s3-region=eu-test-1",
            "--s3-bucket=bucket",
            "--s3-access-key=access",
            "--s3-secret-key=secret",
        ];

        let default = Config::try_parse_from(required).unwrap();
        let on =
            Config::try_parse_from(required.into_iter().chain(["--authzen-evaluations"])).unwrap();

        assert!(!default.authzen_evaluations);
        assert!(on.authzen_evaluations);
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

    #[test]
    fn a_lease_must_exceed_the_heartbeat_interval() {
        let mut config = config("postgres://db.example/codehub");
        config.runner_heartbeat_interval = NonZeroU32::MIN.saturating_add(4);
        for lease in [1, 4, 5] {
            config.runner_lease = NonZeroU32::new(lease).unwrap();
            let error = config.check_runner_lease().unwrap_err();
            assert!(
                error.to_string().contains("CODEHUB_RUNNER_LEASE"),
                "{error}"
            );
        }
        config.runner_lease = NonZeroU32::new(6).unwrap();
        assert!(config.check_runner_lease().is_ok());
    }

    #[test]
    fn require_names_the_missing_setting_and_passes_a_given_one() {
        let given = Some("value".to_owned());
        assert_eq!(require(&given, "--x (X)").ok(), Some("value"));
        assert!(matches!(
            require(&None, "--x (X)"),
            Err(Error::MissingSetting("--x (X)"))
        ));
    }
}
