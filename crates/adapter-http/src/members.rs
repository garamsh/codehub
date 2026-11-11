//! Members of an organisation and their grants.

use std::sync::Arc;

use app::port::driven::membership::{Access, Role};
use app::port::driving::members::Members;
use axum::extract::State;
use axum::http::StatusCode;
use serde::Deserialize;

use crate::Services;
use crate::extract::{Authenticated, Body, Params, PrincipalBody, Queried, parse, parse_prefix};
use crate::problem::Problem;

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RoleBody {
    Admin,
    Member,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AccessBody {
    Read,
    Write,
}

#[derive(Deserialize)]
pub struct MemberBody {
    #[serde(flatten)]
    principal: PrincipalBody,
    role: RoleBody,
}

#[derive(Deserialize)]
pub struct GrantBody {
    #[serde(flatten)]
    principal: PrincipalBody,
    prefix: String,
    access: AccessBody,
}

pub async fn add_member<P: Services>(
    State(ports): State<Arc<P>>,
    Authenticated(caller): Authenticated,
    Params(organisation): Params<String>,
    Body(member): Body<MemberBody>,
) -> Result<StatusCode, Problem> {
    let organisation = parse("organisation name", &organisation)?;
    let role = match member.role {
        RoleBody::Admin => Role::Admin,
        RoleBody::Member => Role::Member,
    };
    ports
        .members()
        .add_member(&caller, &organisation, &member.principal.principal()?, role)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn remove_member<P: Services>(
    State(ports): State<Arc<P>>,
    Authenticated(caller): Authenticated,
    Params(organisation): Params<String>,
    Queried(member): Queried<PrincipalBody>,
) -> Result<StatusCode, Problem> {
    let organisation = parse("organisation name", &organisation)?;
    ports
        .members()
        .remove_member(&caller, &organisation, &member.principal()?)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn add_grant<P: Services>(
    State(ports): State<Arc<P>>,
    Authenticated(caller): Authenticated,
    Params((organisation, repository)): Params<(String, String)>,
    Body(grant): Body<GrantBody>,
) -> Result<StatusCode, Problem> {
    let organisation = parse("organisation name", &organisation)?;
    let repository = parse("repository name", &repository)?;
    ports
        .members()
        .add_grant(
            &caller,
            &organisation,
            &grant.principal.principal()?,
            &repository,
            parse_prefix(&grant.prefix)?,
            access(&grant.access),
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn remove_grant<P: Services>(
    State(ports): State<Arc<P>>,
    Authenticated(caller): Authenticated,
    Params((organisation, repository)): Params<(String, String)>,
    Queried(grant): Queried<GrantBody>,
) -> Result<StatusCode, Problem> {
    let organisation = parse("organisation name", &organisation)?;
    let repository = parse("repository name", &repository)?;
    ports
        .members()
        .remove_grant(
            &caller,
            &organisation,
            &grant.principal.principal()?,
            &repository,
            parse_prefix(&grant.prefix)?,
            access(&grant.access),
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

const fn access(body: &AccessBody) -> Access {
    match body {
        AccessBody::Read => Access::Read,
        AccessBody::Write => Access::Write,
    }
}
