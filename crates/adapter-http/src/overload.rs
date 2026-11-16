//! Overload protection (ADR 0055): a token bucket per principal, a cap on the requests in flight,
//! and the bound on how long a request may take.

use std::collections::HashMap;
use std::num::{NonZeroU32, NonZeroUsize};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use app::domain::principal::Principal;
use axum::extract::{MatchedPath, Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use tower::ServiceExt;
use tower::timeout::Timeout;

use crate::ApiState;
use crate::problem::Problem;

/// The seconds a refused request is told to wait where the process has no better estimate.
const RETRY_AFTER_SECONDS: u64 = 1;

/// The routes the request timeout does not apply to: the runner's claim, which holds for
/// `RunnerSettings::hold`; the landing, whose attempts are bounded by `CODEHUB_LANDING_ATTEMPTS`
/// (ADR 0036) and whose wait for the gate by nothing but the cap on requests in flight (ADR 0039);
/// and the runner's result, whose settling may revert a landed changeset in steps that dropping the
/// request half-way would leave unfinished. They still count toward that cap (ADR 0055).
const OWN_BOUND: &[&str] = &[
    "/v1/runner/jobs/claim",
    "/v1/runner/jobs/{id}/result",
    "/v1/orgs/{org}/repos/{repo}/drafts/{draft}/land",
];

/// The limits of one server process.
pub struct Limits {
    /// Admits the requests of each principal of the user API.
    pub rate: RateLimiter,
    /// Counts the requests in flight.
    pub in_flight: InFlight,
    /// How long a request may take, `OWN_BOUND` routes aside.
    pub request_timeout: Duration,
    /// The largest request body the server reads, in bytes.
    pub body_bytes: usize,
}

/// A token bucket per principal, kept in this process only.
pub struct RateLimiter {
    per_second: f64,
    burst: f64,
    state: Mutex<Buckets>,
}

struct Buckets {
    by_principal: HashMap<Principal, Bucket>,
    swept: Instant,
}

#[derive(Clone, Copy)]
struct Bucket {
    tokens: f64,
    refilled: Instant,
}

impl Bucket {
    /// The tokens held at `now`, never more than `burst`.
    fn at(self, now: Instant, per_second: f64, burst: f64) -> f64 {
        let elapsed = now.saturating_duration_since(self.refilled).as_secs_f64();
        elapsed.mul_add(per_second, self.tokens).min(burst)
    }
}

impl RateLimiter {
    /// A limiter that refills each principal's bucket by `per_second` tokens a second up to
    /// `burst`, which a principal that has not called starts with.
    #[must_use]
    pub fn new(per_second: NonZeroU32, burst: NonZeroU32) -> Self {
        Self {
            per_second: f64::from(per_second.get()),
            burst: f64::from(burst.get()),
            state: Mutex::new(Buckets {
                by_principal: HashMap::new(),
                swept: Instant::now(),
            }),
        }
    }

    /// Takes one token of `principal`'s bucket, or answers the whole seconds until there is one.
    pub(crate) fn admit(&self, principal: &Principal) -> Result<(), u64> {
        self.admit_at(principal, Instant::now())
    }

    fn admit_at(&self, principal: &Principal, now: Instant) -> Result<(), u64> {
        let mut buckets = lock(&self.state);
        let full_after = Duration::from_secs_f64(self.burst / self.per_second);
        if now.saturating_duration_since(buckets.swept) >= full_after {
            // A bucket idle for a whole refill is full again, which is what an absent one is.
            let (per_second, burst) = (self.per_second, self.burst);
            buckets
                .by_principal
                .retain(|_, bucket| bucket.at(now, per_second, burst) < burst);
            buckets.swept = now;
        }
        let bucket = buckets
            .by_principal
            .entry(principal.clone())
            .or_insert(Bucket {
                tokens: self.burst,
                refilled: now,
            });
        let tokens = bucket.at(now, self.per_second, self.burst);
        if tokens >= 1.0 {
            *bucket = Bucket {
                tokens: tokens - 1.0,
                refilled: now,
            };
            Ok(())
        } else {
            *bucket = Bucket {
                tokens,
                refilled: now,
            };
            let wait = ((1.0 - tokens) / self.per_second).ceil();
            Err((wait as u64).max(RETRY_AFTER_SECONDS))
        }
    }
}

fn lock(buckets: &Mutex<Buckets>) -> MutexGuard<'_, Buckets> {
    buckets
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The number of requests a process is serving, and the most it takes at once.
pub struct InFlight {
    max: usize,
    current: AtomicUsize,
}

/// A place among the requests in flight, given back when it is dropped.
struct Slot<'a>(&'a InFlight);

impl InFlight {
    /// A counter that admits `max` requests at once.
    #[must_use]
    pub const fn new(max: NonZeroUsize) -> Self {
        Self {
            max: max.get(),
            current: AtomicUsize::new(0),
        }
    }

    fn enter(&self) -> Option<Slot<'_>> {
        if self.current.fetch_add(1, Ordering::AcqRel) >= self.max {
            self.current.fetch_sub(1, Ordering::AcqRel);
            None
        } else {
            Some(Slot(self))
        }
    }
}

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        self.0.current.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Answers 503 where the process is serving as many requests as it takes, and bounds the time of
/// every other request but the `OWN_BOUND` routes.
pub(crate) async fn bound(
    State(state): State<Arc<ApiState>>,
    matched: Option<MatchedPath>,
    request: Request,
    next: Next,
) -> Response {
    let limits = &state.limits;
    let Some(_slot) = limits.in_flight.enter() else {
        return Problem::overloaded(RETRY_AFTER_SECONDS).into_response();
    };
    if matched.is_some_and(|path| OWN_BOUND.contains(&path.as_str())) {
        return next.run(request).await;
    }
    Timeout::new(next, limits.request_timeout)
        .oneshot(request)
        .await
        .unwrap_or_else(|_| Problem::timed_out().into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ROUTES, RUNNER_ROUTES};

    fn principal(subject: &str) -> Principal {
        Principal::new("https://id.example", subject).unwrap()
    }

    fn limiter(per_second: f64, burst: f64) -> RateLimiter {
        RateLimiter {
            per_second,
            burst,
            state: Mutex::new(Buckets {
                by_principal: HashMap::new(),
                swept: Instant::now(),
            }),
        }
    }

    #[test]
    fn refuses_past_the_burst_and_refills_with_time() {
        let limiter = limiter(2.0, 3.0);
        let ada = principal("ada");
        let start = Instant::now();

        for _ in 0..3 {
            assert_eq!(limiter.admit_at(&ada, start), Ok(()));
        }
        assert_eq!(limiter.admit_at(&ada, start), Err(1));
        // Half a second refills one token; a quarter does not.
        assert_eq!(
            limiter.admit_at(&ada, start + Duration::from_millis(250)),
            Err(1)
        );
        assert_eq!(
            limiter.admit_at(&ada, start + Duration::from_millis(500)),
            Ok(())
        );
    }

    #[test]
    fn keeps_a_bucket_per_principal() {
        let limiter = limiter(1.0, 1.0);
        let start = Instant::now();

        assert_eq!(limiter.admit_at(&principal("ada"), start), Ok(()));
        assert_eq!(limiter.admit_at(&principal("ada"), start), Err(1));
        // The control: another principal, and the same subject under another issuer, are not
        // refused.
        assert_eq!(limiter.admit_at(&principal("grace"), start), Ok(()));
        let elsewhere = Principal::new("https://other.example", "ada").unwrap();
        assert_eq!(limiter.admit_at(&elsewhere, start), Ok(()));
    }

    #[test]
    fn tells_how_long_until_a_token_in_whole_seconds() {
        let limiter = limiter(0.5, 1.0);
        let ada = principal("ada");
        let start = Instant::now();

        assert_eq!(limiter.admit_at(&ada, start), Ok(()));
        assert_eq!(limiter.admit_at(&ada, start), Err(2));
        assert_eq!(
            limiter.admit_at(&ada, start + Duration::from_secs(1)),
            Err(1)
        );
        assert_eq!(
            limiter.admit_at(&ada, start + Duration::from_secs(2)),
            Ok(())
        );
    }

    #[test]
    fn forgets_a_bucket_that_has_refilled() {
        let limiter = limiter(1.0, 2.0);
        let start = Instant::now();
        let _ = limiter.admit_at(&principal("ada"), start);

        let _ = limiter.admit_at(&principal("grace"), start + Duration::from_secs(10));

        let buckets = lock(&limiter.state);
        assert_eq!(buckets.by_principal.len(), 1);
        assert!(buckets.by_principal.contains_key(&principal("grace")));
    }

    #[test]
    fn counts_requests_in_flight_and_gives_a_place_back() {
        let counter = InFlight::new(NonZeroUsize::new(2).unwrap());

        let first = counter.enter();
        let second = counter.enter();
        assert!(first.is_some() && second.is_some());
        assert!(counter.enter().is_none());
        drop(first);
        assert!(counter.enter().is_some());
    }

    #[test]
    fn exempts_only_routes_the_router_has() {
        let routed: Vec<&str> = ROUTES
            .iter()
            .chain(RUNNER_ROUTES)
            .map(|(path, _)| *path)
            .collect();
        for exempt in OWN_BOUND {
            assert!(routed.contains(exempt), "{exempt} is not a route");
        }
    }

    #[test]
    fn exempts_the_claim_the_landing_and_the_result_from_the_timeout() {
        assert_eq!(
            OWN_BOUND,
            [
                "/v1/runner/jobs/claim",
                "/v1/runner/jobs/{id}/result",
                "/v1/orgs/{org}/repos/{repo}/drafts/{draft}/land",
            ]
        );
    }
}
