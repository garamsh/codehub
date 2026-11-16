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
/// the row the first left and moves nothing. Where the connection fails after the statement was
/// sent, the ref is read again and decides the answer: the new target means the swap was won.
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

    /// The answer of a swap whose connection failed after the statement was sent, from the ref
    /// read on a connection of the pool that is not the failed one.
    async fn settle(
        &self,
        repository: i64,
        name: Ref,
        expected: Option<Address>,
        new: Address,
        error: &sqlx::Error,
    ) -> Result<(), ChangesetLogError> {
        match self.current(repository, name).await? {
            Some(current) if current == new => Ok(()),
            current if current == expected => Err(backend(error)),
            current => Err(ChangesetLogError::RefMoved { current }),
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

/// Whether a statement that failed with `error` may have committed: the connection broke after it
/// was sent.
fn outcome_unknown(error: &sqlx::Error) -> bool {
    matches!(
        error,
        sqlx::Error::Io(_) | sqlx::Error::Tls(_) | sqlx::Error::Protocol(_)
    )
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
        Ref::LastGreen => "last-green",
    }
}

#[async_trait::async_trait]
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

    async fn first_parent_chain(
        &self,
        repository: RepositoryId,
        head: Option<Address>,
        base: Option<Address>,
    ) -> Result<Option<Vec<Changeset>>, ChangesetLogError> {
        let Some(head) = head else {
            return Ok(base.is_none().then(Vec::new));
        };
        if base == Some(head) {
            return Ok(Some(Vec::new()));
        }
        let base_digest = base.as_ref().map(|base| base.digest().as_slice());
        // The first parent sits at a fixed offset of the encoding (ADR 0016): a version byte, a
        // kind byte, a four-byte parent count, then the first parent as an algorithm byte, a
        // four-byte length and its 32-byte digest.
        let rows = sqlx::query_as::<_, (Vec<u8>, Vec<u8>)>(
            "with recursive chain (depth, id, encoding) as ( \
                 select 0, id, encoding from changesets where repository = $1 and id = $2 \
                 union all \
                 select chain.depth + 1, parent.id, parent.encoding \
                 from chain join changesets parent \
                   on parent.repository = $1 \
                  and parent.id = substring(chain.encoding from 12 for 32) \
                 where substring(chain.encoding from 3 for 4) <> '\\x00000000'::bytea \
                   and chain.id is distinct from $3 \
             ) select id, encoding from chain order by depth",
        )
        .bind(repository_column(repository)?)
        .bind(head.digest().as_slice())
        .bind(base_digest)
        .fetch_all(&self.pool)
        .await
        .map_err(backend)?;
        let mut chain = Vec::with_capacity(rows.len());
        for (id, encoding) in rows {
            if base_digest == Some(id.as_slice()) {
                return Ok(Some(chain));
            }
            chain.push(Changeset::decode(&encoding).map_err(backend)?);
        }
        let Some(last) = chain.last() else {
            return Err(ChangesetLogError::NotFound(head));
        };
        match last.parents.first() {
            Some(parent) => Err(ChangesetLogError::NotFound(*parent)),
            None if base.is_none() => Ok(Some(chain)),
            None => Ok(None),
        }
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
        .await;
        let moved = match moved {
            Ok(done) => done.rows_affected(),
            Err(error) if outcome_unknown(&error) => {
                return self.settle(repository, name, expected, new, &error).await;
            }
            Err(error) => return Err(backend(error)),
        };
        if moved == 1 {
            return Ok(());
        }
        Err(ChangesetLogError::RefMoved {
            current: self.current(repository, name).await?,
        })
    }
}
