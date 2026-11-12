use app::domain::name::OrganisationName;
use app::domain::principal::Principal;
use app::domain::runner::{NewRegistration, Registration, RegistrationId};
use app::port::driven::runner_registry::{RunnerRegistry, RunnerRegistryError};
use sqlx::PgPool;

use crate::conversion;
use crate::database::Database;

/// The runner registry in PostgreSQL; it numbers registrations from 1 in registration order,
/// leaving gaps where a registration was refused.
pub struct PostgresRunnerRegistry {
    pool: PgPool,
}

impl PostgresRunnerRegistry {
    /// A runner registry over `database`.
    #[must_use]
    pub fn new(database: &Database) -> Self {
        Self {
            pool: database.pool.clone(),
        }
    }
}

fn backend(error: impl ToString) -> RunnerRegistryError {
    RunnerRegistryError::Backend(error.to_string())
}

type Row = (i64, String, Option<i64>, String, String, Vec<String>);

fn registration(row: Row) -> Result<Registration, RunnerRegistryError> {
    let (id, organisation, repository, issuer, subject, labels) = row;
    Ok(Registration {
        id: conversion::registration(id).map_err(backend)?,
        registered: NewRegistration {
            organisation: organisation.parse().map_err(backend)?,
            repository: repository
                .map(conversion::repository)
                .transpose()
                .map_err(backend)?,
            principal: Principal::new(&issuer, &subject).map_err(backend)?,
            labels: conversion::labels(&labels).map_err(backend)?,
        },
    })
}

impl RunnerRegistry for PostgresRunnerRegistry {
    async fn register(
        &self,
        registration: NewRegistration,
    ) -> Result<RegistrationId, RunnerRegistryError> {
        let repository = registration
            .repository
            .map(conversion::repository_column)
            .transpose()
            .map_err(backend)?;
        let id = sqlx::query_scalar::<_, i64>(
            "insert into runner_registrations (organisation, repository, issuer, subject, labels) \
             values ($1, $2, $3, $4, $5) returning id",
        )
        .bind(registration.organisation.as_str())
        .bind(repository)
        .bind(registration.principal.issuer())
        .bind(registration.principal.subject())
        .bind(conversion::labels_column(&registration.labels))
        .fetch_one(&self.pool)
        .await
        .map_err(backend)?;
        conversion::registration(id).map_err(backend)
    }

    async fn remove(
        &self,
        organisation: &OrganisationName,
        id: RegistrationId,
    ) -> Result<(), RunnerRegistryError> {
        let removed =
            sqlx::query("delete from runner_registrations where id = $1 and organisation = $2")
                .bind(conversion::number_column(id.value(), "registration").map_err(backend)?)
                .bind(organisation.as_str())
                .execute(&self.pool)
                .await
                .map_err(backend)?
                .rows_affected();
        if removed == 0 {
            return Err(RunnerRegistryError::NotFound(id));
        }
        Ok(())
    }

    async fn list(
        &self,
        organisation: &OrganisationName,
    ) -> Result<Vec<Registration>, RunnerRegistryError> {
        sqlx::query_as::<_, Row>(
            "select id, organisation, repository, issuer, subject, labels \
             from runner_registrations where organisation = $1 order by id",
        )
        .bind(organisation.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(backend)?
        .into_iter()
        .map(registration)
        .collect()
    }

    async fn of_principal(
        &self,
        principal: &Principal,
    ) -> Result<Vec<Registration>, RunnerRegistryError> {
        sqlx::query_as::<_, Row>(
            "select id, organisation, repository, issuer, subject, labels \
             from runner_registrations where issuer = $1 and subject = $2 order by id",
        )
        .bind(principal.issuer())
        .bind(principal.subject())
        .fetch_all(&self.pool)
        .await
        .map_err(backend)?
        .into_iter()
        .map(registration)
        .collect()
    }
}
