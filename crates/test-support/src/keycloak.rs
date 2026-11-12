//! Tokens and `codehub` settings for the test identity provider, Keycloak.

use crate::env;

/// Keycloak's token endpoint, which the password grant, the RFC 8693 exchange and a decision
/// point's client credentials use.
///
/// # Panics
///
/// Panics if `CODEHUB_TEST_KEYCLOAK_ISSUER` is not set.
#[must_use]
pub fn token_endpoint() -> String {
    format!(
        "{}/protocol/openid-connect/token",
        env("CODEHUB_TEST_KEYCLOAK_ISSUER")
    )
}

/// The settings that put `codehub` in introspection mode against Keycloak, as environment
/// variables: its endpoint, the introspection client, and the audience `codehub`.
///
/// # Panics
///
/// Panics if a variable of the check environment is not set.
#[must_use]
pub fn introspection_settings() -> Vec<(&'static str, String)> {
    vec![
        ("CODEHUB_IDENTITY", "introspection".to_owned()),
        (
            "CODEHUB_INTROSPECTION_ENDPOINT",
            format!("{}/introspect", token_endpoint()),
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

/// An access token for `user` (`alice`, `bob`, `carol`) from the password grant of a client of the
/// realm `CODEHUB_TEST_KEYCLOAK_ISSUER`. `client` names the client by the infix of its variables:
/// `TEST`, `OTHER`, `SHORT` or `UPSTREAM`, as `CODEHUB_TEST_KEYCLOAK_<client>_CLIENT_ID` and
/// `_CLIENT_SECRET` hold them.
///
/// # Panics
///
/// Panics if a variable is not set or Keycloak does not answer with an access token.
pub async fn keycloak_token(client: &str, user: &str) -> String {
    let password = env(&format!(
        "CODEHUB_TEST_KEYCLOAK_PASSWORD_{}",
        user.to_uppercase()
    ));
    let response = reqwest::Client::new()
        .post(token_endpoint())
        .basic_auth(
            env(&format!("CODEHUB_TEST_KEYCLOAK_{client}_CLIENT_ID")),
            Some(env(&format!(
                "CODEHUB_TEST_KEYCLOAK_{client}_CLIENT_SECRET"
            ))),
        )
        .form(&[
            ("grant_type", "password"),
            ("username", user),
            ("password", &password),
        ])
        .send()
        .await
        .unwrap();
    let status = response.status();
    let answer: serde_json::Value = response.json().await.unwrap();
    assert!(
        status.is_success(),
        "Keycloak refused {user}'s token request: {answer}"
    );
    answer
        .get("access_token")
        .and_then(serde_json::Value::as_str)
        .expect("the answer has an access token")
        .to_owned()
}
