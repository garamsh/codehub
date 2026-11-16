//! The HTTP JSON API (ADR 0015, ADR 0024): a router that translates requests to the driving ports
//! and their answers back. It holds no subject matter, binds no socket and reads no
//! configuration.

mod checks;
mod extract;
mod health;
mod jobs;
mod members;
mod overload;
mod problem;
mod repositories;
mod runners;
mod runs;
mod source_control;
mod workspace;

use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use app::port::driving::authentication::Authentication;
use app::port::driving::checks::Checks;
use app::port::driving::jobs::Jobs;
use app::port::driving::members::Members;
use app::port::driving::repositories::Repositories;
use app::port::driving::runners::Runners;
use app::port::driving::runs::Runs;
use app::port::driving::source_control::SourceControl;
use async_trait::async_trait;
use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::http::header;
use axum::response::IntoResponse;
use axum::routing;

use crate::problem::Problem;

pub use crate::overload::{InFlight, Limits, RateLimiter};

/// The OpenAPI 3.1 document of the API, served at `/v1/openapi.json`.
pub const OPENAPI: &str = include_str!("../openapi.json");

/// The dependencies the readiness probe asks about.
#[async_trait]
pub trait Readiness: Send + Sync {
    /// Whether every dependency of the server answers now.
    ///
    /// # Errors
    ///
    /// [`NotReady`] naming the dependency that did not answer.
    async fn check(&self) -> Result<(), NotReady>;
}

/// Why the server is not ready: the short reason `/readyz` answers 503 with.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct NotReady(pub &'static str);

/// A wait for a [`Duration`], which the composition site supplies from its runtime: this crate
/// names no timer of its own.
pub type Pause = Pin<Box<dyn Future<Output = ()> + Send>>;

/// How the runner API holds a claim that finds no job, and what it tells a runner to do.
#[derive(Clone, Copy)]
pub struct RunnerSettings {
    /// How long a claim is held before it is answered with no job.
    pub hold: Duration,
    /// How long to wait between attempts to claim while holding.
    pub retry_interval: Duration,
    /// How often a runner is told to heartbeat, in whole seconds. The jobs service's lease is
    /// longer.
    pub heartbeat_interval: Duration,
    /// Waits for the duration given.
    pub pause: fn(Duration) -> Pause,
}

/// What the router serves: every driving port it translates requests to, the settings of the
/// runner API and the readiness probe. The router owns the state for as long as it lives and
/// shares it between requests.
pub struct ApiState {
    /// Identifies the caller of every route but the probes.
    pub authentication: Arc<dyn Authentication>,
    /// Finds the repositories, and creates them and the organisations.
    pub repositories: Arc<dyn Repositories>,
    /// Administers members and their grants.
    pub members: Arc<dyn Members>,
    /// Serves the drafts, the landing and the reads of source control.
    pub source_control: Arc<dyn SourceControl>,
    /// Registers and lists runners.
    pub runners: Arc<dyn Runners>,
    /// Serves the runner's job conversation.
    pub jobs: Arc<dyn Jobs>,
    /// Serves the checks and refs of a repository.
    pub checks: Arc<dyn Checks>,
    /// Starts, reads and cancels the runs of a workspace.
    pub runs: Arc<dyn Runs>,
    /// How a runner's claim is held.
    pub settings: RunnerSettings,
    /// Asked on every request to `/readyz`.
    pub readiness: Arc<dyn Readiness>,
    /// The limits of this process on how much it is asked (ADR 0055).
    pub limits: Limits,
}

/// Declares a router's routes once, as both the router and its list of routes, so the two cannot
/// differ.
macro_rules! routes {
    (
        $list:ident, $routed:ident, $state:ty;
        $($path:literal => $first:ident($first_handler:expr) $(, $rest:ident($rest_handler:expr))*;)+
    ) => {
        /// Every route of the router: its path template and the lowercase names of its methods.
        pub const $list: &[(&str, &[&str])] = &[
            $(($path, &[stringify!($first) $(, stringify!($rest))*])),+
        ];

        fn $routed() -> Router<$state> {
            Router::new()$(
                .route($path, routing::$first($first_handler)$(.$rest($rest_handler))*)
            )+
        }
    };
}

routes! {
    ROUTES, routed, Arc<ApiState>;
    "/v1/openapi.json" => get(openapi);
    "/v1/orgs" => post(repositories::create_organisation);
    "/v1/orgs/{org}/repos" => post(repositories::create_repository);
    "/v1/orgs/{org}/members" =>
        put(members::add_member), delete(members::remove_member);
    "/v1/orgs/{org}/repos/{repo}/grants" =>
        put(members::add_grant), delete(members::remove_grant);
    "/v1/orgs/{org}/repos/{repo}/uploads" => post(source_control::request_upload);
    "/v1/orgs/{org}/repos/{repo}/drafts/{draft}/files/{*path}" =>
        put(source_control::stage_file), delete(source_control::stage_deletion);
    "/v1/orgs/{org}/repos/{repo}/drafts/{draft}" =>
        put(workspace::open), get(workspace::read);
    "/v1/orgs/{org}/repos/{repo}/drafts/{draft}/description" => put(workspace::describe);
    "/v1/orgs/{org}/repos/{repo}/drafts/{draft}/snapshot" => post(workspace::snapshot);
    "/v1/orgs/{org}/repos/{repo}/drafts/{draft}/changes/{*path}" =>
        delete(workspace::restore);
    "/v1/orgs/{org}/repos/{repo}/drafts/{draft}/diff" => get(workspace::diff);
    "/v1/orgs/{org}/repos/{repo}/drafts/{draft}/land" => post(source_control::land);
    "/v1/orgs/{org}/repos/{repo}/changesets/{id}/files" => get(workspace::list_files);
    "/v1/orgs/{org}/repos/{repo}/files/{*path}" => get(source_control::read_file);
    "/v1/orgs/{org}/repos/{repo}/history/{*path}" => get(source_control::history);
    "/v1/orgs/{org}/runners" => post(runners::register), get(runners::list);
    "/v1/orgs/{org}/runners/{id}" => delete(runners::remove);
    "/v1/orgs/{org}/repos/{repo}/changesets/{id}/checks" => get(checks::list);
    "/v1/orgs/{org}/repos/{repo}/jobs/{id}/cancel" => post(checks::cancel);
    "/v1/orgs/{org}/repos/{repo}/refs/{name}" => get(checks::reference);
    "/v1/orgs/{org}/repos/{repo}/drafts/{draft}/checks/plan" => get(checks::plan);
    "/v1/orgs/{org}/repos/{repo}/drafts/{draft}/runs" => post(runs::start);
    "/v1/orgs/{org}/repos/{repo}/drafts/{draft}/runs/latest" => get(runs::latest);
    "/v1/orgs/{org}/repos/{repo}/runs/{id}" => get(runs::get);
    "/v1/orgs/{org}/repos/{repo}/runs/{id}/cancel" => post(runs::cancel);
    "/v1/orgs/{org}/repos/{repo}/runs/{id}/files" => get(runs::files);
}

routes! {
    RUNNER_ROUTES, runner_routed, Arc<ApiState>;
    "/v1/runner/jobs/claim" => post(jobs::claim);
    "/v1/runner/jobs/{id}/heartbeat" => post(jobs::heartbeat);
    "/v1/runner/jobs/{id}/result" => post(jobs::report);
    "/v1/runner/jobs/{id}/uploads" => post(jobs::uploads);
}

routes! {
    HEALTH_ROUTES, health_routed, Arc<dyn Readiness>;
    "/healthz" => get(health::live);
    "/readyz" => get(health::ready);
}

async fn openapi() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "application/json")], OPENAPI)
}

async fn no_route() -> Problem {
    Problem::no_route()
}

/// A server with no dependency to ask about, which is always ready.
#[async_trait]
impl Readiness for () {
    async fn check(&self) -> Result<(), NotReady> {
        Ok(())
    }
}

/// `routes` over `state`, with the probes over the state's readiness, and a fallback for a request
/// no route matches.
fn serve(routes: Router<Arc<ApiState>>, state: ApiState) -> Router {
    let readiness = Arc::clone(&state.readiness);
    let body_bytes = state.limits.body_bytes;
    let state = Arc::new(state);
    routes
        .fallback(no_route)
        .layer(DefaultBodyLimit::max(body_bytes))
        .layer(axum::middleware::from_fn_with_state(
            Arc::clone(&state),
            overload::bound,
        ))
        .with_state(state)
        .merge(health_routed().with_state(readiness))
}

/// The router serving the whole API over `state`: every route of [`ROUTES`], [`RUNNER_ROUTES`]
/// and [`HEALTH_ROUTES`], and a fallback for a request no route matches.
pub fn router(state: ApiState) -> Router {
    serve(routed().merge(runner_routed()), state)
}

/// The router of the user and agent API: every route of [`ROUTES`] and [`HEALTH_ROUTES`], and a
/// fallback for a request no route matches, which `/v1/runner/` is.
pub fn api_router(state: ApiState) -> Router {
    serve(routed(), state)
}

/// The router of the runner API: every route of [`RUNNER_ROUTES`] and [`HEALTH_ROUTES`], and a
/// fallback for a request no route matches.
pub fn runner_router(state: ApiState) -> Router {
    serve(runner_routed(), state)
}

/// The router of a process that serves only the probes of [`HEALTH_ROUTES`] over `readiness`,
/// with a fallback for a request no route matches.
pub fn health_router(readiness: Arc<dyn Readiness>) -> Router {
    health_routed().fallback(no_route).with_state(readiness)
}
