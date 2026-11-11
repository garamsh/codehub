//! The shipped `codehub` binary in introspection mode, against the test identity provider
//! (Keycloak) and the services of `dev/check-env.sh`.

#[cfg(test)]
pub mod common;

use std::path::Path;

use common::{Client, TestDatabase, introspection_settings, pairs};
use reqwest::{Method, StatusCode};
use serde_json::json;
use test_support::{Server, keycloak_token};

#[tokio::test]
async fn accepts_a_token_for_its_audience_and_refuses_one_for_another() {
    let database = TestDatabase::new().await;
    let settings = introspection_settings();
    let server = Server::start_with(
        Path::new(env!("CARGO_BIN_EXE_codehub")),
        &database,
        &pairs(&settings),
    );
    let create = json!({"name": "acme"});

    let (status, _) = Client::new(&server.base, Some(keycloak_token("OTHER", "alice").await))
        .send(Method::POST, "/v1/orgs", Some(create.clone()))
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _) = Client::new(&server.base, Some(keycloak_token("TEST", "alice").await))
        .send(Method::POST, "/v1/orgs", Some(create))
        .await;
    assert_eq!(status, StatusCode::CREATED);
}

#[tokio::test]
async fn does_not_start_without_the_introspection_settings() {
    let database = TestDatabase::new().await;
    let settings: Vec<_> = introspection_settings()
        .into_iter()
        .filter(|(name, _)| *name != "CODEHUB_INTROSPECTION_CLIENT_SECRET")
        .collect();

    let started = Server::try_start_with(
        Path::new(env!("CARGO_BIN_EXE_codehub")),
        &database,
        &pairs(&settings),
    );

    let Err(line) = started else {
        panic!("codehub started without the setting");
    };
    assert!(
        line.contains("--introspection-client-secret (CODEHUB_INTROSPECTION_CLIENT_SECRET)"),
        "{line}"
    );
}
