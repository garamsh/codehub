//! Organisations and repositories.

use std::sync::Arc;

use app::port::driving::repositories::Repositories;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};

use crate::Services;
use crate::extract::{Authenticated, Body, Params, parse};
use crate::problem::Problem;

#[derive(Deserialize)]
pub struct NewName {
    name: String,
}

#[derive(Serialize)]
pub struct Created {
    name: String,
    /// The repository's identity, which a decision point may name it by; an organisation has none.
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<u64>,
}

pub async fn create_organisation<P: Services>(
    State(ports): State<Arc<P>>,
    Authenticated(caller): Authenticated,
    Body(new): Body<NewName>,
) -> Result<(StatusCode, Json<Created>), Problem> {
    let name = parse("organisation name", &new.name)?;
    ports
        .repositories()
        .create_organisation(&caller, &name)
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(Created {
            name: new.name,
            id: None,
        }),
    ))
}

pub async fn create_repository<P: Services>(
    State(ports): State<Arc<P>>,
    Authenticated(caller): Authenticated,
    Params(organisation): Params<String>,
    Body(new): Body<NewName>,
) -> Result<(StatusCode, Json<Created>), Problem> {
    let organisation = parse("organisation name", &organisation)?;
    let name = parse("repository name", &new.name)?;
    let id = ports
        .repositories()
        .create_repository(&caller, &organisation, &name)
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(Created {
            name: new.name,
            id: Some(id.value()),
        }),
    ))
}
