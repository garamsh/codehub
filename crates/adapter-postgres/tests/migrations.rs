//! Whether a database has the schema this build embeds.

use adapter_postgres::Database;
use sqlx::{AssertSqlSafe, Connection, Executor, PgConnection};
use test_support::TestDatabase;

/// Runs `statement` on the database at `url`.
#[cfg(test)]
async fn run(url: &str, statement: &str) {
    let mut connection = PgConnection::connect(url).await.unwrap();
    connection
        .execute(AssertSqlSafe(statement.to_owned()))
        .await
        .unwrap();
}

#[tokio::test]
async fn an_empty_database_is_not_current_and_a_migrated_one_is() {
    let database = TestDatabase::new().await;
    let connected = Database::connect(&database.url(), 2).await.unwrap();
    assert_eq!(connected.schema_is_current().await, Ok(false));

    connected.migrate().await.unwrap();
    assert_eq!(connected.schema_is_current().await, Ok(true));
}

#[tokio::test]
async fn a_database_short_of_the_latest_migration_is_not_current() {
    let database = TestDatabase::new().await;
    let connected = Database::connect(&database.url(), 2).await.unwrap();
    connected.migrate().await.unwrap();
    assert_eq!(connected.schema_is_current().await, Ok(true));

    run(
        &database.url(),
        "delete from _sqlx_migrations where version = (select max(version) from _sqlx_migrations)",
    )
    .await;
    assert_eq!(connected.schema_is_current().await, Ok(false));
}

#[tokio::test]
async fn a_database_with_a_newer_schema_is_current() {
    let database = TestDatabase::new().await;
    let connected = Database::connect(&database.url(), 2).await.unwrap();
    connected.migrate().await.unwrap();

    run(
        &database.url(),
        "insert into _sqlx_migrations (version, description, success, checksum, execution_time) \
         values (9999999, 'newer', true, ''::bytea, 0)",
    )
    .await;
    assert_eq!(connected.schema_is_current().await, Ok(true));
}
