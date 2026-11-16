use std::fmt;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgConnection, PgPoolOptions};

use crate::statements::Statements;
use crate::{primary, retry};

/// How often the supervisor asks the primary whether it is still the writable primary: the floor
/// of its attention, which a failure the adapters see ([`retry::suspect_failover`]) cuts short.
const SUPERVISE_EVERY: Duration = Duration::from_secs(1);

/// How long reads skip a replica that failed them and go straight to the primary.
const REPLICA_COOL_DOWN: Duration = Duration::from_secs(5);

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
    /// The prepared statements kept per connection. A transaction-mode pooler needs one or more,
    /// and a pooler that tracks them; zero sends every statement unnamed, for a direct connection or
    /// a session-mode pooler (ADR 0054).
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
    replica: Option<Replica>,
    /// Held so the supervisor, which holds it weakly, ends when the last clone is dropped.
    _shared: Arc<Shared>,
}

/// The pool of the read replica, and when reads may try it again after it failed them.
#[derive(Clone)]
struct Replica {
    pool: PgPool,
    down_until: Arc<Mutex<Option<Instant>>>,
}

impl Replica {
    fn usable(&self) -> bool {
        self.down_until
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_none_or(|until| Instant::now() >= until)
    }

    fn failed(&self) {
        *self
            .down_until
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some(Instant::now() + REPLICA_COOL_DOWN);
    }
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

/// How the pool's connections are used at one moment.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PoolState {
    /// Connections open, idle or lent out.
    pub open: u32,
    /// Connections open and idle.
    pub idle: u32,
    /// The most connections the pool opens.
    pub max: u32,
}

impl Database {
    /// How the pool's connections are used now.
    #[must_use]
    pub fn pool_state(&self) -> PoolState {
        let open = self.pool.size();
        let idle = u32::try_from(self.pool.num_idle()).unwrap_or(u32::MAX);
        PoolState {
            open,
            idle: idle.min(open),
            max: self.pool.options().get_max_connections(),
        }
    }

    /// How long taking a connection from the pool takes now, the connection returned at once. The
    /// pool offers no hook around its own acquisitions, so a probe is the measurement there is.
    ///
    /// # Errors
    ///
    /// [`DatabaseError::Connect`] where no connection could be taken.
    pub async fn probe_acquire(&self) -> Result<Duration, DatabaseError> {
        let started = Instant::now();
        let connection = self
            .pool
            .acquire()
            .await
            .map_err(|error| DatabaseError::Connect(error.to_string()))?;
        let waited = started.elapsed();
        drop(connection);
        Ok(waited)
    }

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
        Self::open_supervised(options, SUPERVISE_EVERY).await
    }

    /// [`Self::open`], the supervisor asking every `supervise_every`.
    pub(crate) async fn open_supervised(
        options: PostgresOptions<'_>,
        supervise_every: Duration,
    ) -> Result<Self, DatabaseError> {
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
            supervise_every,
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
            && replica.usable()
        {
            match read(replica.pool.clone()).await {
                Ok(Some(found)) => return Ok(Some(found)),
                Ok(None) => {}
                Err(_) => replica.failed(),
            }
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
fn replica_pool(url: &str, options: &PostgresOptions<'_>) -> Result<Replica, DatabaseError> {
    let mut hosts = primary::hosts(url, options.statement_cache, options.statement_timeout)
        .map_err(DatabaseError::Connect)?;
    if hosts.len() != 1 {
        return Err(DatabaseError::Connect(
            "the read replica URL must name exactly one host".to_owned(),
        ));
    }
    Ok(Replica {
        pool: PgPoolOptions::new()
            .max_connections(options.pool_size)
            .acquire_timeout(options.acquire_timeout.min(REPLICA_ACQUIRE))
            .connect_lazy_with(hosts.remove(0)),
        down_until: Arc::new(Mutex::new(None)),
    })
}

/// Keeps `pool` on the writable primary: every `every`, and at once where an adapter saw a failure
/// that a failover explains, it asks the server it last found whether that is still a primary, and
/// where it is not, or does not answer, it looks for the primary among the hosts again and points
/// the pool at it, whether the host it finds is another or the same: an address that follows the
/// primary serves a new node, and the connections made before are closed on their next use. It
/// ends when the database is dropped.
async fn supervise(
    shared: Weak<Shared>,
    pool: PgPool,
    mut current: PgConnectOptions,
    probe: PgConnection,
    every: Duration,
) {
    let mut probe = Some(probe);
    loop {
        tokio::select! {
            () = tokio::time::sleep(every) => {}
            () = retry::FAILOVER_SUSPECTED.notified() => {}
        }
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
        if (found.get_host(), found.get_port()) != (current.get_host(), current.get_port()) {
            pool.set_connect_options(found.clone());
            current = found;
        }
        *shared
            .switched
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Instant::now();
        probe = Some(connection);
    }
}

#[cfg(test)]
mod tests {
    use sqlx::{Connection, Executor};
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

    /// A pool over a fresh database whose supervisor asks every `supervise_every`.
    async fn supervised(supervise_every: Duration) -> (TestDatabase, Database) {
        let database = TestDatabase::new().await;
        let url = database.url();
        let pool = Database::open_supervised(
            PostgresOptions {
                url: &url,
                pool_size: 2,
                acquire_timeout: Duration::from_secs(2),
                statement_timeout: Duration::ZERO,
                statement_cache: 100,
                read_url: None,
            },
            supervise_every,
        )
        .await
        .unwrap();
        (database, pool)
    }

    async fn backend_of(database: &Database) -> i32 {
        database
            .statements
            .query_scalar::<i32>("select pg_backend_pid()")
            .fetch_one(&database.pool)
            .await
            .unwrap()
    }

    /// Ends every backend of the database but `keep` and the one asking: the supervisor's probe
    /// among them, which is how it learns its server went away.
    async fn end_the_probe(url: &str, keep: i32) {
        let mut other = PgConnection::connect(url).await.unwrap();
        let ending = format!(
            "select pg_terminate_backend(pid) from pg_stat_activity \
             where datname = current_database() and pid <> pg_backend_pid() and pid <> {keep}"
        );
        other.execute(sqlx::AssertSqlSafe(ending)).await.unwrap();
    }

    /// Whether the pool serves a connection other than the backend `old` within ten seconds, the
    /// condition polled and bounded as `testing.md` allows; `nudge` runs before each look.
    async fn replaced(database: &Database, old: i32, nudge: impl Fn()) -> bool {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            nudge();
            if backend_of(database).await != old {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        false
    }

    #[tokio::test]
    async fn a_primary_that_changed_behind_the_same_address_gets_a_new_pool() {
        let (test, database) = supervised(Duration::from_millis(100)).await;
        let old = backend_of(&database).await;

        end_the_probe(&test.url(), old).await;

        assert!(
            replaced(&database, old, || {}).await,
            "the connections made before the failover were kept"
        );
    }

    #[tokio::test]
    async fn a_failure_a_failover_explains_wakes_the_supervisor_before_its_next_poll() {
        let (test, database) = supervised(Duration::from_secs(3600)).await;
        let old = backend_of(&database).await;

        end_the_probe(&test.url(), old).await;

        assert!(
            replaced(&database, old, || retry::suspect_failover(
                &sqlx::Error::WorkerCrashed
            ))
            .await,
            "the supervisor waited for its poll"
        );
    }

    #[tokio::test]
    async fn a_replica_that_failed_a_read_is_not_asked_again_during_the_cool_down() {
        let test = TestDatabase::new().await;
        let url = test.url();
        let database = Database::open(PostgresOptions {
            url: &url,
            pool_size: 2,
            acquire_timeout: Duration::from_secs(2),
            statement_timeout: Duration::ZERO,
            statement_cache: 100,
            read_url: Some("postgres://u:p@127.0.0.1:1/db"),
        })
        .await
        .unwrap();
        let asked = std::cell::Cell::new(0);
        let statements = database.statements;
        let read = |pool: PgPool| {
            asked.set(asked.get() + 1);
            async move {
                statements
                    .query_scalar::<i32>("select 1")
                    .fetch_optional(&pool)
                    .await
            }
        };

        assert_eq!(database.immutable(&read).await.unwrap(), Some(1));
        assert_eq!(asked.get(), 2, "the replica, then the primary");
        assert_eq!(database.immutable(&read).await.unwrap(), Some(1));
        assert_eq!(asked.get(), 3, "the primary alone");
    }
}
