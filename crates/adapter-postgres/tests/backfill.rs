//! Migration 0006 over a database that already holds repositories: each gets one creation entry
//! with sequence 1 in the directory outbox, and its change counter is 1.

use sqlx::{Connection, PgConnection};
use test_support::TestDatabase;

/// The migrations before 0006, in order.
#[cfg(test)]
const BEFORE: [&str; 5] = [
    include_str!("../migrations/0001_initial.sql"),
    include_str!("../migrations/0002_grant_repository_organisation.sql"),
    include_str!("../migrations/0003_runners_and_jobs.sql"),
    include_str!("../migrations/0004_revert_reservations.sql"),
    include_str!("../migrations/0005_failed_reverts.sql"),
];

/// Migration 0006, the one under test.
#[cfg(test)]
const OUTBOX: &str = include_str!("../migrations/0006_directory_outbox.sql");

#[tokio::test]
async fn gives_each_existing_repository_one_creation_entry_with_sequence_1() {
    let database = TestDatabase::new().await;
    let mut connection = PgConnection::connect(&database.url()).await.unwrap();
    for migration in BEFORE {
        sqlx::raw_sql(migration)
            .execute(&mut connection)
            .await
            .unwrap();
    }
    sqlx::raw_sql(
        "insert into organisations (name) values ('acme'), ('beta'); \
         insert into repositories (organisation, name) \
         values ('acme', 'app'), ('acme', 'docs'), ('beta', 'app');",
    )
    .execute(&mut connection)
    .await
    .unwrap();

    sqlx::raw_sql(OUTBOX)
        .execute(&mut connection)
        .await
        .unwrap();

    let entries: Vec<(String, String, String, i64)> = sqlx::query_as(
        "select organisation, name, kind, sequence from directory_outbox order by repository",
    )
    .fetch_all(&mut connection)
    .await
    .unwrap();
    let expected: Vec<(String, String, String, i64)> =
        [("acme", "app"), ("acme", "docs"), ("beta", "app")]
            .into_iter()
            .map(|(organisation, name)| {
                (
                    organisation.to_owned(),
                    name.to_owned(),
                    "created".to_owned(),
                    1,
                )
            })
            .collect();
    assert_eq!(entries, expected);
    let counters: Vec<i64> = sqlx::query_scalar("select change_sequence from repositories")
        .fetch_all(&mut connection)
        .await
        .unwrap();
    assert_eq!(counters, vec![1, 1, 1]);
}
