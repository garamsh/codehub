//! The token endpoint of the image registry (ADR 0053): the Docker registry token authentication
//! a client follows after the registry's 401, answered with a token the registry trusts.

use std::sync::Arc;

use app::domain::credential::Credential;
use app::domain::principal::Caller;
use app::port::driven::identity_verification::IdentityError;
use app::port::driving::authentication::AuthenticationError;
use app::port::driving::registry_token::{RegistryToken, RegistryTokenRequest};
use axum::Json;
use axum::extract::{FromRequestParts, State};
use axum::http::request::Parts;
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::json;

use crate::ApiState;
use crate::extract::Queried;
use crate::problem::Problem;

/// The path of the token endpoint.
pub const PATH: &str = "/v1/registry/token";

/// The token service and the caller its request is made by.
pub struct Registered {
    service: Arc<dyn RegistryToken>,
    caller: Caller,
}

/// The credential a request carries: a bearer token, or the password of Basic credentials, whose
/// user name is ignored.
fn credential(parts: &Parts) -> Result<Credential, Problem> {
    let refused = |detail| Problem::unauthorised_for_registry(detail);
    let header = parts
        .headers
        .get(header::AUTHORIZATION)
        .ok_or_else(|| refused("a bearer token or Basic credentials are required"))?;
    let (scheme, value) = header
        .to_str()
        .ok()
        .and_then(|value| value.split_once(' '))
        .ok_or_else(|| refused("the authorization header is not a bearer token or Basic"))?;
    let token = if scheme.eq_ignore_ascii_case("bearer") {
        value.to_owned()
    } else if scheme.eq_ignore_ascii_case("basic") {
        let decoded = STANDARD
            .decode(value.trim())
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .ok_or_else(|| refused("the Basic credentials are not valid"))?;
        let (_user, password) = decoded
            .split_once(':')
            .ok_or_else(|| refused("the Basic credentials are not valid"))?;
        password.to_owned()
    } else {
        return Err(refused(
            "the authorization header is not a bearer token or Basic",
        ));
    };
    token.parse().map_err(|_| refused("the token is empty"))
}

impl FromRequestParts<Arc<ApiState>> for Registered {
    type Rejection = Problem;

    async fn from_request_parts(parts: &mut Parts, ports: &Arc<ApiState>) -> Result<Self, Problem> {
        let service = ports.registry_token.clone().ok_or_else(unconfigured)?;
        let credential = credential(parts)?;
        let caller = ports
            .authentication
            .authenticate(&credential)
            .await
            .map_err(|error| match error {
                AuthenticationError::Identity(IdentityError::Rejected) => {
                    Problem::unauthorised_for_registry("the token was rejected")
                }
                error => error.into(),
            })?;
        Ok(Self { service, caller })
    }
}

fn unconfigured() -> Problem {
    Problem::not_found("this server does not issue registry tokens")
}

/// `GET /v1/registry/token`: a token carrying what the caller is allowed on the scopes asked.
pub async fn token(
    registered: Registered,
    Queried(query): Queried<Vec<(String, String)>>,
) -> Result<Json<serde_json::Value>, Problem> {
    let mut service = None;
    let mut scopes = Vec::new();
    for (name, value) in query {
        match name.as_str() {
            "service" if service.is_none() => service = Some(value),
            "scope" => scopes.push(value),
            _ => {}
        }
    }
    let service = service.ok_or_else(|| Problem::bad_request("the service is required"))?;
    let grant = registered
        .service
        .issue(
            &registered.caller,
            &RegistryTokenRequest { service, scopes },
        )
        .await?;
    Ok(Json(json!({
        "token": grant.token,
        "access_token": grant.token,
        "expires_in": grant.expires_in,
        "issued_at": grant.issued_at,
    })))
}

/// Any other method: refused, naming the one the endpoint takes. The OAuth2 `POST` of the Docker
/// client lands here and the client falls back to `GET`.
pub async fn not_get(State(ports): State<Arc<ApiState>>) -> Response {
    if ports.registry_token.is_none() {
        return unconfigured().into_response();
    }
    let mut response = Problem::method_not_allowed("the endpoint takes GET").into_response();
    response
        .headers_mut()
        .insert(header::ALLOW, HeaderValue::from_static("GET"));
    response
}
