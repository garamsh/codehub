use std::fmt;

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

impl Database {
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
