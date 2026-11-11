use app::domain::name::{OrganisationName, RepositoryName};
use app::domain::repository::RepositoryId;
use app::port::driven::repository_directory::{DirectoryError, RepositoryDirectory};
use sqlx::PgPool;

use crate::conversion;
use crate::database::Database;

/// The repository directory in PostgreSQL; it numbers repositories from 1 in creation order,
/// leaving gaps where a creation lost to an existing name.
pub struct PostgresRepositoryDirectory {
    pool: PgPool,
}

impl PostgresRepositoryDirectory {
    /// A repository directory over `database`.
    #[must_use]
    pub fn new(database: &Database) -> Self {
        Self {
            pool: database.pool.clone(),
        }
    }
}

fn backend(error: impl ToString) -> DirectoryError {
    DirectoryError::Backend(error.to_string())
}

fn identifier(value: i64) -> Result<RepositoryId, DirectoryError> {
    u64::try_from(value).map(RepositoryId::new).map_err(|_| {
        backend(format!(
            "a stored repository identifier {value} is negative"
        ))
    })
}

impl RepositoryDirectory for PostgresRepositoryDirectory {
    async fn create_organisation(&self, name: &OrganisationName) -> Result<(), DirectoryError> {
        let created = sqlx::query(
            "insert into organisations (name) values ($1) on conflict (name) do nothing",
        )
        .bind(name.as_str())
        .execute(&self.pool)
        .await
        .map_err(backend)?
        .rows_affected();
        if created == 0 {
            return Err(DirectoryError::OrganisationExists(name.clone()));
        }
        Ok(())
    }

    async fn create_repository(
        &self,
        organisation: &OrganisationName,
        repository: &RepositoryName,
    ) -> Result<RepositoryId, DirectoryError> {
        let inserted = sqlx::query_scalar::<_, i64>(
            "insert into repositories (organisation, name) values ($1, $2) \
             on conflict (organisation, name) do nothing returning id",
        )
        .bind(organisation.as_str())
        .bind(repository.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| {
            let no_organisation = error
                .as_database_error()
                .is_some_and(|error| error.is_foreign_key_violation());
            if no_organisation {
                DirectoryError::OrganisationNotFound(organisation.clone())
            } else {
                backend(error)
            }
        })?;
        match inserted {
            Some(id) => identifier(id),
            None => Err(DirectoryError::RepositoryExists(
                organisation.clone(),
                repository.clone(),
            )),
        }
    }

    async fn find_repository(
        &self,
        organisation: &OrganisationName,
        repository: &RepositoryName,
    ) -> Result<RepositoryId, DirectoryError> {
        let found = sqlx::query_scalar::<_, i64>(
            "select id from repositories where organisation = $1 and name = $2",
        )
        .bind(organisation.as_str())
        .bind(repository.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(backend)?;
        match found {
            Some(id) => identifier(id),
            None => Err(DirectoryError::RepositoryNotFound(
                organisation.clone(),
                repository.clone(),
            )),
        }
    }

    async fn names_of(
        &self,
        repository: RepositoryId,
    ) -> Result<Option<(OrganisationName, RepositoryName)>, DirectoryError> {
        // An identifier outside the stored range was never assigned.
        let Ok(id) = conversion::repository_column(repository) else {
            return Ok(None);
        };
        let found = sqlx::query_as::<_, (String, String)>(
            "select organisation, name from repositories where id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(backend)?;
        found
            .map(|(organisation, name)| {
                Ok((
                    organisation.parse().map_err(backend)?,
                    name.parse().map_err(backend)?,
                ))
            })
            .transpose()
    }
}
