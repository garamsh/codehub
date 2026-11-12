//! The shipped `codehub` binary in external authorisation mode: identity by introspection against
//! the test identity provider (Keycloak), decisions from the AuthZEN decision point (Cerbos, policy
//! in `dev/cerbos/policies`) and the services of `dev/check-env.sh`. In that policy alice may do
//! everything in `acme` and create repositories, but not organisations, in `beta`; bob may read
//! `acme/app` and write under `docs/` in it.

#[cfg(test)]
pub mod common;

use std::path::Path;
use std::process::Command;

use common::{Client, TestDatabase, count, env, introspection_settings, pairs};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use test_support::{Server, keycloak_token};

const BINARY: &str = env!("CARGO_BIN_EXE_codehub");

/// The settings of external mode: introspection against Keycloak, and the decision point with the
/// client its bearer token is obtained as.
#[cfg(test)]
fn external_settings() -> Vec<(&'static str, String)> {
    let mut settings = introspection_settings();
    settings.extend([
        ("CODEHUB_AUTHORISATION", "authzen".to_owned()),
        ("CODEHUB_AUTHZEN_URL", env("CODEHUB_TEST_AUTHZEN_URL")),
        (
            "CODEHUB_AUTHZEN_TOKEN_ENDPOINT",
            format!(
                "{}/protocol/openid-connect/token",
                env("CODEHUB_TEST_KEYCLOAK_ISSUER")
            ),
        ),
        (
            "CODEHUB_AUTHZEN_CLIENT_ID",
            env("CODEHUB_TEST_KEYCLOAK_AUTHZEN_CLIENT_ID"),
        ),
        (
            "CODEHUB_AUTHZEN_CLIENT_SECRET",
            env("CODEHUB_TEST_KEYCLOAK_AUTHZEN_CLIENT_SECRET"),
        ),
    ]);
    settings
}

/// A client of `server` presenting a Keycloak token for `user`.
#[cfg(test)]
async fn client(server: &Server, user: &str) -> Client {
    Client::new(&server.base, Some(keycloak_token("TEST", user).await))
}

/// Starts `codehub` in external mode over `database`.
#[cfg(test)]
fn start_external(database: &TestDatabase) -> Server {
    let settings = external_settings();
    Server::start_with(Path::new(BINARY), database, &pairs(&settings))
}

#[cfg(test)]
fn create_repository(organisation: &str) -> (Method, String, Option<Value>) {
    (
        Method::POST,
        format!("/v1/orgs/{organisation}/repos"),
        Some(json!({"name": "app"})),
    )
}

#[tokio::test]
async fn applies_the_decision_points_allow_and_deny_to_a_request() {
    let database = TestDatabase::new().await;
    let server = start_external(&database);
    let (method, path, body) = create_repository("acme");

    let (status, _) = client(&server, "alice")
        .await
        .send(method.clone(), &path, body.clone())
        .await;
    assert_eq!(status, StatusCode::CREATED);

    // The same request from bob, whom the policy does not allow it.
    let (status, problem) = client(&server, "bob")
        .await
        .send(
            Method::POST,
            "/v1/orgs/acme/repos",
            Some(json!({"name": "other"})),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(problem["detail"], "the action is not allowed");

    // The decision point also decides reads: bob may read acme/app, so a missing file is a
    // missing file; carol may not, so the repository is not disclosed.
    let read = "/v1/orgs/acme/repos/app/files/docs/a.txt";
    let (status, problem) = client(&server, "bob")
        .await
        .send(Method::GET, read, None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(problem["detail"], "no file at docs/a.txt");
    let (status, problem) = client(&server, "carol")
        .await
        .send(Method::GET, read, None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(problem["detail"], "no repository acme/app");
}

#[tokio::test]
async fn refuses_a_request_without_a_valid_token_in_external_mode() {
    let database = TestDatabase::new().await;
    let server = start_external(&database);
    let (method, path, body) = create_repository("acme");

    let (status, _) = Client::new(&server.base, None)
        .send(method.clone(), &path, body.clone())
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _) = client(&server, "alice")
        .await
        .send(method, &path, body)
        .await;
    assert_eq!(status, StatusCode::CREATED);
}

#[tokio::test]
async fn refuses_each_member_endpoint_with_403_saying_members_are_administered_outside() {
    let database = TestDatabase::new().await;
    let server = start_external(&database);
    let alice = client(&server, "alice").await;
    let (method, path, body) = create_repository("acme");
    // The control: alice is allowed everything in acme, including this.
    assert_eq!(alice.send(method, &path, body).await.0, StatusCode::CREATED);

    let principal = "issuer=http://127.0.0.1:18081/realms/codehub&subject=carol";
    let grant = json!({
        "issuer": "http://127.0.0.1:18081/realms/codehub",
        "subject": "carol",
        "prefix": "",
        "access": "read",
    });
    let endpoints = [
        (
            Method::PUT,
            "/v1/orgs/acme/members".to_owned(),
            Some(json!({
                "issuer": "http://127.0.0.1:18081/realms/codehub",
                "subject": "carol",
                "role": "member",
            })),
        ),
        (
            Method::DELETE,
            format!("/v1/orgs/acme/members?{principal}"),
            None,
        ),
        (
            Method::PUT,
            "/v1/orgs/acme/repos/app/grants".to_owned(),
            Some(grant),
        ),
        (
            Method::DELETE,
            format!("/v1/orgs/acme/repos/app/grants?{principal}&prefix=&access=read"),
            None,
        ),
    ];
    for (method, path, body) in endpoints {
        let (status, problem) = alice.send(method.clone(), &path, body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {path}");
        assert_eq!(
            problem["detail"], "members are administered outside codehub",
            "{method} {path}"
        );
    }
    assert_eq!(count(&database, "select count(*) from members").await, 0);
    assert_eq!(count(&database, "select count(*) from grants").await, 0);
}

#[tokio::test]
async fn creates_an_organisation_on_first_use_where_the_decision_point_allows_it() {
    let database = TestDatabase::new().await;
    let server = start_external(&database);
    let alice = client(&server, "alice").await;
    assert_eq!(
        count(&database, "select count(*) from organisations").await,
        0
    );

    let (method, path, body) = create_repository("acme");
    assert_eq!(alice.send(method, &path, body).await.0, StatusCode::CREATED);

    assert_eq!(
        count(
            &database,
            "select count(*) from organisations where name = 'acme'"
        )
        .await,
        1
    );
    assert_eq!(
        count(
            &database,
            "select count(*) from repositories where organisation = 'acme' and name = 'app'"
        )
        .await,
        1
    );
    // Visible through the API: alice, allowed to read it, gets a missing file and not a missing
    // repository.
    let (status, problem) = alice
        .send(Method::GET, "/v1/orgs/acme/repos/app/files/a.txt", None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(problem["detail"], "no file at a.txt");
}

#[tokio::test]
async fn does_not_create_an_organisation_where_the_decision_point_denies_it() {
    let database = TestDatabase::new().await;
    let server = start_external(&database);
    let alice = client(&server, "alice").await;

    // alice may create repositories in beta, and not the organisation.
    let (method, path, body) = create_repository("beta");
    let (status, problem) = alice.send(method, &path, body).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(problem["detail"], "no organisation beta");
    assert_eq!(
        count(
            &database,
            "select count(*) from organisations where name = 'beta'"
        )
        .await,
        0
    );
    assert_eq!(
        count(&database, "select count(*) from repositories").await,
        0
    );

    // The control: where it is allowed, the same call creates the organisation.
    let (method, path, body) = create_repository("acme");
    assert_eq!(alice.send(method, &path, body).await.0, StatusCode::CREATED);
    assert_eq!(
        count(
            &database,
            "select count(*) from organisations where name = 'acme'"
        )
        .await,
        1
    );
}

#[tokio::test]
async fn writes_no_admin_grant_when_an_organisation_is_created() {
    let external = TestDatabase::new().await;
    let server = start_external(&external);
    let (status, _) = client(&server, "alice")
        .await
        .send(Method::POST, "/v1/orgs", Some(json!({"name": "acme"})))
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(
        count(
            &external,
            "select count(*) from organisations where name = 'acme'"
        )
        .await,
        1
    );
    assert_eq!(count(&external, "select count(*) from members").await, 0);

    // The control: built-in mode, the same request, writes alice's admin membership.
    let built_in = TestDatabase::new().await;
    let settings = introspection_settings();
    let server = Server::start_with(Path::new(BINARY), &built_in, &pairs(&settings));
    let (status, _) = client(&server, "alice")
        .await
        .send(Method::POST, "/v1/orgs", Some(json!({"name": "acme"})))
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(count(&built_in, "select count(*) from members").await, 1);
}

#[tokio::test]
async fn does_not_start_without_the_authzen_settings() {
    let database = TestDatabase::new().await;
    let settings: Vec<_> = external_settings()
        .into_iter()
        .filter(|(name, _)| *name != "CODEHUB_AUTHZEN_CLIENT_SECRET")
        .collect();

    let started = Server::try_start_with(Path::new(BINARY), &database, &pairs(&settings));

    let Err(line) = started else {
        panic!("codehub started without the setting");
    };
    assert!(
        line.contains("--authzen-client-secret (CODEHUB_AUTHZEN_CLIENT_SECRET)"),
        "{line}"
    );
}

#[test]
fn lists_every_authorisation_setting_in_help() {
    let output = Command::new(BINARY).arg("--help").output().unwrap();
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    for flag in [
        "--authorisation",
        "--authzen-url",
        "--authzen-token-endpoint",
        "--authzen-client-id",
        "--authzen-client-secret",
        "--authzen-evaluations",
        "CODEHUB_AUTHORISATION",
        "CODEHUB_AUTHZEN_URL",
        "CODEHUB_AUTHZEN_TOKEN_ENDPOINT",
        "CODEHUB_AUTHZEN_CLIENT_ID",
        "CODEHUB_AUTHZEN_CLIENT_SECRET",
        "CODEHUB_AUTHZEN_EVALUATIONS",
    ] {
        assert!(help.contains(flag), "{flag} is not in --help");
    }
}
