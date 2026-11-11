//! The shipped `codehub` binary in introspection mode, against the test identity provider
//! (Keycloak) and the services of `dev/check-env.sh`.

#[cfg(test)]
pub mod common;

use std::path::Path;

use common::{Client, TestDatabase, env};
use reqwest::{Method, StatusCode};
use serde_json::json;
use test_support::{Server, keycloak_token};

/// The settings that put `codehub` in introspection mode against Keycloak, as environment
/// variables.
#[cfg(test)]
fn introspection_settings() -> Vec<(&'static str, String)> {
    vec![
        ("CODEHUB_IDENTITY", "introspection".to_owned()),
        (
            "CODEHUB_INTROSPECTION_ENDPOINT",
            format!(
                "{}/protocol/openid-connect/token/introspect",
                env("CODEHUB_TEST_KEYCLOAK_ISSUER")
            ),
        ),
        (
            "CODEHUB_INTROSPECTION_CLIENT_ID",
            env("CODEHUB_TEST_KEYCLOAK_INTROSPECTION_CLIENT_ID"),
        ),
        (
            "CODEHUB_INTROSPECTION_CLIENT_SECRET",
            env("CODEHUB_TEST_KEYCLOAK_INTROSPECTION_CLIENT_SECRET"),
        ),
        (
            "CODEHUB_INTROSPECTION_ISSUER",
            env("CODEHUB_TEST_KEYCLOAK_ISSUER"),
        ),
        (
            "CODEHUB_INTROSPECTION_AUDIENCE",
            env("CODEHUB_TEST_KEYCLOAK_AUDIENCE"),
        ),
    ]
}

/// `settings` as the borrowed pairs [`Server::start_with`] takes.
#[cfg(test)]
fn pairs<'a>(settings: &'a [(&'static str, String)]) -> Vec<(&'a str, &'a str)> {
    settings
        .iter()
        .map(|(name, value)| (*name, value.as_str()))
        .collect()
}

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
