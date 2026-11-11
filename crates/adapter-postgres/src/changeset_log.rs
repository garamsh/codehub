use app::domain::address::Address;
use app::domain::changeset::Changeset;
use app::domain::repository::RepositoryId;
use app::port::driven::changeset_log::{ChangesetLog, ChangesetLogError, Ref};
use sqlx::PgPool;

use crate::conversion;
use crate::database::Database;

/// The changeset log and the refs in PostgreSQL.
///
/// A ref moves in one statement, so two moves of the same ref cannot interleave: the second finds
/// the row the first left and moves nothing.
pub struct PostgresChangesetLog {
    pool: PgPool,
}

impl PostgresChangesetLog {
    /// A changeset log over `database`.
    #[must_use]
    pub fn new(database: &Database) -> Self {
        Self {
            pool: database.pool.clone(),
        }
    }

    async fn current(
        &self,
        repository: i64,
        name: Ref,
    ) -> Result<Option<Address>, ChangesetLogError> {
        sqlx::query_scalar::<_, Vec<u8>>(
            "select target from refs where repository = $1 and name = $2",
        )
        .bind(repository)
        .bind(ref_name(name))
        .fetch_optional(&self.pool)
        .await
        .map_err(backend)?
        .map(|bytes| conversion::address(bytes).map_err(ChangesetLogError::Backend))
        .transpose()
    }
}

fn backend(error: impl ToString) -> ChangesetLogError {
    ChangesetLogError::Backend(error.to_string())
}

fn repository_column(repository: RepositoryId) -> Result<i64, ChangesetLogError> {
    conversion::repository_column(repository).map_err(ChangesetLogError::Backend)
}

const fn ref_name(name: Ref) -> &'static str {
    match name {
        Ref::Main => "main",
    }
}

impl ChangesetLog for PostgresChangesetLog {
    async fn append(
        &self,
        repository: RepositoryId,
        changeset: Changeset,
    ) -> Result<Address, ChangesetLogError> {
        let encoding = changeset.encode().map_err(ChangesetLogError::Encode)?;
        let id = Address::of(&encoding);
        sqlx::query(
            "insert into changesets (repository, id, encoding) values ($1, $2, $3) \
             on conflict (repository, id) do nothing",
        )
        .bind(repository_column(repository)?)
        .bind(id.digest().as_slice())
        .bind(encoding)
        .execute(&self.pool)
        .await
        .map_err(backend)?;
        Ok(id)
    }

    async fn get(
        &self,
        repository: RepositoryId,
        id: Address,
    ) -> Result<Changeset, ChangesetLogError> {
        let encoding = sqlx::query_scalar::<_, Vec<u8>>(
            "select encoding from changesets where repository = $1 and id = $2",
        )
        .bind(repository_column(repository)?)
        .bind(id.digest().as_slice())
        .fetch_optional(&self.pool)
        .await
        .map_err(backend)?
        .ok_or(ChangesetLogError::NotFound(id))?;
        Changeset::decode(&encoding).map_err(backend)
    }

    async fn read_ref(
        &self,
        repository: RepositoryId,
        name: Ref,
    ) -> Result<Option<Address>, ChangesetLogError> {
        self.current(repository_column(repository)?, name).await
    }

    async fn compare_and_swap(
        &self,
        repository: RepositoryId,
        name: Ref,
        expected: Option<Address>,
        new: Address,
    ) -> Result<(), ChangesetLogError> {
        let repository = repository_column(repository)?;
        let moved = match expected {
            None => sqlx::query(
                "insert into refs (repository, name, target) values ($1, $2, $3) \
                 on conflict (repository, name) do nothing",
            )
            .bind(repository)
            .bind(ref_name(name))
            .bind(new.digest().as_slice()),
            Some(expected) => sqlx::query(
                "update refs set target = $3 \
                 where repository = $1 and name = $2 and target = $4",
            )
            .bind(repository)
            .bind(ref_name(name))
            .bind(new.digest().as_slice())
            .bind(expected.digest().as_slice()),
        }
        .execute(&self.pool)
        .await
        .map_err(backend)?
        .rows_affected();
        if moved == 1 {
            return Ok(());
        }
        Err(ChangesetLogError::RefMoved {
            current: self.current(repository, name).await?,
        })
    }
}
