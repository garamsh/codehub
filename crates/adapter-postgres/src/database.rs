use std::fmt;
use std::time::{Duration, Instant};

use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

/// A pool of connections to one PostgreSQL database, which every adapter of this crate shares.
#[derive(Clone)]
pub struct Database {
    pub(crate) pool: PgPool,
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
    /// one of them connects.
    ///
    /// # Errors
    ///
    /// [`DatabaseError::Connect`] where `url` is not a PostgreSQL URL or the server does not
    /// answer.
    pub async fn connect(url: &str, pool_size: u32) -> Result<Self, DatabaseError> {
        let pool = PgPoolOptions::new()
            .max_connections(pool_size)
            .connect(url)
            .await
            .map_err(|error| DatabaseError::Connect(error.to_string()))?;
        Ok(Self { pool })
    }

    /// Whether the database has applied every migration this build embeds. A schema that is newer,
    /// one that has applied migrations this build does not know, is current too (ADR 0052).
    ///
    /// # Errors
    ///
    /// [`DatabaseError::Connect`] where the database does not answer.
    pub async fn schema_is_current(&self) -> Result<bool, DatabaseError> {
        let unanswered = |error: sqlx::Error| DatabaseError::Connect(error.to_string());
        let recorded =
            sqlx::query_scalar::<_, bool>("select to_regclass('_sqlx_migrations') is not null")
                .fetch_one(&self.pool)
                .await
                .map_err(unanswered)?;
        if !recorded {
            return Ok(false);
        }
        let applied = sqlx::query_scalar::<_, Option<i64>>(
            "select max(version) from _sqlx_migrations where success",
        )
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
