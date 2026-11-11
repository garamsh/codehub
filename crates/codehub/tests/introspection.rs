//! The shipped `codehub` binary in introspection mode, against the test identity provider
//! (Keycloak) and the services of `dev/check-env.sh`.
//!
//! `test_support::Server` always starts the binary in its default OIDC mode, so the introspection
//! server is spawned here.

#[cfg(test)]
pub mod common;

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};

use common::{Client, TestDatabase, env};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

/// A `codehub` in introspection mode, killed when dropped.
#[cfg(test)]
struct IntrospectionServer {
    child: Child,
    base: String,
}

#[cfg(test)]
impl IntrospectionServer {
    /// The command for `codehub` over `database`, in introspection mode with every setting.
    fn command(database: &TestDatabase) -> Command {
        let mut command = Command::new(Path::new(env!("CARGO_BIN_EXE_codehub")));
        command
            .env("CODEHUB_LISTEN", "127.0.0.1:0")
            .env("CODEHUB_POSTGRES_URL", database.url())
            .env("CODEHUB_S3_ENDPOINT", env("CODEHUB_TEST_S3_ENDPOINT"))
            .env("CODEHUB_S3_REGION", env("CODEHUB_TEST_S3_REGION"))
            .env("CODEHUB_S3_BUCKET", env("CODEHUB_TEST_S3_BUCKET"))
            .env("CODEHUB_S3_ACCESS_KEY", env("CODEHUB_TEST_S3_ACCESS_KEY"))
            .env("CODEHUB_S3_SECRET_KEY", env("CODEHUB_TEST_S3_SECRET_KEY"))
            .env("CODEHUB_IDENTITY", "introspection")
            .env(
                "CODEHUB_INTROSPECTION_ENDPOINT",
                format!(
                    "{}/protocol/openid-connect/token/introspect",
                    env("CODEHUB_TEST_KEYCLOAK_ISSUER")
                ),
            )
            .env(
                "CODEHUB_INTROSPECTION_CLIENT_ID",
                env("CODEHUB_TEST_KEYCLOAK_INTROSPECTION_CLIENT_ID"),
            )
            .env(
                "CODEHUB_INTROSPECTION_CLIENT_SECRET",
                env("CODEHUB_TEST_KEYCLOAK_INTROSPECTION_CLIENT_SECRET"),
            )
            .env(
                "CODEHUB_INTROSPECTION_ISSUER",
                env("CODEHUB_TEST_KEYCLOAK_ISSUER"),
            )
            .env(
                "CODEHUB_INTROSPECTION_AUDIENCE",
                env("CODEHUB_TEST_KEYCLOAK_AUDIENCE"),
            )
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        command
    }

    /// Starts the server and reads its stderr up to the line that names its address.
    fn start(mut command: Command) -> Self {
        let mut child = command.spawn().unwrap();
        let stderr = child.stderr.take().unwrap();
        let line = BufReader::new(stderr).lines().next().unwrap().unwrap();
        let address = line
            .strip_prefix("codehub listening on ")
            .unwrap_or_else(|| panic!("codehub did not start: {line}"));
        Self {
            base: format!("http://{address}"),
            child,
        }
    }
}

#[cfg(test)]
impl Drop for IntrospectionServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// An access token for alice from the Keycloak client `client` (`TEST` yields codehub's
/// audience, `OTHER` another one).
#[cfg(test)]
async fn token(client: &str) -> String {
    let issuer = env("CODEHUB_TEST_KEYCLOAK_ISSUER");
    let response = reqwest::Client::new()
        .post(format!("{issuer}/protocol/openid-connect/token"))
        .basic_auth(
            env(&format!("CODEHUB_TEST_KEYCLOAK_{client}_CLIENT_ID")),
            Some(env(&format!(
                "CODEHUB_TEST_KEYCLOAK_{client}_CLIENT_SECRET"
            ))),
        )
        .form(&[
            ("grant_type", "password"),
            ("username", "alice"),
            ("password", &env("CODEHUB_TEST_KEYCLOAK_PASSWORD_ALICE")),
        ])
        .send()
        .await
        .unwrap();
    assert!(
        response.status().is_success(),
        "Keycloak refused the token request"
    );
    let answer: Value = response.json().await.unwrap();
    answer["access_token"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn accepts_a_token_for_its_audience_and_refuses_one_for_another() {
    let database = TestDatabase::new().await;
    let server = IntrospectionServer::start(IntrospectionServer::command(&database));
    let create = json!({"name": "acme"});

    let (status, _) = Client::new(&server.base, Some(token("OTHER").await))
        .send(Method::POST, "/v1/orgs", Some(create.clone()))
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _) = Client::new(&server.base, Some(token("TEST").await))
        .send(Method::POST, "/v1/orgs", Some(create))
        .await;
    assert_eq!(status, StatusCode::CREATED);
}

#[tokio::test]
async fn does_not_start_without_the_introspection_settings() {
    let database = TestDatabase::new().await;
    let mut command = IntrospectionServer::command(&database);
    command.env_remove("CODEHUB_INTROSPECTION_CLIENT_SECRET");

    // A server that starts anyway would run for ever, so read one line and kill it.
    let mut child = command.spawn().unwrap();
    let stderr = child.stderr.take().unwrap();
    let line = BufReader::new(stderr).lines().next().unwrap().unwrap();
    let _ = child.kill();
    let _ = child.wait();

    assert!(
        !line.starts_with("codehub listening on "),
        "codehub started without the setting"
    );
    assert!(
        line.contains("--introspection-client-secret (CODEHUB_INTROSPECTION_CLIENT_SECRET)"),
        "{line}"
    );
}
