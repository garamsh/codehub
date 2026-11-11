//! The HTTP JSON API (ADR 0015, ADR 0024): a router that translates requests to the driving ports
//! and their answers back. It holds no subject matter, binds no socket and reads no
//! configuration.

mod extract;
mod members;
mod problem;
mod repositories;
mod source_control;

use std::sync::Arc;

use app::port::driving::authentication::Authentication;
use app::port::driving::members::Members;
use app::port::driving::repositories::Repositories;
use app::port::driving::source_control::SourceControl;
use axum::Router;
use axum::http::header;
use axum::response::IntoResponse;
use axum::routing;

use crate::problem::Problem;

/// The OpenAPI 3.1 document of the API, served at `/v1/openapi.json`.
pub const OPENAPI: &str = include_str!("../openapi.json");

/// The driving ports one router serves.
trait Services: Send + Sync + 'static {
    type Authentication: Authentication + Send + Sync;
    type Repositories: Repositories + Send + Sync;
    type Members: Members + Send + Sync;
    type SourceControl: SourceControl + Send + Sync;

    fn authentication(&self) -> &Self::Authentication;
    fn repositories(&self) -> &Self::Repositories;
    fn members(&self) -> &Self::Members;
    fn source_control(&self) -> &Self::SourceControl;
}

struct Ports<A, R, M, S> {
    authentication: A,
    repositories: R,
    members: M,
    source_control: S,
}

impl<A, R, M, S> Services for Ports<A, R, M, S>
where
    A: Authentication + Send + Sync + 'static,
    R: Repositories + Send + Sync + 'static,
    M: Members + Send + Sync + 'static,
    S: SourceControl + Send + Sync + 'static,
{
    type Authentication = A;
    type Repositories = R;
    type Members = M;
    type SourceControl = S;

    fn authentication(&self) -> &A {
        &self.authentication
    }

    fn repositories(&self) -> &R {
        &self.repositories
    }

    fn members(&self) -> &M {
        &self.members
    }

    fn source_control(&self) -> &S {
        &self.source_control
    }
}

/// Declares the routes once, as both the router and [`ROUTES`], so the two cannot differ.
macro_rules! routes {
    ($($path:literal => $first:ident($first_handler:expr) $(, $rest:ident($rest_handler:expr))*;)+) => {
        /// Every route of the router: its path template and the lowercase names of its methods.
        pub const ROUTES: &[(&str, &[&str])] = &[
            $(($path, &[stringify!($first) $(, stringify!($rest))*])),+
        ];

        fn routed<P: Services>() -> Router<Arc<P>> {
            Router::new()$(
                .route($path, routing::$first($first_handler)$(.$rest($rest_handler))*)
            )+
        }
    };
}

routes! {
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
