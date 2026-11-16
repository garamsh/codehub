use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use app::domain::address::Address;
use app::domain::changeset::Changeset;
use app::domain::repository::RepositoryId;
use app::port::driven::changeset_log::{ChangesetLog, ChangesetLogError, Ref};
use sqlx::PgPool;

use crate::conversion;
use crate::database::Database;
use crate::retry;
use crate::statements::Statements;

/// How many changesets of `main` a settled swap looks through for its own.
const SETTLE_DEPTH: usize = 1000;
/// How long a settled swap waits for the backend that ran its statement to stop.
const STOP_BOUND: Duration = Duration::from_secs(5);
/// The pause between two looks at that backend.
const STOP_POLL: Duration = Duration::from_millis(10);

/// A tag that tells the statement of one swap attempt from every other statement of the database,
/// in whichever process and on whichever backend it runs.
fn swap_tag() -> String {
    static ATTEMPTS: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    let attempt = ATTEMPTS.fetch_add(1, Ordering::Relaxed);
    format!("/*swap {}-{nanos}-{attempt}*/", std::process::id())
}

/// The changeset log and the refs in PostgreSQL.
///
/// A ref moves in one statement, so two moves of the same ref cannot interleave: the second finds
/// the row the first left and moves nothing. The statement carries a tag of its attempt, and is not
/// prepared, so the tag does not fill the statement cache. Where the connection fails after the
/// statement was sent, the backend that runs it may still commit it. The swap is settled in two
/// steps: the backend is stopped, found by the tag in `pg_stat_activity` and not by a pid kept per
/// connection, which a transaction pooler would make the wrong backend; then the swap is won
/// exactly where the new changeset is `main` or on its first-parent chain.
pub struct PostgresChangesetLog {
    pool: PgPool,
    statements: Statements,
    database: Database,
}

impl PostgresChangesetLog {
    /// A changeset log over `database`.
    #[must_use]
    pub fn new(database: &Database) -> Self {
        Self {
            pool: database.pool.clone(),
            statements: database.statements,
            database: database.clone(),
        }
    }

    /// The answer of a swap whose connection failed after the statement tagged `tag` was sent:
    /// the statement is stopped, and then `main` tells by identity whether it won.
    async fn settle(
        &self,
        repository: RepositoryId,
        name: Ref,
        expected: Option<Address>,
        new: Address,
        tag: &str,
        error: &sqlx::Error,
    ) -> Result<(), ChangesetLogError> {
        self.stop_statement(tag).await.map_err(|_| backend(error))?;
        let current = self.read_ref(repository, name).await?;
        let won = match current {
            None => false,
            Some(current) if current == new => true,
            Some(current) => self
                .first_parent_chain(repository, Some(current), Some(new), SETTLE_DEPTH)
                .await
                .map_err(|_| backend(error))?
                .is_some(),
        };
        if won {
            Ok(())
        } else if current == expected {
            Err(backend(error))
        } else {
            Err(ChangesetLogError::RefMoved { current })
        }
    }

    /// Ends the backend that runs a statement tagged `tag`, if one does, and waits until none runs
    /// it: a backend that is idle ran its statement to its end, and one that was ended rolled an
    /// unfinished transaction back, so the statement cannot commit after this returns.
    async fn stop_statement(&self, tag: &str) -> Result<(), sqlx::Error> {
        let deadline = Instant::now() + STOP_BOUND;
        loop {
            let running = self
                .statements
                .query_scalar::<bool>(
                    "select pg_terminate_backend(pid) from pg_stat_activity \
                 where datname = current_database() and pid <> pg_backend_pid() \
                   and state <> 'idle' and position($1 in query) > 0",
                )
                .bind(tag)
                .persistent(false)
                .fetch_all(&self.pool)
                .await?;
            if running.is_empty() {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(sqlx::Error::PoolTimedOut);
            }
            tokio::time::sleep(STOP_POLL).await;
        }
    }

    /// The answer of a chain that ended without `base`: `None` where the log holds `base`, and
    /// not found where it does not.
    async fn beyond(
        &self,
        repository: i64,
        base: Address,
    ) -> Result<Option<Vec<Changeset>>, ChangesetLogError> {
        let held = retry::idempotent(|| async move {
            self.statements
                .query_scalar::<bool>(
                    "select exists (select 1 from changesets where repository = $1 and id = $2)",
                )
                .bind(repository)
                .bind(base.digest().as_slice())
                .fetch_one(&self.pool)
                .await
        })
        .await
        .map_err(backend)?;
        if held {
            Ok(None)
        } else {
            Err(ChangesetLogError::NotFound(base))
        }
    }

    async fn current(
        &self,
        repository: i64,
        name: Ref,
    ) -> Result<Option<Address>, ChangesetLogError> {
        retry::idempotent(|| async move {
            self.statements
                .query_scalar::<Vec<u8>>(
                    "select target from refs where repository = $1 and name = $2",
                )
                .bind(repository)
                .bind(ref_name(name))
                .fetch_optional(&self.pool)
                .await
        })
        .await
        .map_err(backend)?
        .map(|bytes| conversion::address(bytes).map_err(ChangesetLogError::Backend))
        .transpose()
    }
}

/// Whether a statement that failed with `error` may have committed: the connection broke after it
/// was sent, or the server ended the session (SQLSTATE 57P01, 57P02, 57P03 and the class 08).
fn outcome_unknown(error: &sqlx::Error) -> bool {
    if let sqlx::Error::Database(error) = error {
        return error.code().is_some_and(|code| {
            matches!(&*code, "57P01" | "57P02" | "57P03") || code.starts_with("08")
        });
    }
    matches!(
        error,
        sqlx::Error::Io(_)
            | sqlx::Error::Tls(_)
            | sqlx::Error::Protocol(_)
            | sqlx::Error::WorkerCrashed
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
        let repository = repository_column(repository)?;
        let encoding = &encoding;
        retry::idempotent(|| async move {
            self.statements
                .query(
                    "insert into changesets (repository, id, encoding) values ($1, $2, $3) \
                 on conflict (repository, id) do nothing",
                )
                .bind(repository)
                .bind(id.digest().as_slice())
                .bind(encoding.clone())
                .execute(&self.pool)
                .await
        })
        .await
        .map_err(backend)?;
        Ok(id)
    }

    async fn get(
        &self,
        repository: RepositoryId,
        id: Address,
    ) -> Result<Changeset, ChangesetLogError> {
        let repository = repository_column(repository)?;
        let encoding = self
            .database
            .immutable(|pool| async move {
                self.statements
                    .query_scalar::<Vec<u8>>(
                        "select encoding from changesets where repository = $1 and id = $2",
                    )
                    .bind(repository)
                    .bind(id.digest().as_slice())
                    .fetch_optional(&pool)
                    .await
            })
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
        limit: usize,
    ) -> Result<Option<Vec<Changeset>>, ChangesetLogError> {
        let column = repository_column(repository)?;
        let Some(head) = head else {
            return match base {
                None => Ok(Some(Vec::new())),
                Some(base) => self.beyond(column, base).await,
            };
        };
        if base == Some(head) {
            return Ok(Some(Vec::new()));
        }
        let base_digest = base.as_ref().map(|base| base.digest().as_slice());
        // The first parent sits at a fixed offset of the encoding, whose format version 1 is that
        // of ADR 0016: a version byte, a kind byte, a four-byte parent count, then the first
        // parent as an algorithm byte, a four-byte length and its 32-byte digest. Another version
        // ends the walk at its changeset, which then fails to decode.
        let rows = retry::idempotent(|| async move {
            self.statements
                .query_as::<(Vec<u8>, Vec<u8>)>(
                    "with recursive chain (depth, id, encoding) as ( \
                 select 0, id, encoding from changesets where repository = $1 and id = $2 \
                 union all \
                 select chain.depth + 1, parent.id, parent.encoding \
                 from chain join changesets parent \
                   on parent.repository = $1 \
                  and parent.id = substring(chain.encoding from 12 for 32) \
                 where get_byte(chain.encoding, 0) = 1 \
                   and substring(chain.encoding from 3 for 4) <> '\\x00000000'::bytea \
                   and chain.id is distinct from $3 \
                   and chain.depth < $4 \
             ) select id, encoding from chain order by depth",
                )
                .bind(column)
                .bind(head.digest().as_slice())
                .bind(base_digest)
                .bind(i64::try_from(limit).unwrap_or(i64::MAX))
                .fetch_all(&self.pool)
                .await
        })
        .await
        .map_err(backend)?;
        let mut chain = Vec::with_capacity(rows.len());
        for (id, encoding) in rows {
            if base_digest == Some(id.as_slice()) {
                return Ok(Some(chain));
            }
            chain.push(Changeset::decode(&encoding).map_err(backend)?);
        }
        if chain.len() > limit {
            return Err(ChangesetLogError::Backend(format!(
                "the first-parent chain is longer than {limit}"
            )));
        }
        let Some(last) = chain.last() else {
            return Err(ChangesetLogError::NotFound(head));
        };
        match (last.parents.first(), base) {
            (Some(parent), _) => Err(ChangesetLogError::NotFound(*parent)),
            (None, None) => Ok(Some(chain)),
            (None, Some(base)) => self.beyond(column, base).await,
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
        let column = repository_column(repository)?;
        let tag = swap_tag();
        let statement = match expected {
            None => format!(
                "{tag} insert into refs (repository, name, target) values ($1, $2, $3) \
                 on conflict (repository, name) do nothing"
            ),
            Some(_) => format!(
                "{tag} update refs set target = $3 \
                 where repository = $1 and name = $2 and target = $4"
            ),
        };
        // The tag is digits and dashes made here, and nothing a caller supplies.
        let query = self
            .statements
            .query(sqlx::AssertSqlSafe(statement))
            .persistent(false)
            .bind(column)
            .bind(ref_name(name))
            .bind(new.digest().as_slice());
        let query = match expected {
            None => query,
            Some(expected) => query.bind(expected.digest().as_slice()),
        };
        let moved = match query.execute(&self.pool).await {
            Ok(done) => done.rows_affected(),
            Err(error) if outcome_unknown(&error) => {
                return self
                    .settle(repository, name, expected, new, &tag, &error)
                    .await;
            }
            Err(error) => return Err(backend(error)),
        };
        if moved == 1 {
            return Ok(());
        }
        Err(ChangesetLogError::RefMoved {
            current: self.current(column, name).await?,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::error::Error;
    use std::fmt;

    use sqlx::error::{DatabaseError, ErrorKind};

    use super::outcome_unknown;

    #[derive(Debug)]
    struct Server(&'static str);

    impl fmt::Display for Server {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(formatter, "server error {}", self.0)
        }
    }

    impl Error for Server {}

    impl DatabaseError for Server {
        fn message(&self) -> &str {
            "server error"
        }

        fn code(&self) -> Option<Cow<'_, str>> {
            Some(Cow::Borrowed(self.0))
        }

        fn as_error(&self) -> &(dyn Error + Send + Sync + 'static) {
            self
        }

        fn as_error_mut(&mut self) -> &mut (dyn Error + Send + Sync + 'static) {
            self
        }

        fn into_error(self: Box<Self>) -> Box<dyn Error + Send + Sync + 'static> {
            self
        }

        fn kind(&self) -> ErrorKind {
            ErrorKind::Other
        }
    }

    fn server(code: &'static str) -> sqlx::Error {
        sqlx::Error::Database(Box::new(Server(code)))
    }

    #[test]
    fn a_broken_connection_or_a_session_the_server_ended_leaves_the_outcome_unknown() {
        let io = std::io::Error::from(std::io::ErrorKind::ConnectionReset);
        assert!(outcome_unknown(&sqlx::Error::Io(io)));
        assert!(outcome_unknown(&sqlx::Error::Protocol("bad".to_owned())));
        assert!(outcome_unknown(&sqlx::Error::WorkerCrashed));
        for code in ["57P01", "57P02", "57P03", "08000", "08006", "08P01"] {
            assert!(outcome_unknown(&server(code)), "{code}");
        }
    }

    #[test]
    fn an_error_the_server_answered_a_statement_with_leaves_no_doubt() {
        for code in ["23505", "40001", "40P01", "57014", "42601", "53300"] {
            assert!(!outcome_unknown(&server(code)), "{code}");
        }
        assert!(!outcome_unknown(&sqlx::Error::PoolTimedOut));
        assert!(!outcome_unknown(&sqlx::Error::RowNotFound));
    }
}
