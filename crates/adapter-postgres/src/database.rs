use std::fmt;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgConnection, PgPoolOptions};

use crate::statements::Statements;
use crate::{primary, retry};

/// How often the supervisor asks the primary whether it is still the writable primary.
const SUPERVISE_EVERY: Duration = Duration::from_secs(1);

/// The longest the replica pool waits for a connection before the read falls back to the primary.
const REPLICA_ACQUIRE: Duration = Duration::from_secs(2);

/// What a [`Database`] is opened with (ADR 0054).
#[derive(Clone, Copy, Debug)]
pub struct PostgresOptions<'a> {
    /// The URL of the database: one host, or several with `target_session_attrs=read-write`.
    pub url: &'a str,
    /// The most connections to the primary.
    pub pool_size: u32,
    /// How long taking a connection from the pool waits before failing.
    pub acquire_timeout: Duration,
    /// How long one statement may run; zero sets no limit.
    pub statement_timeout: Duration,
    /// The prepared statements kept per connection; zero sends every statement unnamed, which a
    /// transaction-mode pooler needs.
    pub statement_cache: usize,
    /// The URL of a read replica, which serves the immutable reads, if any.
    pub read_url: Option<&'a str>,
}

/// A pool of connections to the writable primary of one PostgreSQL database, which every adapter
/// of this crate shares, and the replica pool that serves the immutable reads where one is set.
#[derive(Clone)]
pub struct Database {
    pub(crate) pool: PgPool,
    pub(crate) statements: Statements,
    replica: Option<PgPool>,
    /// Held so the supervisor, which holds it weakly, ends when the last clone is dropped.
    _shared: Arc<Shared>,
}

/// What the pool's hooks and the supervisor share.
struct Shared {
    statements: Statements,
    hosts: Vec<PgConnectOptions>,
    /// When the pool last moved to another primary. A connection made before is closed on its next
    /// use, so a pool never keeps a session to a server that is no longer the primary.
    switched: Mutex<Instant>,
}

impl Shared {
    fn switched(&self) -> Instant {
        *self
            .switched
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Why the database could not be reached or migrated.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum DatabaseError {
    /// Connecting failed; carries a description.
    Connect(String),
    /// Applying the migrations failed; carries a description.
    Migrate(String),
}

impl fmt::Display for DatabaseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connect(description) => {
                write!(f, "connecting to the database failed: {description}")
            }
            Self::Migrate(description) => write!(f, "migrating the database failed: {description}"),
        }
    }
}

impl std::error::Error for DatabaseError {}

impl Database {
    /// Opens a pool of at most `pool_size` connections to the database at `url`, and checks that
    /// one of them connects. The settings of [`PostgresOptions`] not named here take their
    /// defaults, with no statement timeout.
    ///
    /// # Errors
    ///
    /// [`DatabaseError::Connect`] where `url` is not a PostgreSQL URL or no host is a writable
    /// primary.
    pub async fn connect(url: &str, pool_size: u32) -> Result<Self, DatabaseError> {
        Self::open(PostgresOptions {
            url,
            pool_size,
            acquire_timeout: Duration::from_secs(30),
            statement_timeout: Duration::ZERO,
            statement_cache: 100,
            read_url: None,
        })
        .await
    }

    /// Opens the pool `options` describe on the first host that is a writable primary, and starts
    /// the task that moves the pool to the new primary after a failover.
    ///
    /// # Errors
    ///
    /// [`DatabaseError::Connect`] where a URL is not a PostgreSQL URL or no host is a writable
    /// primary.
    pub async fn open(options: PostgresOptions<'_>) -> Result<Self, DatabaseError> {
        let hosts = primary::hosts(
            options.url,
            options.statement_cache,
            options.statement_timeout,
        )
        .map_err(DatabaseError::Connect)?;
        let statements = Statements::new(options.statement_cache);
        let (first, probe) = primary::find(statements, &hosts)
            .await
            .map_err(DatabaseError::Connect)?;
        let shared = Arc::new(Shared {
            statements,
            hosts,
            switched: Mutex::new(Instant::now()),
        });
        let held = Arc::downgrade(&shared);
        let pool = PgPoolOptions::new()
            .max_connections(options.pool_size)
            .acquire_timeout(options.acquire_timeout)
            .after_connect(move |connection, _| {
                Box::pin(async move {
                    if primary::in_recovery(statements, connection).await? {
                        return Err(sqlx::Error::Protocol(
                            "the server is a standby, not a writable primary".to_owned(),
                        ));
                    }
                    Ok(())
                })
            })
            .before_acquire(move |_, metadata| {
                let held = held.clone();
                Box::pin(async move {
                    let Some(shared) = held.upgrade() else {
                        return Ok(true);
                    };
                    let made = Instant::now().checked_sub(metadata.age);
                    Ok(made.is_none_or(|made| made >= shared.switched()))
                })
            })
            .connect_with(first.clone())
            .await
            .map_err(|error| DatabaseError::Connect(error.to_string()))?;
        let replica = options
            .read_url
            .map(|url| replica_pool(url, &options))
            .transpose()?;
        tokio::spawn(supervise(
            Arc::downgrade(&shared),
            pool.clone(),
            first,
            probe,
        ));
        Ok(Self {
            pool,
            statements,
            replica,
            _shared: shared,
        })
    }

    /// Whether the connected server is a writable primary: false while it is a standby.
    ///
    /// # Errors
    ///
    /// [`DatabaseError::Connect`] where no connection to a primary can be made.
    pub async fn is_writable(&self) -> Result<bool, DatabaseError> {
        let unanswered = |error: sqlx::Error| DatabaseError::Connect(error.to_string());
        let mut connection = self.pool.acquire().await.map_err(unanswered)?;
        primary::in_recovery(self.statements, &mut connection)
            .await
            .map(|standby| !standby)
            .map_err(unanswered)
    }

    /// Reads data that is immutable and keyed by its own address: from the replica where one is
    /// set, and from the primary where the replica has no answer, is behind or fails. Nothing else
    /// may be read through it: a ref, a draft, a queue, a grant or a run changes after it is
    /// written, and a replica lags.
    pub(crate) async fn immutable<T, Fut>(
        &self,
        mut read: impl FnMut(PgPool) -> Fut,
    ) -> Result<Option<T>, sqlx::Error>
    where
        Fut: Future<Output = Result<Option<T>, sqlx::Error>>,
    {
        if let Some(replica) = &self.replica
            && let Ok(Some(found)) = read(replica.clone()).await
        {
            return Ok(Some(found));
        }
        retry::idempotent(|| read(self.pool.clone())).await
    }

    /// Whether the database has applied every migration this build embeds. A schema that is newer,
    /// one that has applied migrations this build does not know, is current too (ADR 0052).
    ///
    /// # Errors
    ///
    /// [`DatabaseError::Connect`] where the database does not answer.
    pub async fn schema_is_current(&self) -> Result<bool, DatabaseError> {
        let unanswered = |error: sqlx::Error| DatabaseError::Connect(error.to_string());
        let recorded = self
            .statements
            .query_scalar::<bool>("select to_regclass('_sqlx_migrations') is not null")
            .fetch_one(&self.pool)
            .await
            .map_err(unanswered)?;
        if !recorded {
            return Ok(false);
        }
        let applied = self
            .statements
            .query_scalar::<Option<i64>>("select max(version) from _sqlx_migrations where success")
            .fetch_one(&self.pool)
            .await
            .map_err(unanswered)?;
        let latest = sqlx::migrate!("./migrations")
            .iter()
            .map(|migration| migration.version)
            .max();
        Ok(applied >= latest)
    }

    /// Applies the migrations the database has not yet had, in order; a migration already
    /// applied is not applied again.
    ///
    /// # Errors
    ///
    /// [`DatabaseError::Migrate`] where a migration fails or an applied one has changed.
    pub async fn migrate(&self) -> Result<(), DatabaseError> {
        sqlx::migrate!("./migrations")
            .run(&self.pool)
            .await
            .map_err(|error| DatabaseError::Migrate(error.to_string()))
    }
}

/// The lazily connected pool of the read replica at `url`, which must name one host.
fn replica_pool(url: &str, options: &PostgresOptions<'_>) -> Result<PgPool, DatabaseError> {
    let mut hosts = primary::hosts(url, options.statement_cache, options.statement_timeout)
        .map_err(DatabaseError::Connect)?;
    if hosts.len() != 1 {
        return Err(DatabaseError::Connect(
            "the read replica URL must name exactly one host".to_owned(),
        ));
    }
    Ok(PgPoolOptions::new()
        .max_connections(options.pool_size)
        .acquire_timeout(options.acquire_timeout.min(REPLICA_ACQUIRE))
        .connect_lazy_with(hosts.remove(0)))
}

/// Keeps `pool` on the writable primary: every [`SUPERVISE_EVERY`] it asks the server it last
/// found whether that is still a primary, and where it is not, or does not answer, it looks for the
/// primary among the hosts again and points the pool at it. It ends when the database is dropped.
async fn supervise(
    shared: Weak<Shared>,
    pool: PgPool,
    mut current: PgConnectOptions,
    probe: PgConnection,
) {
    let mut probe = Some(probe);
    loop {
        tokio::time::sleep(SUPERVISE_EVERY).await;
        let Some(shared) = shared.upgrade() else {
            return;
        };
        let healthy = match probe.as_mut() {
            Some(connection) => matches!(
                tokio::time::timeout(
                    primary::HOST_TIMEOUT,
                    primary::in_recovery(shared.statements, connection)
                )
                .await,
                Ok(Ok(false))
            ),
            None => false,
        };
        if healthy {
            continue;
        }
        if let Some(stale) = probe.take() {
            primary::close(stale).await;
        }
        let Ok((found, connection)) = primary::find(shared.statements, &shared.hosts).await else {
            continue;
        };
        let moved =
            (found.get_host(), found.get_port()) != (current.get_host(), current.get_port());
        if moved {
            pool.set_connect_options(found.clone());
            *shared
                .switched
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Instant::now();
            current = found;
        }
        probe = Some(connection);
    }
}

#[cfg(test)]
mod tests {
    use test_support::TestDatabase;

    use super::*;

    async fn open(statement_timeout: Duration) -> (TestDatabase, Database) {
        let database = TestDatabase::new().await;
        let url = database.url();
        let pool = Database::open(PostgresOptions {
            url: &url,
            pool_size: 2,
            acquire_timeout: Duration::from_secs(2),
            statement_timeout,
            statement_cache: 100,
            read_url: None,
        })
        .await
        .unwrap();
        (database, pool)
    }

    #[tokio::test]
    async fn a_statement_running_past_the_timeout_is_cancelled_by_the_server() {
        let (_database, database) = open(Duration::from_secs(1)).await;

        let started = Instant::now();
        let cancelled = database
            .statements
            .query("select pg_sleep(10)")
            .execute(&database.pool)
            .await
            .unwrap_err();

        assert!(started.elapsed() < Duration::from_secs(8));
        let sqlx::Error::Database(error) = cancelled else {
            panic!("not a database error: {cancelled}");
        };
        assert_eq!(error.code().as_deref(), Some("57014"));
    }

    #[tokio::test]
    async fn a_timeout_of_zero_leaves_the_statement_unlimited() {
        let (_database, database) = open(Duration::ZERO).await;

        let setting = database
            .statements
            .query_scalar::<String>("select current_setting('statement_timeout')")
            .fetch_one(&database.pool)
            .await
            .unwrap();

        assert_eq!(setting, "0");
    }
}
