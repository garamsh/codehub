//! The one place a statement of this crate is made (ADR 0054).
//!
//! sqlx names every statement it sends as a persistent one and closes a named statement only when
//! it falls out of the connection's statement cache, so with the cache off a named statement is
//! never closed. A pooler that hands one server session to many clients then meets the same name
//! twice. A statement sent unnamed is parsed and dropped with its portal, which any such pooler
//! passes. [`Statements`] decides, from the cache size the pool was opened with, whether a
//! statement is persistent, and a test fails where `sqlx::query`, `sqlx::query_as` or
//! `sqlx::query_scalar` is called anywhere else in the crate's sources, so a new statement cannot
//! forget it.

use sqlx::postgres::{PgArguments, PgRow};
use sqlx::query::{Query, QueryAs, QueryScalar};
use sqlx::{FromRow, Postgres, SqlSafeStr};

/// Whether the statements of a pool are persistent: named, and kept in the connection's cache.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Statements {
    persistent: bool,
}

impl Statements {
    /// The statements of a pool whose connections keep `cache` prepared statements; none keeps
    /// every statement unnamed.
    pub(crate) const fn new(cache: usize) -> Self {
        Self {
            persistent: cache > 0,
        }
    }

    /// A statement that returns no typed row.
    pub(crate) fn query<'q>(self, sql: impl SqlSafeStr) -> Query<'q, Postgres, PgArguments> {
        sqlx::query(sql).persistent(self.persistent)
    }

    /// A statement whose rows are read as `O`.
    pub(crate) fn query_as<'q, O>(
        self,
        sql: impl SqlSafeStr,
    ) -> QueryAs<'q, Postgres, O, PgArguments>
    where
        O: for<'r> FromRow<'r, PgRow>,
    {
        sqlx::query_as(sql).persistent(self.persistent)
    }

    /// A statement whose one column is read as `O`.
    pub(crate) fn query_scalar<'q, O>(
        self,
        sql: impl SqlSafeStr,
    ) -> QueryScalar<'q, Postgres, O, PgArguments>
    where
        (O,): for<'r> FromRow<'r, PgRow>,
    {
        sqlx::query_scalar(sql).persistent(self.persistent)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::Path;
    use std::time::Duration;

    use app::domain::address::Address;
    use app::domain::changeset::Changeset;
    use app::domain::repository::RepositoryId;
    use app::port::driven::changeset_log::{ChangesetLog, Ref};
    use app::port::driven::derived_data::DerivedData;
    use app::port::driven::job_queue::JobQueue;
    use test_support::TestDatabase;

    use crate::{
        Database, PostgresChangesetLog, PostgresDerivedData, PostgresJobQueue, PostgresOptions,
    };

    /// The prepared statements the one connection of a pool holds after the adapters ran, for a
    /// pool opened with `cache`.
    async fn prepared_after_the_adapters_ran(cache: usize) -> i64 {
        let created = TestDatabase::new().await;
        let url = created.url();
        // The migrations send named statements of their own, so a pool of its own applies them.
        let migrating = Database::connect(&url, 1).await.unwrap();
        migrating.migrate().await.unwrap();
        let database = Database::open(PostgresOptions {
            url: &url,
            pool_size: 1,
            acquire_timeout: Duration::from_secs(5),
            statement_timeout: Duration::ZERO,
            statement_cache: cache,
            read_url: None,
        })
        .await
        .unwrap();
        let log = PostgresChangesetLog::new(&database);
        let derived = PostgresDerivedData::new(&database);
        let queue = PostgresJobQueue::new(&database);
        // No repository exists, so most of these are refused; each is still sent to the server.
        let repository = RepositoryId::new(1);
        let id = Address::of(b"changeset");
        let changeset = Changeset {
            parents: Vec::new(),
            author: "ada".to_owned(),
            timestamp: 1,
            message: "one".to_owned(),
            changes: BTreeMap::new(),
        };
        let _ = log.append(repository, changeset).await;
        let _ = log.get(repository, id).await;
        let _ = log.read_ref(repository, Ref::Main).await;
        let _ = log.compare_and_swap(repository, Ref::Main, None, id).await;
        let _ = derived.put_trees(repository, id, id, BTreeMap::new()).await;
        let _ = derived.root(repository, id).await;
        let _ = queue.claim(&[], 0, 30).await;
        let _ = queue.expire(0).await;
        assert!(database.is_writable().await.unwrap());
        assert!(database.schema_is_current().await.unwrap());

        sqlx::query_scalar("select count(*) from pg_prepared_statements")
            .persistent(false)
            .fetch_one(&database.pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn with_no_statement_cache_a_connection_prepares_no_statement() {
        assert_eq!(prepared_after_the_adapters_ran(0).await, 0);
    }

    #[tokio::test]
    async fn with_a_statement_cache_a_connection_keeps_its_prepared_statements() {
        assert!(prepared_after_the_adapters_ran(100).await >= 4);
    }

    #[test]
    fn no_statement_is_made_but_through_statements() {
        let sources = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut strays = Vec::new();
        for entry in fs::read_dir(sources).unwrap() {
            let path = entry.unwrap().path();
            if path.file_name().is_some_and(|name| name == "statements.rs") {
                continue;
            }
            let text = fs::read_to_string(&path).unwrap();
            for (number, line) in text.lines().enumerate() {
                if ["sqlx::query(", "sqlx::query_as", "sqlx::query_scalar"]
                    .iter()
                    .any(|call| line.contains(call))
                {
                    strays.push(format!("{}:{}", path.display(), number + 1));
                }
            }
        }
        assert!(
            strays.is_empty(),
            "statements made without Statements, which sets whether they are persistent: {strays:?}"
        );
    }
}
