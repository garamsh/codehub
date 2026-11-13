//! The shipped `codehub` binary against the services the checks run (`dev/check-env.sh`):
//! PostgreSQL, MinIO and Dex. Each test starts its own process over a database of its own.

#[cfg(test)]
pub mod common;

use common::{Client, Server, TestDatabase, env, token};
use reqwest::{Method, StatusCode};
use serde_json::json;
use sqlx::{AssertSqlSafe, Connection, Executor, PgConnection};

const CONTENT: &str = "hello from codehub\n";

#[tokio::test]
async fn lands_a_file_and_reads_it_back() {
    let database = TestDatabase::new().await;
    let server = Server::start(&database);
    let alice = Client::new(&server.base, Some(token("alice").await));

    alice.create_repository("acme", "app").await;
    assert_eq!(alice.read_file("acme", "app", "docs/hello.txt").await, None);
    alice
        .land_file("acme", "app", "work", "docs/hello.txt", CONTENT)
        .await;

    assert_eq!(
        alice.read_file("acme", "app", "docs/hello.txt").await,
        Some(CONTENT.to_owned())
    );
}

#[tokio::test]
async fn refuses_a_request_without_a_token() {
    let database = TestDatabase::new().await;
    let server = Server::start(&database);
    let create = json!({"name": "acme"});

    let (status, _) = Client::new(&server.base, None)
        .send(Method::POST, "/v1/orgs", Some(create.clone()))
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _) = Client::new(&server.base, Some(token("alice").await))
        .send(Method::POST, "/v1/orgs", Some(create))
        .await;
    assert_eq!(status, StatusCode::CREATED);
}

#[tokio::test]
async fn exits_zero_on_sigterm() {
    let database = TestDatabase::new().await;
    let mut server = Server::start(&database);

    assert!(server.signal("TERM").success());
}

#[tokio::test]
async fn exits_zero_on_sigint() {
    let database = TestDatabase::new().await;
    let mut server = Server::start(&database);

    assert!(server.signal("INT").success());
}

#[tokio::test]
async fn answers_the_probes_without_a_token_while_its_dependencies_answer() {
    let database = TestDatabase::new().await;
    let server = Server::start(&database);
    let anonymous = Client::new(&server.base, None);

    for path in ["/healthz", "/readyz"] {
        let (status, _) = anonymous.send(Method::GET, path, None).await;
        assert_eq!(status, StatusCode::OK, "{path}");
    }
}

#[tokio::test]
async fn is_not_ready_once_postgresql_is_unreachable_and_still_alive() {
    let database = TestDatabase::new().await;
    let server = Server::start(&database);
    let anonymous = Client::new(&server.base, None);
    let (status, _) = anonymous.send(Method::GET, "/readyz", None).await;
    assert_eq!(status, StatusCode::OK);

    // Dropping the database closes the server's connections and refuses new ones.
    let url = database.url();
    let name = url.rsplit('/').next().unwrap();
    let mut admin = PgConnection::connect(&env("CODEHUB_TEST_POSTGRES_URL"))
        .await
        .unwrap();
    admin
        .execute(AssertSqlSafe(format!("drop database {name} with (force)")))
        .await
        .unwrap();

    let (status, problem) = anonymous.send(Method::GET, "/readyz", None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(problem["detail"], "PostgreSQL did not answer");
    let (status, _) = anonymous.send(Method::GET, "/healthz", None).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn serves_a_landed_file_after_a_restart() {
    let database = TestDatabase::new().await;
    let mut first = Server::start(&database);
    let alice = token("alice").await;
    let client = Client::new(&first.base, Some(alice.clone()));
    client.create_repository("acme", "app").await;
    client
        .land_file("acme", "app", "work", "docs/hello.txt", CONTENT)
        .await;
    assert!(first.signal("TERM").success());

    let second = Server::start(&database);
    let client = Client::new(&second.base, Some(alice));
    assert_eq!(
        client.read_file("acme", "app", "docs/hello.txt").await,
        Some(CONTENT.to_owned())
    );
}
