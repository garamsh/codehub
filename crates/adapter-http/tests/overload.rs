//! The limits on what the API takes on (ADR 0055): the rate of a principal, the size of a body, the
//! time of a request and the requests in flight.

#[cfg(test)]
pub mod common;

use std::num::{NonZeroU32, NonZeroUsize};
use std::sync::Arc;
use std::time::{Duration, Instant};

use adapter_http::{InFlight, Limits, RateLimiter};
use app::domain::credential::Credential;
use app::domain::principal::Caller;
use app::port::driving::authentication::{Authentication, AuthenticationError};
use app::use_case::administration::Administration;
use axum::Router;
use axum::http::{Method, StatusCode};
use common::{Api, Reply, router_limited, send, state};
use serde_json::json;
use tokio::sync::Notify;

const PROBLEM: &str = "application/problem+json";

/// The token whose authentication never answers, standing in for a handler that hangs.
const STALLED: &str = "token-stalled";

/// Authentication that answers as `inner` does, but never for the token `STALLED`, where it tells
/// `entered` that the request has arrived.
struct Stalling {
    inner: Arc<dyn Authentication>,
    entered: Arc<Notify>,
}

#[cfg(test)]
#[async_trait::async_trait]
impl Authentication for Stalling {
    async fn authenticate(&self, credential: &Credential) -> Result<Caller, AuthenticationError> {
        if credential.as_str() == STALLED {
            self.entered.notify_one();
            std::future::pending::<()>().await;
        }
        self.inner.authenticate(credential).await
    }
}

/// Limits no request reaches, but for the one of `rate`, `body_bytes`, `request_timeout` and
/// `max_in_flight` the test lowers.
#[cfg(test)]
fn limits(
    rate: (u32, u32),
    body_bytes: usize,
    request_timeout: Duration,
    max_in_flight: usize,
) -> Limits {
    Limits {
        rate: RateLimiter::new(
            NonZeroU32::new(rate.0).unwrap(),
            NonZeroU32::new(rate.1).unwrap(),
        ),
        in_flight: InFlight::new(NonZeroUsize::new(max_in_flight).unwrap()),
        request_timeout,
        body_bytes,
    }
}

/// A router whose authentication stalls on `STALLED`, over `api`'s adapters, and the notify that
/// fires when a stalled request arrives.
#[cfg(test)]
fn stalling(api: &Api, limits: Limits) -> (Router, Arc<Notify>) {
    let entered = Arc::new(Notify::new());
    let authentication = Arc::new(Stalling {
        inner: state(&api.stores, Administration::BuiltIn).authentication,
        entered: Arc::clone(&entered),
    });
    (router_limited(&api.stores, authentication, limits), entered)
}

/// A list of the runners of `acme`: authenticated, and answered quickly.
#[cfg(test)]
async fn read(router: &Router, token: &str) -> Reply {
    send(
        router.clone(),
        Method::GET,
        "/v1/orgs/acme/runners",
        Some(token),
        None,
    )
    .await
}

#[tokio::test]
async fn answers_too_many_requests_past_the_burst_while_another_principal_is_served() {
    let api = Api::new();
    let (ada, grace) = (api.token("ada"), api.token("grace"));
    let (router, _) = stalling(&api, limits((1, 3), 1 << 20, Duration::from_secs(60), 100));

    for attempt in 1..=3 {
        let within = read(&router, &ada).await;
        assert_ne!(within.status, StatusCode::TOO_MANY_REQUESTS, "{attempt}");
    }
    let refused = read(&router, &ada).await;

    assert_eq!(refused.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(refused.content_type, PROBLEM);
    assert_eq!(refused.retry_after, "1");
    // The control: another principal has a bucket of its own.
    let other = read(&router, &grace).await;
    assert_ne!(other.status, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn answers_unauthorised_before_it_counts_a_request_against_a_rate() {
    let api = Api::new();
    let (router, _) = stalling(&api, limits((1, 1), 1 << 20, Duration::from_secs(60), 100));

    for _ in 0..3 {
        let refused = read(&router, "token-unknown").await;
        assert_eq!(refused.status, StatusCode::UNAUTHORIZED);
    }
}

#[tokio::test]
async fn does_not_rate_limit_a_runner() {
    let api = Api::new();
    let ada = api.token("ada");
    api.repository(&ada, "acme").await;
    let created = api
        .post(
            "/v1/orgs/acme/runners",
            &ada,
            json!({"issuer": "https://id.test", "subject": "r1", "labels": ["linux"]}),
        )
        .await;
    assert_eq!(created.status, StatusCode::CREATED);
    let runner = api.token("r1");
    let (router, _) = stalling(&api, limits((1, 1), 1 << 20, Duration::from_secs(60), 100));

    for _ in 0..3 {
        let heartbeat = send(
            router.clone(),
            Method::POST,
            "/v1/runner/jobs/1/heartbeat",
            Some(&runner),
            Some(json!({})),
        )
        .await;
        assert_ne!(heartbeat.status, StatusCode::TOO_MANY_REQUESTS);
    }
    // The control: the same principal on a user route is limited by the same limiter.
    let first = read(&router, &runner).await;
    let second = read(&router, &runner).await;
    assert_ne!(first.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(second.status, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn answers_payload_too_large_for_a_body_over_the_limit() {
    let api = Api::new();
    let ada = api.token("ada");
    let (router, _) = stalling(&api, limits((100, 100), 64, Duration::from_secs(60), 100));
    let create = |name: String| {
        let (router, ada) = (router.clone(), ada.clone());
        async move {
            send(
                router,
                Method::POST,
                "/v1/orgs",
                Some(&ada),
                Some(json!({ "name": name })),
            )
            .await
        }
    };

    let refused = create("a".repeat(200)).await;
    assert_eq!(refused.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(refused.content_type, PROBLEM);
    // The control: a body under the limit is read.
    let created = create("acme".to_owned()).await;
    assert_eq!(created.status, StatusCode::CREATED);
}

#[tokio::test]
async fn answers_service_unavailable_for_a_request_that_takes_longer_than_the_timeout() {
    let api = Api::new();
    let ada = api.token("ada");
    api.repository(&ada, "acme").await;
    let (router, _) = stalling(
        &api,
        limits((100, 100), 1 << 20, Duration::from_millis(200), 100),
    );

    let started = Instant::now();
    let timed_out = read(&router, STALLED).await;

    assert_eq!(timed_out.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(timed_out.content_type, PROBLEM);
    assert_eq!(timed_out.retry_after, "");
    assert!(started.elapsed() < Duration::from_secs(10));
    // The control: a request that answers in time is not timed out.
    let answered = read(&router, &ada).await;
    assert_eq!(answered.status, StatusCode::OK);
}

#[tokio::test]
async fn leaves_a_held_claim_to_its_own_bound_rather_than_the_request_timeout() {
    let api = Api::new();
    let ada = api.token("ada");
    api.repository(&ada, "acme").await;
    let created = api
        .post(
            "/v1/orgs/acme/runners",
            &ada,
            json!({"issuer": "https://id.test", "subject": "r1", "labels": ["linux"]}),
        )
        .await;
    assert_eq!(created.status, StatusCode::CREATED);
    let runner = api.token("r1");
    // The fixture holds a claim for `common::HOLD`, which is longer than this timeout.
    let timeout = common::HOLD / 3;
    let (router, _) = stalling(&api, limits((100, 100), 1 << 20, timeout, 100));

    let claimed = send(
        router.clone(),
        Method::POST,
        "/v1/runner/jobs/claim",
        Some(&runner),
        Some(json!({})),
    )
    .await;

    assert_eq!(claimed.status, StatusCode::NO_CONTENT);
    // The control: a request of the same duration elsewhere is timed out.
    let stalled = read(&router, STALLED).await;
    assert_eq!(stalled.status, StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn answers_service_unavailable_with_retry_after_when_the_requests_in_flight_are_at_the_limit()
{
    let api = Api::new();
    let ada = api.token("ada");
    api.repository(&ada, "acme").await;
    let (router, entered) = stalling(
        &api,
        limits((100, 100), 1 << 20, Duration::from_secs(60), 1),
    );
    // The control: with nothing in flight the request is served.
    assert_eq!(read(&router, &ada).await.status, StatusCode::OK);
    let held = tokio::spawn({
        let router = router.clone();
        async move { read(&router, STALLED).await.status }
    });
    entered.notified().await;

    let refused = read(&router, &ada).await;

    assert_eq!(refused.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(refused.content_type, PROBLEM);
    assert_eq!(refused.retry_after, "1");
    // The probes are not counted: a process at its limit stays ready.
    let probe = send(router.clone(), Method::GET, "/healthz", None, None).await;
    assert_eq!(probe.status, StatusCode::OK);
    held.abort();
}
