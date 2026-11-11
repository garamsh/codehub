//! What the schema refuses by itself, whatever the application checks first.

#[cfg(test)]
pub mod common;

use common::Harness;
use sqlx::{AssertSqlSafe, Connection, Error, Executor, PgConnection};

/// Runs `statement` on `connection`.
///
/// # Errors
///
/// Returns the database's refusal.
#[cfg(test)]
async fn run(connection: &mut PgConnection, statement: &str) -> Result<(), Error> {
    connection
        .execute(AssertSqlSafe(statement.to_owned()))
        .await
        .map(|_| ())
}

/// The insert of a grant of `organisation` over `repository`, to a member of `acme`.
#[cfg(test)]
fn grant(organisation: &str, repository: &str) -> String {
    format!(
        "insert into grants (organisation, issuer, subject, repository, prefix, access) \
         values ('{organisation}', 'https://id.test', 'ada', \
         (select id from repositories where name = '{repository}'), ''::bytea, 'read')"
    )
}

#[tokio::test]
async fn refuses_a_grant_over_a_repository_of_another_organisation() {
    let harness = Harness::new().await;
    let mut connection = PgConnection::connect(&harness.url()).await.unwrap();
    for statement in [
        "insert into organisations (name) values ('acme'), ('globex')",
        "insert into repositories (organisation, name) values ('acme', 'own'), ('globex', 'other')",
        "insert into members (organisation, issuer, subject, role) \
         values ('acme', 'https://id.test', 'ada', 'member')",
    ] {
        run(&mut connection, statement).await.unwrap();
    }

    run(&mut connection, &grant("acme", "own")).await.unwrap();
    let refused = run(&mut connection, &grant("acme", "other"))
        .await
        .unwrap_err();

    let violation = refused.as_database_error().unwrap();
    assert_eq!(
        violation.constraint(),
        Some("grants_repository_organisation")
    );
    let granted: i64 = sqlx::query_scalar("select count(*) from grants")
        .fetch_one(&mut connection)
        .await
        .unwrap();
    assert_eq!(granted, 1);
}
