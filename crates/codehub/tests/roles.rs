//! The shipped `codehub` binary as each of its roles (ADR 0052): the routes each serves, the work
//! each does, and the schema each waits for, against the services the checks run
//! (`dev/test.env`).

#[cfg(test)]
pub mod common;

use std::process::{Command, Output};
use std::time::{Duration, Instant};

use common::{Client, Server, TestDatabase, count};
use reqwest::{Method, StatusCode};
use sqlx::{Connection, PgConnection};

const BINARY: &str = env!("CARGO_BIN_EXE_codehub");

/// Runs `codehub` with `arguments` and the database `database` names, and returns what it did.
#[cfg(test)]
fn codehub(arguments: &[&str], database: &TestDatabase) -> Output {
    Command::new(BINARY)
        .args(arguments)
        .env("CODEHUB_POSTGRES_URL", database.url())
        .output()
        .unwrap()
}

/// Applies the migrations to `database` as the `migrate` role.
#[cfg(test)]
fn migrate(database: &TestDatabase) {
    let output = codehub(&["migrate"], database);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// The status `method` `path` answers on `server`, without a token.
#[cfg(test)]
async fn status(server: &Server, method: Method, path: &str) -> StatusCode {
    Client::new(&server.base, None)
        .send(method, path, None)
        .await
        .0
}

#[tokio::test]
async fn api_does_not_serve_the_runner_routes_that_runner_api_does() {
    let database = TestDatabase::new().await;
    migrate(&database);
    let claim = "/v1/runner/jobs/claim";

    let runner_api = Server::start_role("runner-api", &database, &[]);
    // The control: runner-api routes the claim, so it asks who is claiming.
    assert_eq!(
        status(&runner_api, Method::POST, claim).await,
        StatusCode::UNAUTHORIZED
    );

    let api = Server::start_role("api", &database, &[]);
    assert_eq!(
        status(&api, Method::POST, claim).await,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn runner_api_does_not_serve_the_user_routes_that_api_does() {
    let database = TestDatabase::new().await;
    migrate(&database);
    let repositories = "/v1/orgs/acme/repos";

    let api = Server::start_role("api", &database, &[]);
    // The control: api routes the creation, so it asks who is creating.
    assert_eq!(
        status(&api, Method::POST, repositories).await,
        StatusCode::UNAUTHORIZED
    );

    let runner_api = Server::start_role("runner-api", &database, &[]);
    assert_eq!(
        status(&runner_api, Method::POST, repositories).await,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn migrate_applies_the_migrations_and_exits_zero_each_time() {
    let database = TestDatabase::new().await;
    let applied = "select count(*) from _sqlx_migrations where success";

    migrate(&database);
    let first = count(&database, applied).await;
    assert!(first > 0);

    migrate(&database);
    assert_eq!(count(&database, applied).await, first);
}

#[tokio::test]
async fn the_worker_delivers_the_directory_outbox() {
    let database = TestDatabase::new().await;
    migrate(&database);
    let mut connection = PgConnection::connect(&database.url()).await.unwrap();
    sqlx::query(
        "insert into directory_outbox (repository, organisation, name, kind, sequence) \
         values (1, 'acme', 'app', 'created', 1)",
    )
    .execute(&mut connection)
    .await
    .unwrap();
    // The control: with no worker running the entry stays.
    let entries = "select count(*) from directory_outbox";
    assert_eq!(count(&database, entries).await, 1);

    // The default `none` accepts and discards every entry it is given.
    let _worker = Server::start_role("worker", &database, &[]);

    let deadline = Instant::now() + Duration::from_secs(30);
    while count(&database, entries).await != 0 {
        assert!(
            Instant::now() < deadline,
            "the worker did not relay the entry"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

#[tokio::test]
async fn a_serving_role_is_alive_but_not_ready_until_the_database_is_migrated() {
    for role in ["api", "runner-api", "worker"] {
        let database = TestDatabase::new().await;
        let server = Server::start_role(role, &database, &[]);
        let anonymous = Client::new(&server.base, None);

        assert_eq!(
            status(&server, Method::GET, "/healthz").await,
            StatusCode::OK,
            "{role}"
        );
        let (ready, problem) = anonymous.send(Method::GET, "/readyz", None).await;
        assert_eq!(ready, StatusCode::SERVICE_UNAVAILABLE, "{role}");
        assert_eq!(
            problem["detail"], "the database schema is older than this binary",
            "{role}"
        );

        // The control: the same process is ready once the schema is there.
        migrate(&database);
        assert_eq!(
            status(&server, Method::GET, "/readyz").await,
            StatusCode::OK,
            "{role}"
        );
    }
}

#[tokio::test]
async fn a_role_does_not_migrate_where_it_starts() {
    let database = TestDatabase::new().await;
    let _api = Server::start_role("api", &database, &[]);

    let mut connection = PgConnection::connect(&database.url()).await.unwrap();
    let table: Option<String> = sqlx::query_scalar("select to_regclass('_sqlx_migrations')::text")
        .fetch_one(&mut connection)
        .await
        .unwrap();
    assert_eq!(table, None);
}

#[tokio::test]
async fn a_role_lists_only_its_own_options_in_its_help() {
    let database = TestDatabase::new().await;
    let help =
        |role: &str| String::from_utf8(codehub(&[role, "--help"], &database).stdout).unwrap();

    let api = help("api");
    assert!(api.contains("--s3-bucket"), "{api}");
    assert!(!api.contains("--runner-lease"), "{api}");

    let worker = help("worker");
    assert!(worker.contains("--directory-changes"), "{worker}");
    assert!(!worker.contains("--s3-bucket"), "{worker}");

    let migrate = help("migrate");
    assert!(migrate.contains("--postgres-url"), "{migrate}");
    assert!(!migrate.contains("--listen"), "{migrate}");
}

#[tokio::test]
async fn a_role_refuses_an_option_it_does_not_have() {
    let database = TestDatabase::new().await;
    // The control: the option is one the role takes.
    assert!(
        codehub(&["migrate", "--postgres-pool-size=2"], &database)
            .status
            .success()
    );

    let output = codehub(&["migrate", "--listen=127.0.0.1:0"], &database);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("--listen"));
}
