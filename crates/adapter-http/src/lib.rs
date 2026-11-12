//! The HTTP JSON API (ADR 0015, ADR 0024): a router that translates requests to the driving ports
//! and their answers back. It holds no subject matter, binds no socket and reads no
//! configuration.

mod checks;
mod extract;
mod jobs;
mod members;
mod problem;
mod repositories;
mod runners;
mod source_control;

use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use app::port::driving::authentication::Authentication;
use app::port::driving::checks::Checks;
use app::port::driving::jobs::Jobs;
use app::port::driving::members::Members;
use app::port::driving::repositories::Repositories;
use app::port::driving::runners::Runners;
use app::port::driving::source_control::SourceControl;
use axum::Router;
use axum::http::header;
use axum::response::IntoResponse;
use axum::routing;

use crate::problem::Problem;

/// The OpenAPI 3.1 document of the API, served at `/v1/openapi.json`.
pub const OPENAPI: &str = include_str!("../openapi.json");

/// What every route needs: the port that identifies the caller.
trait Authenticates: Send + Sync + 'static {
    type Authentication: Authentication + Send + Sync;

    fn authentication(&self) -> &Self::Authentication;
}

/// What a route that names a repository needs: the port that finds it.
trait Locates: Send + Sync + 'static {
    type Repositories: Repositories + Send + Sync;

    fn repositories(&self) -> &Self::Repositories;
}

/// The driving ports the router of the source-control API serves.
trait Services: Authenticates + Locates {
    type Members: Members + Send + Sync;
    type SourceControl: SourceControl + Send + Sync;

    fn members(&self) -> &Self::Members;
    fn source_control(&self) -> &Self::SourceControl;
}

/// The driving ports the router of the runner API serves.
trait RunnerServices: Authenticates + Locates {
    type Runners: Runners + Send + Sync;
    type Jobs: Jobs + Send + Sync;
    type Checks: Checks + Send + Sync;

    fn runners(&self) -> &Self::Runners;
    fn jobs(&self) -> &Self::Jobs;
    fn checks(&self) -> &Self::Checks;
    fn settings(&self) -> &RunnerSettings;
}

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

struct Ports<A, R, M, S> {
    authentication: A,
    repositories: R,
    members: M,
    source_control: S,
}

impl<A, R, M, S> Authenticates for Ports<A, R, M, S>
where
    A: Authentication + Send + Sync + 'static,
    R: Send + Sync + 'static,
    M: Send + Sync + 'static,
    S: Send + Sync + 'static,
{
    type Authentication = A;

    fn authentication(&self) -> &A {
        &self.authentication
    }
}

impl<A, R, M, S> Locates for Ports<A, R, M, S>
where
    A: Send + Sync + 'static,
    R: Repositories + Send + Sync + 'static,
    M: Send + Sync + 'static,
    S: Send + Sync + 'static,
{
    type Repositories = R;

    fn repositories(&self) -> &R {
        &self.repositories
    }
}

impl<A, R, M, S> Services for Ports<A, R, M, S>
where
    A: Authentication + Send + Sync + 'static,
    R: Repositories + Send + Sync + 'static,
    M: Members + Send + Sync + 'static,
    S: SourceControl + Send + Sync + 'static,
{
    type Members = M;
    type SourceControl = S;

    fn members(&self) -> &M {
        &self.members
    }

    fn source_control(&self) -> &S {
        &self.source_control
    }
}

struct RunnerPorts<A, R, N, J, C> {
    authentication: A,
    repositories: R,
    runners: N,
    jobs: J,
    checks: C,
    settings: RunnerSettings,
}

impl<A, R, N, J, C> Authenticates for RunnerPorts<A, R, N, J, C>
where
    A: Authentication + Send + Sync + 'static,
    R: Send + Sync + 'static,
    N: Send + Sync + 'static,
    J: Send + Sync + 'static,
    C: Send + Sync + 'static,
{
    type Authentication = A;

    fn authentication(&self) -> &A {
        &self.authentication
    }
}

impl<A, R, N, J, C> Locates for RunnerPorts<A, R, N, J, C>
where
    A: Send + Sync + 'static,
    R: Repositories + Send + Sync + 'static,
    N: Send + Sync + 'static,
    J: Send + Sync + 'static,
    C: Send + Sync + 'static,
{
    type Repositories = R;

    fn repositories(&self) -> &R {
        &self.repositories
    }
}

impl<A, R, N, J, C> RunnerServices for RunnerPorts<A, R, N, J, C>
where
    A: Authentication + Send + Sync + 'static,
    R: Repositories + Send + Sync + 'static,
    N: Runners + Send + Sync + 'static,
    J: Jobs + Send + Sync + 'static,
    C: Checks + Send + Sync + 'static,
{
    type Runners = N;
    type Jobs = J;
    type Checks = C;

    fn runners(&self) -> &N {
        &self.runners
    }

    fn jobs(&self) -> &J {
        &self.jobs
    }

    fn checks(&self) -> &C {
        &self.checks
    }

    fn settings(&self) -> &RunnerSettings {
        &self.settings
    }
}

/// Declares a router's routes once, as both the router and its list of routes, so the two cannot
/// differ.
macro_rules! routes {
    (
        $list:ident, $routed:ident, $services:ident;
        $($path:literal => $first:ident($first_handler:expr) $(, $rest:ident($rest_handler:expr))*;)+
    ) => {
        /// Every route of the router: its path template and the lowercase names of its methods.
        pub const $list: &[(&str, &[&str])] = &[
            $(($path, &[stringify!($first) $(, stringify!($rest))*])),+
        ];

        fn $routed<P: $services>() -> Router<Arc<P>> {
            Router::new()$(
                .route($path, routing::$first($first_handler)$(.$rest($rest_handler))*)
            )+
        }
    };
}

routes! {
    ROUTES, routed, Services;
    "/v1/openapi.json" => get(openapi);
    "/v1/orgs" => post(repositories::create_organisation::<P>);
    "/v1/orgs/{org}/repos" => post(repositories::create_repository::<P>);
    "/v1/orgs/{org}/members" =>
        put(members::add_member::<P>), delete(members::remove_member::<P>);
    "/v1/orgs/{org}/repos/{repo}/grants" =>
        put(members::add_grant::<P>), delete(members::remove_grant::<P>);
    "/v1/orgs/{org}/repos/{repo}/uploads" => post(source_control::request_upload::<P>);
    "/v1/orgs/{org}/repos/{repo}/drafts/{draft}/files/{*path}" =>
        put(source_control::stage_file::<P>), delete(source_control::stage_deletion::<P>);
    "/v1/orgs/{org}/repos/{repo}/drafts/{draft}/land" => post(source_control::land::<P>);
    "/v1/orgs/{org}/repos/{repo}/files/{*path}" => get(source_control::read_file::<P>);
    "/v1/orgs/{org}/repos/{repo}/history/{*path}" => get(source_control::history::<P>);
}

routes! {
    RUNNER_ROUTES, runner_routed, RunnerServices;
    "/v1/orgs/{org}/runners" => post(runners::register::<P>), get(runners::list::<P>);
    "/v1/orgs/{org}/runners/{id}" => delete(runners::remove::<P>);
    "/v1/runner/jobs/claim" => post(jobs::claim::<P>);
    "/v1/runner/jobs/{id}/heartbeat" => post(jobs::heartbeat::<P>);
    "/v1/runner/jobs/{id}/result" => post(jobs::report::<P>);
    "/v1/orgs/{org}/repos/{repo}/changesets/{id}/checks" => get(checks::list::<P>);
    "/v1/orgs/{org}/repos/{repo}/jobs/{id}/cancel" => post(checks::cancel::<P>);
    "/v1/orgs/{org}/repos/{repo}/refs/{name}" => get(checks::reference::<P>);
}

async fn openapi() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "application/json")], OPENAPI)
}

async fn no_route() -> Problem {
    Problem::no_route()
}

/// The router serving the API over these driving ports.
///
/// The router owns the ports for as long as it lives and shares them between requests, so they
/// are `'static`. A service that borrows its driven ports is made `'static` by the caller, by
/// leaking the driven ports it borrows, for instance.
pub fn router<A, R, M, S>(
    authentication: A,
    repositories: R,
    members: M,
    source_control: S,
) -> Router
where
    A: Authentication + Send + Sync + 'static,
    R: Repositories + Send + Sync + 'static,
    M: Members + Send + Sync + 'static,
    S: SourceControl + Send + Sync + 'static,
{
    routed().fallback(no_route).with_state(Arc::new(Ports {
        authentication,
        repositories,
        members,
        source_control,
    }))
}

/// The router serving the runner API over these driving ports: registrations, the runner's job
/// conversation, and the checks and refs of a repository.
///
/// It routes only what [`RUNNER_ROUTES`] lists and has no fallback, so a caller merges it into
/// [`router`]'s, which answers a request no route matches. `settings` says how a claim is held.
pub fn runner_router<A, R, N, J, C>(
    authentication: A,
    repositories: R,
    runners: N,
    jobs: J,
    checks: C,
    settings: RunnerSettings,
) -> Router
where
    A: Authentication + Send + Sync + 'static,
    R: Repositories + Send + Sync + 'static,
    N: Runners + Send + Sync + 'static,
    J: Jobs + Send + Sync + 'static,
    C: Checks + Send + Sync + 'static,
{
    runner_routed().with_state(Arc::new(RunnerPorts {
        authentication,
        repositories,
        runners,
        jobs,
        checks,
        settings,
    }))
}
