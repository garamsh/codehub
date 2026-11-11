//! The shipped `codehub` binary against the services the checks run (`dev/check-env.sh`):
//! PostgreSQL, MinIO and Dex. Each test starts its own process over a database of its own.

#[cfg(test)]
pub mod common;

use common::{Client, Server, TestDatabase, token};
use reqwest::{Method, StatusCode};
use serde_json::json;

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
