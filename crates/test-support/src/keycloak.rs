//! Tokens from the test identity provider, Keycloak.

use crate::env;

/// An access token for `user` (`alice`, `bob`, `carol`) from the password grant of a client of the
/// realm `CODEHUB_TEST_KEYCLOAK_ISSUER`. `client` names the client by the infix of its variables:
/// `TEST`, `OTHER`, `SHORT` or `UPSTREAM`, as `CODEHUB_TEST_KEYCLOAK_<client>_CLIENT_ID` and
/// `_CLIENT_SECRET` hold them.
///
/// # Panics
///
/// Panics if a variable is not set or Keycloak does not answer with an access token.
pub async fn keycloak_token(client: &str, user: &str) -> String {
    let issuer = env("CODEHUB_TEST_KEYCLOAK_ISSUER");
    let password = env(&format!(
        "CODEHUB_TEST_KEYCLOAK_PASSWORD_{}",
        user.to_uppercase()
    ));
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
    answer["access_token"]
        .as_str()
        .expect("the answer has an access token")
        .to_owned()
}
