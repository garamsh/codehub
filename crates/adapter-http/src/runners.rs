//! Runner registrations of an organisation.

use std::collections::BTreeSet;
use std::sync::Arc;

use app::domain::name::{Label, RepositoryName};
use app::domain::runner::{Registration, RegistrationId};
use app::port::driving::runners::Runners;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};

use crate::RunnerServices;
use crate::extract::{Authenticated, Body, Params, PrincipalBody, find, parse};
use crate::problem::Problem;

#[derive(Deserialize)]
pub struct NewRunner {
    #[serde(flatten)]
    principal: PrincipalBody,
    labels: Vec<String>,
    repository: Option<String>,
}

#[derive(Serialize)]
pub struct RegistrationBody {
    id: u64,
    organisation: String,
    repository_id: Option<u64>,
    issuer: String,
    subject: String,
    labels: Vec<String>,
}

#[derive(Serialize)]
pub struct RegistrationsBody {
    runners: Vec<RegistrationBody>,
}

impl From<Registration> for RegistrationBody {
    fn from(registration: Registration) -> Self {
        let registered = registration.registered;
        Self {
            id: registration.id.value(),
            organisation: registered.organisation.to_string(),
            repository_id: registered.repository.map(|repository| repository.value()),
            issuer: registered.principal.issuer().to_owned(),
            subject: registered.principal.subject().to_owned(),
            labels: registered.labels.iter().map(ToString::to_string).collect(),
        }
    }
}

pub async fn register<P: RunnerServices>(
    State(ports): State<Arc<P>>,
    Authenticated(caller): Authenticated,
    Params(organisation): Params<String>,
    Body(new): Body<NewRunner>,
) -> Result<(StatusCode, Json<RegistrationBody>), Problem> {
    let name = parse("organisation name", &organisation)?;
    let labels = new
        .labels
        .iter()
        .map(|label| parse::<Label>("label", label))
        .collect::<Result<BTreeSet<_>, _>>()?;
    let principal = new.principal.principal()?;
    let named = new
        .repository
        .as_deref()
        .map(|repository| parse::<RepositoryName>("repository name", repository))
        .transpose()?;
    let repository = match new.repository.as_deref() {
        Some(repository) => Some(find(&*ports, &caller, &organisation, repository).await?),
        None => None,
    };
    let id = ports
        .runners()
        .register(&caller, &name, named.as_ref(), &principal, labels.clone())
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(RegistrationBody {
            id: id.value(),
            organisation: name.to_string(),
            repository_id: repository.map(|repository| repository.value()),
            issuer: principal.issuer().to_owned(),
            subject: principal.subject().to_owned(),
            labels: labels.iter().map(ToString::to_string).collect(),
        }),
    ))
}

pub async fn list<P: RunnerServices>(
    State(ports): State<Arc<P>>,
    Authenticated(caller): Authenticated,
    Params(organisation): Params<String>,
) -> Result<Json<RegistrationsBody>, Problem> {
    let registrations = ports
        .runners()
        .list(&caller, &parse("organisation name", &organisation)?)
        .await?;
    Ok(Json(RegistrationsBody {
        runners: registrations.into_iter().map(Into::into).collect(),
    }))
}

pub async fn remove<P: RunnerServices>(
    State(ports): State<Arc<P>>,
    Authenticated(caller): Authenticated,
    Params((organisation, id)): Params<(String, u64)>,
) -> Result<StatusCode, Problem> {
    ports
        .runners()
        .remove(
            &caller,
            &parse("organisation name", &organisation)?,
            RegistrationId::new(id),
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
