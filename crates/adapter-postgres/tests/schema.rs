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

#[tokio::test]
async fn refuses_a_runner_registration_for_a_repository_of_another_organisation() {
    let harness = Harness::new().await;
    let mut connection = PgConnection::connect(&harness.url()).await.unwrap();
    for statement in [
        "insert into organisations (name) values ('acme'), ('globex')",
        "insert into repositories (organisation, name) values ('acme', 'own'), ('globex', 'other')",
    ] {
        run(&mut connection, statement).await.unwrap();
    }
    let registration = |organisation: &str, repository: &str| {
        format!(
            "insert into runner_registrations (organisation, repository, issuer, subject, labels) \
             values ('{organisation}', (select id from repositories where name = '{repository}'), \
             'https://id.test', 'bot', '{{}}')"
        )
    };

    run(&mut connection, &registration("acme", "own"))
        .await
        .unwrap();
    let refused = run(&mut connection, &registration("acme", "other"))
        .await
        .unwrap_err();

    let violation = refused.as_database_error().unwrap();
    assert_eq!(
        violation.constraint(),
        Some("runner_registrations_repository_organisation")
    );
    let registered: i64 = sqlx::query_scalar("select count(*) from runner_registrations")
        .fetch_one(&mut connection)
        .await
        .unwrap();
    assert_eq!(registered, 1);
}

#[tokio::test]
async fn refuses_a_running_job_that_holds_no_lease() {
    let harness = Harness::new().await;
    let mut connection = PgConnection::connect(&harness.url()).await.unwrap();
    for statement in [
        "insert into organisations (name) values ('acme')",
        "insert into repositories (organisation, name) values ('acme', 'app')",
        "insert into changesets (repository, id, encoding) \
         values ((select id from repositories), decode(repeat('00', 32), 'hex'), ''::bytea)",
    ] {
        run(&mut connection, statement).await.unwrap();
    }
    let job = |state: &str, lease: &str, deadline: &str| {
        format!(
            "insert into jobs (organisation, repository, changeset, check_name, command, \
             check_labels, blocking, timeout_seconds, state, runner, lease_expires_at, deadline) \
             values ('acme', (select id from repositories), decode(repeat('00', 32), 'hex'), \
             'unit', 'run', '{{}}', true, 600, '{state}', null, {lease}, {deadline})"
        )
    };

    run(&mut connection, &job("queued", "null", "null"))
        .await
        .unwrap();
    run(&mut connection, &job("running", "30", "630"))
        .await
        .unwrap();
    let refused = run(&mut connection, &job("running", "null", "630"))
        .await
        .unwrap_err();

    let violation = refused.as_database_error().unwrap();
    assert_eq!(violation.constraint(), Some("jobs_lease"));
    let held: i64 = sqlx::query_scalar("select count(*) from jobs")
        .fetch_one(&mut connection)
        .await
        .unwrap();
    assert_eq!(held, 2);
}
