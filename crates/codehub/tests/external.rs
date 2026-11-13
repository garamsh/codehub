//! The shipped `codehub` binary in external authorisation mode: identity by introspection against
//! the test identity provider (Keycloak), decisions from the AuthZEN decision point (Cerbos, policy
//! in `dev/cerbos/policies`) and the services of `dev/check-env.sh`. In that policy alice may do
//! everything in `acme` and create repositories, but not organisations, in `beta`; bob may read
//! `acme/app` and write under `docs/` in it.

#[cfg(test)]
pub mod common;

use std::path::Path;
use std::process::Command;

use aws_lc_rs::encoding::{AsDer, Pkcs8V1Der};
use aws_lc_rs::rsa::{KeyPair, KeySize};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
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
async fn answers_the_id_the_database_holds_for_a_repository_it_creates() {
    let database = TestDatabase::new().await;
    let server = start_external(&database);
    let alice = client(&server, "alice").await;
    let (method, path, body) = create_repository("acme");

    let (status, answer) = alice.send(method, &path, body).await;

    assert_eq!(status, StatusCode::CREATED);
    let held = count(&database, "select id from repositories where name = 'app'").await;
    assert_eq!(answer["id"].as_i64(), Some(held), "{answer}");
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

/// The settings of the `grn` profile with `private_key_jwt`, the key being read from `key_file`.
#[cfg(test)]
fn grn_settings(key_file: &str) -> Vec<(&'static str, String)> {
    let mut settings: Vec<_> = external_settings()
        .into_iter()
        .filter(|(name, _)| !name.starts_with("CODEHUB_AUTHZEN_CLIENT_"))
        .collect();
    settings.extend([
        ("CODEHUB_AUTHZEN_PROFILE", "grn".to_owned()),
        (
            "CODEHUB_AUTHZEN_CLIENT_AUTHENTICATION",
            "private-key-jwt".to_owned(),
        ),
        (
            "CODEHUB_AUTHZEN_CLIENT_ID",
            "grn:{organisation}/iam/service/codehub".to_owned(),
        ),
        ("CODEHUB_AUTHZEN_KEY_FILE", key_file.to_owned()),
        ("CODEHUB_AUTHZEN_ISSUER", "https://garam.test".to_owned()),
    ]);
    settings
}

/// The line `codehub` refuses to start with, under `settings`.
#[cfg(test)]
fn refusal(database: &TestDatabase, settings: &[(&'static str, String)]) -> String {
    match Server::try_start_with(Path::new(BINARY), database, &pairs(settings)) {
        Err(line) => line,
        Ok(_) => panic!("codehub started under {settings:?}"),
    }
}

/// Replaces the value of `name` in `settings`, or removes it where `value` is `None`.
#[cfg(test)]
fn with_setting(
    mut settings: Vec<(&'static str, String)>,
    name: &'static str,
    value: Option<&str>,
) -> Vec<(&'static str, String)> {
    settings.retain(|(setting, _)| *setting != name);
    if let Some(value) = value {
        settings.push((name, value.to_owned()));
    }
    settings
}

/// A freshly generated 2048-bit RSA key as a PEM PKCS#8 file in the tests' temporary directory,
/// named `name`; returns its path.
#[cfg(test)]
fn write_rsa_key(name: &str) -> String {
    let pair = KeyPair::generate(KeySize::Rsa2048).unwrap();
    let der = AsDer::<Pkcs8V1Der>::as_der(&pair).unwrap();
    let body = STANDARD.encode(der.as_ref());
    let lines: Vec<&str> = body
        .as_bytes()
        .chunks(64)
        .map(|line| std::str::from_utf8(line).unwrap())
        .collect();
    let pem = format!(
        "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n",
        lines.join("\n")
    );
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::write(&path, pem).unwrap();
    path.to_str().unwrap().to_owned()
}

#[tokio::test]
async fn does_not_start_where_the_private_key_jwt_settings_are_wrong() {
    let database = TestDatabase::new().await;
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR"));
    let garbage = directory.join("authzen-not-a-key.pem");
    std::fs::write(&garbage, "NOT-A-KEY-SECRET-CONTENT").unwrap();
    let garbage = garbage.to_str().unwrap();
    let missing = directory.join("authzen-no-such-key.pem");
    let missing = missing.to_str().unwrap();
    let valid = grn_settings(&write_rsa_key("authzen-rsa-key.pem"));

    // The control: each refusal below differs from these settings in one setting, and these start.
    let started = Server::try_start_with(Path::new(BINARY), &database, &pairs(&valid));
    assert!(started.is_ok(), "{:?}", started.err());
    drop(started);

    let cases = [
        (
            "no key file",
            with_setting(valid.clone(), "CODEHUB_AUTHZEN_KEY_FILE", None),
            "--authzen-key-file (CODEHUB_AUTHZEN_KEY_FILE)",
        ),
        (
            "no issuer",
            with_setting(valid.clone(), "CODEHUB_AUTHZEN_ISSUER", None),
            "--authzen-issuer (CODEHUB_AUTHZEN_ISSUER)",
        ),
        (
            "no client id",
            with_setting(valid.clone(), "CODEHUB_AUTHZEN_CLIENT_ID", None),
            "--authzen-client-id (CODEHUB_AUTHZEN_CLIENT_ID)",
        ),
        (
            "a key file that does not exist",
            with_setting(valid.clone(), "CODEHUB_AUTHZEN_KEY_FILE", Some(missing)),
            "named by CODEHUB_AUTHZEN_KEY_FILE failed",
        ),
        (
            "a template without the organisation",
            with_setting(
                valid.clone(),
                "CODEHUB_AUTHZEN_CLIENT_ID",
                Some("grn:acme/iam/service/codehub"),
            ),
            "(CODEHUB_AUTHZEN_CLIENT_ID, CODEHUB_AUTHZEN_KEY_FILE) are invalid: the client id \
             template has no {organisation}",
        ),
        (
            "a key that is not PEM PKCS#8",
            with_setting(valid.clone(), "CODEHUB_AUTHZEN_KEY_FILE", Some(garbage)),
            "(CODEHUB_AUTHZEN_CLIENT_ID, CODEHUB_AUTHZEN_KEY_FILE) are invalid: the key is not a \
             PEM PKCS#8 private key",
        ),
        (
            "the grn profile with built-in authorisation",
            with_setting(valid.clone(), "CODEHUB_AUTHORISATION", Some("postgres")),
            "CODEHUB_AUTHZEN_PROFILE=grn needs CODEHUB_AUTHORISATION=authzen",
        ),
        (
            "the grn profile with a client secret",
            with_setting(
                valid,
                "CODEHUB_AUTHZEN_CLIENT_AUTHENTICATION",
                Some("client-secret"),
            ),
            "CODEHUB_AUTHZEN_PROFILE=grn needs CODEHUB_AUTHZEN_CLIENT_AUTHENTICATION=private-key-jwt",
        ),
    ];

    for (case, settings, expected) in cases {
        let line = refusal(&database, &settings);
        assert!(line.contains(expected), "{case}: {line}");
        assert!(!line.contains("NOT-A-KEY-SECRET-CONTENT"), "{case}: {line}");
    }
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
        "--authzen-profile",
        "--authzen-client-authentication",
        "--authzen-key-file",
        "--authzen-key-id",
        "--authzen-issuer",
        "--authzen-audience",
        "CODEHUB_AUTHZEN_PROFILE",
        "CODEHUB_AUTHZEN_CLIENT_AUTHENTICATION",
        "CODEHUB_AUTHZEN_KEY_FILE",
        "CODEHUB_AUTHZEN_KEY_ID",
        "CODEHUB_AUTHZEN_ISSUER",
        "CODEHUB_AUTHZEN_AUDIENCE",
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
