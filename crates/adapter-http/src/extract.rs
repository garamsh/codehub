//! What the handlers take from a request: the caller, the path parameters, the query and the
//! JSON body, each parsed at this boundary into the application's types.

use std::fmt::Display;
use std::str::FromStr;
use std::sync::Arc;

use app::domain::credential::Credential;
use app::domain::path::{Path, Prefix};
use app::domain::principal::{Caller, Principal};
use app::domain::repository::RepositoryId;
use app::port::driving::authentication::Authentication;
use app::port::driving::repositories::Repositories;
use axum::body::Bytes;
use axum::extract::{FromRequest, FromRequestParts, Query, Request};
use axum::http::header;
use axum::http::request::Parts;
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::problem::Problem;
use crate::{Authenticates, Locates};

/// The caller the request's bearer token identifies.
pub struct Authenticated(pub Caller);

impl<P: Authenticates> FromRequestParts<Arc<P>> for Authenticated {
    type Rejection = Problem;

    async fn from_request_parts(parts: &mut Parts, ports: &Arc<P>) -> Result<Self, Problem> {
        let header = parts
            .headers
            .get(header::AUTHORIZATION)
            .ok_or_else(|| Problem::unauthorised("a bearer token is required"))?;
        let token = header
            .to_str()
            .ok()
            .and_then(|value| value.split_once(' '))
            .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer"))
            .map(|(_, token)| token)
            .ok_or_else(|| {
                Problem::unauthorised("the authorization header is not a bearer token")
            })?;
        let credential: Credential = token
            .parse()
            .map_err(|_| Problem::unauthorised("the bearer token is empty"))?;
        Ok(Self(
            ports.authentication().authenticate(&credential).await?,
        ))
    }
}

/// The path parameters, deserialised.
pub struct Params<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned + Send> FromRequestParts<S> for Params<T> {
    type Rejection = Problem;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Problem> {
        axum::extract::Path::<T>::from_request_parts(parts, state)
            .await
            .map(|axum::extract::Path(params)| Self(params))
            .map_err(|rejection| Problem::bad_request(rejection.body_text()))
    }
}

/// The query string, deserialised.
pub struct Queried<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned> FromRequestParts<S> for Queried<T> {
    type Rejection = Problem;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Problem> {
        Query::<T>::from_request_parts(parts, state)
            .await
            .map(|Query(query)| Self(query))
            .map_err(|rejection| Problem::bad_request(rejection.body_text()))
    }
}

/// The request body, parsed as JSON.
pub struct Body<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned> FromRequest<S> for Body<T> {
    type Rejection = Problem;

    async fn from_request(request: Request, state: &S) -> Result<Self, Problem> {
        let bytes = Bytes::from_request(request, state)
            .await
            .map_err(|rejection| Problem::bad_request(rejection.body_text()))?;
        serde_json::from_slice(&bytes)
            .map(Self)
            .map_err(|error| Problem::bad_request(format!("the body is not valid: {error}")))
    }
}

/// Parses `text` as a `what`.
pub fn parse<T>(what: &str, text: &str) -> Result<T, Problem>
where
    T: FromStr,
    T::Err: Display,
{
    text.parse()
        .map_err(|error| Problem::bad_request(format!("not a {what}: {error}")))
}

/// The repository `organisation`/`repository`, which `caller` may read.
pub async fn find<P: Locates>(
    ports: &P,
    caller: &Caller,
    organisation: &str,
    repository: &str,
) -> Result<RepositoryId, Problem> {
    Ok(ports
        .repositories()
        .find_repository(
            caller,
            &parse("organisation name", organisation)?,
            &parse("repository name", repository)?,
        )
        .await?)
}

/// Parses a path prefix; the empty text is the whole repository.
pub fn parse_prefix(text: &str) -> Result<Prefix, Problem> {
    if text.is_empty() {
        Ok(Prefix::whole())
    } else {
        Ok(Prefix::from(parse::<Path>("path", text)?))
    }
}

/// An issuer and a subject, as a request names a principal.
#[derive(Deserialize)]
pub struct PrincipalBody {
    issuer: String,
    subject: String,
}

impl PrincipalBody {
    /// The principal named.
    pub fn principal(&self) -> Result<Principal, Problem> {
        Principal::new(&self.issuer, &self.subject)
            .map_err(|error| Problem::bad_request(format!("not a principal: {error}")))
    }
}
