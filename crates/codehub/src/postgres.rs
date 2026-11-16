//! The PostgreSQL operating modes: the settings that go with the database URL and the opening of the
//! pool they describe (ADR 0054).

use std::fmt;
use std::time::Duration;

use adapter_postgres::{Database, PostgresOptions};
use clap::Args;

use crate::Error;
use crate::settings::{DatabaseOptions, redact_url};

/// The connections a migration needs: the lock and the migration run on one.
const MIGRATION_POOL_SIZE: u32 = 2;

/// How the pool to the primary behaves: how long it waits, how long a statement may run, and how
/// many statements it prepares.
#[derive(Args, Debug)]
pub struct PostgresSettings {
    /// How long taking a connection from the pool waits before the request fails, in seconds.
    #[arg(long, env = "CODEHUB_POSTGRES_ACQUIRE_TIMEOUT", default_value_t = 5, value_parser = clap::value_parser!(u64).range(1..=3600))]
    pub postgres_acquire_timeout: u64,
    /// How long one statement may run before PostgreSQL cancels it, in seconds; 0 sets no limit.
    /// Behind a transaction-mode pooler set it on the role or with the pooler's own limit.
    #[arg(long, env = "CODEHUB_POSTGRES_STATEMENT_TIMEOUT", default_value_t = 30, value_parser = clap::value_parser!(u64).range(..=86_400))]
    pub postgres_statement_timeout: u64,
    /// The prepared statements kept per connection. Behind a transaction-mode pooler it must be 1
    /// or more and the pooler must track prepared statements (PgBouncer 1.21 or later with
    /// `max_prepared_statements` above this value). 0 sends every statement unnamed, for a direct
    /// connection or a session-mode pooler; behind a transaction pooler it fails.
    #[arg(long, env = "CODEHUB_POSTGRES_STATEMENT_CACHE", default_value_t = 100, value_parser = clap::value_parser!(u32).range(..=10_000))]
    pub postgres_statement_cache: u32,
}

/// The read replica of a role that reads immutable data.
#[derive(Args)]
pub struct ReplicaOptions {
    /// The URL of a read replica, which serves only the immutable reads, address-keyed data that a
    /// replica's lag cannot make wrong; a miss is read from the primary. Unset serves every read
    /// from the primary.
    #[arg(long, env = "CODEHUB_POSTGRES_READ_URL", hide_env_values = true)]
    pub postgres_read_url: Option<String>,
}

/// The URL migrations are applied through.
#[derive(Args)]
pub struct MigrationOptions {
    /// The URL migrations use; unset uses `--postgres-url`. Migrations take a session advisory
    /// lock, so behind a transaction-mode pooler this names a direct connection to the primary.
    #[arg(long, env = "CODEHUB_POSTGRES_MIGRATE_URL", hide_env_values = true)]
    pub postgres_migrate_url: Option<String>,
}

impl fmt::Debug for ReplicaOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self { postgres_read_url } = self;
        f.debug_struct("ReplicaOptions")
            .field(
                "postgres_read_url",
                &postgres_read_url.as_deref().map(redact_url),
            )
            .finish()
    }
}

impl fmt::Debug for MigrationOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self {
            postgres_migrate_url,
        } = self;
        f.debug_struct("MigrationOptions")
            .field(
                "postgres_migrate_url",
                &postgres_migrate_url.as_deref().map(redact_url),
            )
            .finish()
    }
}

/// Opens the pool `options` name, with `read_url` as its read replica where one is given.
pub(crate) async fn open(
    options: &DatabaseOptions,
    read_url: Option<&str>,
) -> Result<Database, Error> {
    Database::open(PostgresOptions {
        url: &options.postgres_url,
        pool_size: options.postgres_pool_size,
        acquire_timeout: Duration::from_secs(options.postgres.postgres_acquire_timeout),
        statement_timeout: Duration::from_secs(options.postgres.postgres_statement_timeout),
        statement_cache: usize::try_from(options.postgres.postgres_statement_cache).unwrap_or(100),
        read_url,
    })
    .await
    .map_err(Error::Database)
}

/// Opens the pool a migration runs on: the migration URL where one is given, with no statement
/// timeout, which a migration may need to outlast.
pub(crate) async fn open_for_migration(
    options: &DatabaseOptions,
    migration: &MigrationOptions,
) -> Result<Database, Error> {
    Database::open(PostgresOptions {
        url: migration
            .postgres_migrate_url
            .as_deref()
            .unwrap_or(&options.postgres_url),
        pool_size: options.postgres_pool_size.min(MIGRATION_POOL_SIZE),
        acquire_timeout: Duration::from_secs(options.postgres.postgres_acquire_timeout),
        statement_timeout: Duration::ZERO,
        statement_cache: usize::try_from(options.postgres.postgres_statement_cache).unwrap_or(100),
        read_url: None,
    })
    .await
    .map_err(Error::Database)
}

#[cfg(test)]
mod tests {
    use crate::settings::{Cli, Role};

    fn migrate(words: &[&str]) -> crate::settings::MigrateOptions {
        let line = std::iter::once("codehub")
            .chain(["migrate", "--postgres-url=postgres://a/db"])
            .chain(words.iter().copied())
            .map(Into::into);
        match Cli::try_from_args(line).unwrap().role {
            Role::Migrate(options) => options,
            other @ (Role::Api(_) | Role::RunnerApi(_) | Role::Worker(_) | Role::All(_)) => {
                panic!("not the migrate role: {other:?}")
            }
        }
    }

    #[test]
    fn the_pool_settings_default_to_a_direct_connection() {
        let options = migrate(&[]);
        assert_eq!(options.database.postgres.postgres_acquire_timeout, 5);
        assert_eq!(options.database.postgres.postgres_statement_timeout, 30);
        assert_eq!(options.database.postgres.postgres_statement_cache, 100);
        assert_eq!(options.migration.postgres_migrate_url, None);
    }

    #[test]
    fn a_statement_cache_of_zero_and_no_statement_timeout_are_accepted() {
        let options = migrate(&[
            "--postgres-statement-cache=0",
            "--postgres-statement-timeout=0",
        ]);
        assert_eq!(options.database.postgres.postgres_statement_cache, 0);
        assert_eq!(options.database.postgres.postgres_statement_timeout, 0);
    }

    #[test]
    fn a_zero_acquire_timeout_is_refused() {
        let line = ["codehub", "migrate", "--postgres-url=postgres://a/db"]
            .into_iter()
            .chain(["--postgres-acquire-timeout=0"])
            .map(Into::into);
        assert!(Cli::try_from_args(line).is_err());
    }

    #[test]
    fn the_urls_print_without_their_passwords() {
        let options = migrate(&["--postgres-migrate-url=postgres://u:hunter2@direct/db"]);
        let printed = format!("{options:?}");
        assert!(!printed.contains("hunter2"), "{printed}");
        assert!(printed.contains("direct"), "{printed}");
    }
}
