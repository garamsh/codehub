//! The settings that bound what a process takes on and how long it waits (ADR 0055): the rate and
//! size of requests, how many are served at once, the timeouts of the calls it makes, and the
//! deadline of its shutdown.

use std::num::{NonZeroU32, NonZeroUsize};
use std::time::Duration;

use adapter_http::{InFlight, Limits, RateLimiter};
use adapter_s3::Timeouts;
use app::use_case::relay::LEASE;
use clap::Args;

/// The longest the decision point may take to answer, in seconds. One delivery of one entry to
/// garam is at most four requests, so four of these stay inside the relay's lease, which a pass
/// holds for all the entries it delivers (ADR 0055).
const MAX_AUTHZEN_TIMEOUT: u64 = 14;

const _: () = assert!(
    4 * MAX_AUTHZEN_TIMEOUT <= LEASE.as_secs(),
    "four requests at the longest timeout must fit in the relay's lease"
);

/// The default of `--rate-limit-per-second`.
const DEFAULT_RATE: NonZeroU32 = NonZeroU32::MIN.saturating_add(99);

/// The default of `--rate-limit-burst`.
const DEFAULT_BURST: NonZeroU32 = NonZeroU32::MIN.saturating_add(199);

/// The default of `--max-body-size`, one mebibyte.
const DEFAULT_BODY_SIZE: NonZeroUsize = NonZeroUsize::MIN.saturating_add(1024 * 1024 - 1);

/// The default of `--max-in-flight`.
const DEFAULT_IN_FLIGHT: NonZeroUsize = NonZeroUsize::MIN.saturating_add(511);

/// The limits of a role that serves callers.
#[derive(Args, Debug)]
pub struct OverloadOptions {
    /// The requests a second each caller of the user API may make, kept in this process only: with
    /// several replicas the limit multiplies (ADR 0055). The runner routes are not limited.
    #[arg(long, env = "CODEHUB_RATE_LIMIT_PER_SECOND", default_value_t = DEFAULT_RATE)]
    pub rate_limit_per_second: NonZeroU32,
    /// The requests a caller may make at once before the rate applies.
    #[arg(long, env = "CODEHUB_RATE_LIMIT_BURST", default_value_t = DEFAULT_BURST)]
    pub rate_limit_burst: NonZeroU32,
    /// The largest request body the server reads, in bytes; a larger one is answered 413. Objects
    /// do not pass through the API, so this bounds JSON and the runner's results, not
    /// `CODEHUB_MAX_OBJECT_SIZE`.
    #[arg(long, env = "CODEHUB_MAX_BODY_SIZE", default_value_t = DEFAULT_BODY_SIZE)]
    pub max_body_size: NonZeroUsize,
    /// How long a request may take, in seconds, before it is answered 503. The runner's claim and
    /// the landing bound themselves and are not subject to it.
    #[arg(long, env = "CODEHUB_REQUEST_TIMEOUT_SECONDS", default_value_t = 60, value_parser = clap::value_parser!(u64).range(1..=3600))]
    pub request_timeout_seconds: u64,
    /// The most requests this process serves at once; one more is answered 503 with `Retry-After`.
    #[arg(long, env = "CODEHUB_MAX_IN_FLIGHT", default_value_t = DEFAULT_IN_FLIGHT)]
    pub max_in_flight: NonZeroUsize,
    /// How long a request to the identity provider (its key set or its introspection endpoint) may
    /// take, in seconds. A timeout fails the caller's request as a server error.
    #[arg(long, env = "CODEHUB_IDENTITY_TIMEOUT_SECONDS", default_value_t = 10, value_parser = clap::value_parser!(u64).range(1..=3600))]
    pub identity_timeout_seconds: u64,
    /// How long connecting to the object store may take, in seconds.
    #[arg(long, env = "CODEHUB_S3_CONNECT_TIMEOUT_SECONDS", default_value_t = 3, value_parser = clap::value_parser!(u64).range(1..=3600))]
    pub s3_connect_timeout_seconds: u64,
    /// How long a request to the object store may wait for its response to start and then for each
    /// next byte of it, in seconds; a transfer as a whole has no bound.
    #[arg(long, env = "CODEHUB_S3_TIMEOUT_SECONDS", default_value_t = 10, value_parser = clap::value_parser!(u64).range(1..=3600))]
    pub s3_timeout_seconds: u64,
}

impl OverloadOptions {
    /// The limits the router applies.
    pub(crate) fn limits(&self) -> Limits {
        Limits {
            rate: RateLimiter::new(self.rate_limit_per_second, self.rate_limit_burst),
            in_flight: InFlight::new(self.max_in_flight),
            request_timeout: Duration::from_secs(self.request_timeout_seconds),
            body_bytes: self.max_body_size.get(),
        }
    }

    /// The timeouts of the object store's client.
    pub(crate) const fn object_timeouts(&self) -> Timeouts {
        Timeouts {
            connect: Duration::from_secs(self.s3_connect_timeout_seconds),
            read: Duration::from_secs(self.s3_timeout_seconds),
        }
    }

    /// How long a request to the identity provider may take.
    pub(crate) const fn identity_timeout(&self) -> Duration {
        Duration::from_secs(self.identity_timeout_seconds)
    }
}

/// The timeout of the calls to the decision point and garam, and the deadline of the shutdown,
/// which every role that listens has.
#[derive(Args, Debug)]
pub struct TimingOptions {
    /// How long a request to the AuthZEN decision point, its token endpoint or garam may take, in
    /// seconds. A timeout fails the request as a server error (ADR 0033). At most 14, so that the
    /// four requests of one entry's delivery to garam end inside the relay's lease.
    #[arg(long, env = "CODEHUB_AUTHZEN_TIMEOUT_SECONDS", default_value_t = 10, value_parser = clap::value_parser!(u64).range(1..=MAX_AUTHZEN_TIMEOUT))]
    pub authzen_timeout_seconds: u64,
    /// How long, after SIGTERM or SIGINT, the requests in flight may take to finish before they
    /// are dropped, in seconds. Keep it below the pod's termination grace period.
    #[arg(long, env = "CODEHUB_SHUTDOWN_DEADLINE_SECONDS", default_value_t = 25, value_parser = clap::value_parser!(u64).range(1..=3600))]
    pub shutdown_deadline_seconds: u64,
}

impl TimingOptions {
    /// How long a request to the decision point or garam may take.
    pub(crate) const fn authzen_timeout(&self) -> Duration {
        Duration::from_secs(self.authzen_timeout_seconds)
    }

    /// How long the requests in flight have to finish once shutdown begins.
    pub(crate) const fn shutdown_deadline(&self) -> Duration {
        Duration::from_secs(self.shutdown_deadline_seconds)
    }
}
