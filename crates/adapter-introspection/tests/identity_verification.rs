//! The introspection adapter against the test identity provider (Keycloak), and against a stub
//! endpoint for the responses Keycloak cannot be made to produce.

use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use adapter_introspection::IntrospectionIdentityVerification;
use app::domain::credential::Credential;
use app::port::driven::identity_verification::{IdentityError, IdentityVerification};
use serde_json::{Value, json};
use test_support::{env, keycloak_token};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// The subject of alice's tokens, which `dev/keycloak/codehub-realm.json` fixes.
const ALICE: &str = "00000000-0000-4000-8000-00000000a11c";

#[cfg(test)]
fn credential(token: &str) -> Credential {
    Credential::from_str(token).unwrap()
}

/// The endpoints of the test identity provider, from its discovery document.
#[cfg(test)]
struct Provider {
    issuer: String,
    introspection: String,
    revocation: String,
}

#[cfg(test)]
impl Provider {
    async fn discover() -> Self {
        let issuer = env("CODEHUB_TEST_KEYCLOAK_ISSUER");
        let document: Value = reqwest::get(format!("{issuer}/.well-known/openid-configuration"))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let endpoint = |name: &str| document[name].as_str().unwrap().to_owned();
        Self {
            introspection: endpoint("introspection_endpoint"),
            revocation: endpoint("revocation_endpoint"),
            issuer,
        }
    }

    fn verifier(&self) -> IntrospectionIdentityVerification {
        self.verifier_with_secret(&env("CODEHUB_TEST_KEYCLOAK_INTROSPECTION_CLIENT_SECRET"))
    }

    fn verifier_with_secret(&self, secret: &str) -> IntrospectionIdentityVerification {
        IntrospectionIdentityVerification::new(
            &self.introspection,
            &env("CODEHUB_TEST_KEYCLOAK_INTROSPECTION_CLIENT_ID"),
            secret,
            &self.issuer,
            &env("CODEHUB_TEST_KEYCLOAK_AUDIENCE"),
        )
        .unwrap()
    }

    /// What the introspection endpoint answers the adapter's client for `token`.
    async fn introspect(&self, token: &str) -> Value {
        reqwest::Client::new()
            .post(&self.introspection)
            .basic_auth(
                env("CODEHUB_TEST_KEYCLOAK_INTROSPECTION_CLIENT_ID"),
                Some(env("CODEHUB_TEST_KEYCLOAK_INTROSPECTION_CLIENT_SECRET")),
            )
            .form(&[("token", token)])
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn verifies_a_token_for_codehubs_audience_to_the_principal_of_its_user() {
    let provider = Provider::discover().await;
    let verifier = provider.verifier();

    let alice = verifier
        .verify(&credential(&keycloak_token("TEST", "alice").await))
        .await
        .unwrap();
    let bob = verifier
        .verify(&credential(&keycloak_token("TEST", "bob").await))
        .await
        .unwrap();

    assert_eq!(alice.issuer(), provider.issuer);
    assert_eq!(alice.subject(), ALICE);
    assert_ne!(alice, bob);
}

#[tokio::test]
async fn rejects_a_token_for_another_audience() {
    let provider = Provider::discover().await;
    let verifier = provider.verifier();
    let accepted = keycloak_token("TEST", "alice").await;
    let other = keycloak_token("OTHER", "alice").await;

    // The control: the provider calls the other token active, so the audience is what refuses it.
    let introspected = provider.introspect(&other).await;
    assert_eq!(introspected["active"], json!(true));
    assert!(
        !introspected["aud"]
            .as_array()
            .unwrap()
            .contains(&json!(env("CODEHUB_TEST_KEYCLOAK_AUDIENCE")))
    );
    assert!(verifier.verify(&credential(&accepted)).await.is_ok());
    assert_eq!(
        verifier.verify(&credential(&other)).await,
        Err(IdentityError::Rejected)
    );
}

#[tokio::test]
async fn rejects_an_expired_token() {
    let provider = Provider::discover().await;
    let verifier = provider.verifier();
    let token = keycloak_token("SHORT", "alice").await;
    assert!(verifier.verify(&credential(&token)).await.is_ok());

    tokio::time::sleep(Duration::from_secs(3)).await;

    assert_eq!(
        verifier.verify(&credential(&token)).await,
        Err(IdentityError::Rejected)
    );
}

#[tokio::test]
async fn rejects_a_revoked_token() {
    let provider = Provider::discover().await;
    let verifier = provider.verifier();
    let token = keycloak_token("TEST", "alice").await;
    assert!(verifier.verify(&credential(&token)).await.is_ok());

    let revoked = reqwest::Client::new()
        .post(&provider.revocation)
        .basic_auth(
            env("CODEHUB_TEST_KEYCLOAK_TEST_CLIENT_ID"),
            Some(env("CODEHUB_TEST_KEYCLOAK_TEST_CLIENT_SECRET")),
        )
        .form(&[("token", token.as_str())])
        .send()
        .await
        .unwrap();
    assert!(revoked.status().is_success());

    assert_eq!(
        verifier.verify(&credential(&token)).await,
        Err(IdentityError::Rejected)
    );
}

#[tokio::test]
async fn rejects_a_token_the_provider_does_not_know() {
    let provider = Provider::discover().await;

    assert_eq!(
        provider.verifier().verify(&credential("not-a-token")).await,
        Err(IdentityError::Rejected)
    );
}

#[tokio::test]
async fn fails_where_the_provider_refuses_the_clients_credentials() {
    let provider = Provider::discover().await;
    let token = keycloak_token("TEST", "alice").await;

    let refused = provider
        .verifier_with_secret("wrong-secret")
        .verify(&credential(&token))
        .await;

    assert!(
        matches!(refused, Err(IdentityError::Backend(_))),
        "{refused:?}"
    );
    assert!(
        provider
            .verifier()
            .verify(&credential(&token))
            .await
            .is_ok()
    );
}

/// A local endpoint that answers every request with one status and body, and keeps the requests.
#[cfg(test)]
struct Stub {
    url: String,
    requests: Arc<Mutex<Vec<String>>>,
}

#[cfg(test)]
impl Stub {
    async fn start(status: &str, body: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/introspect", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (kept, status, body) = (Arc::clone(&requests), status.to_owned(), body.to_owned());
        tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buffer = vec![0; 4096];
                let read = socket.read(&mut buffer).await.unwrap();
                let request = String::from_utf8_lossy(buffer.get(..read).unwrap()).into_owned();
                kept.lock().unwrap().push(request);
                let response = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\n\
                     content-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });
        Self { url, requests }
    }

    async fn answering(body: Value) -> Self {
        Self::start("200 OK", &body.to_string()).await
    }

    fn verifier(&self) -> IntrospectionIdentityVerification {
        IntrospectionIdentityVerification::new(
            &self.url,
            "resource-client",
            "resource-secret",
            "https://configured.example",
            "codehub",
        )
        .unwrap()
    }
}

#[tokio::test]
async fn posts_the_token_authenticated_as_the_client() {
    let stub = Stub::answering(json!({"active": true, "aud": "codehub", "sub": "u1"})).await;

    stub.verifier()
        .verify(&credential("the-bearer-token"))
        .await
        .unwrap();

    let requests = stub.requests.lock().unwrap();
    let request = requests.first().unwrap();
    assert!(request.starts_with("POST /introspect "), "{request}");
    // base64 of `resource-client:resource-secret`
    assert!(
        request.contains("authorization: Basic cmVzb3VyY2UtY2xpZW50OnJlc291cmNlLXNlY3JldA=="),
        "{request}"
    );
    assert!(request.ends_with("token=the-bearer-token"), "{request}");
}

#[tokio::test]
async fn accepts_the_audience_as_a_string_or_in_an_array() {
    let one = Stub::answering(json!({"active": true, "aud": "codehub", "sub": "u1"})).await;
    let many = Stub::answering(json!({"active": true, "aud": ["x", "codehub"], "sub": "u1"})).await;
    let other = Stub::answering(json!({"active": true, "aud": ["x"], "sub": "u1"})).await;
    let absent = Stub::answering(json!({"active": true, "sub": "u1"})).await;

    assert!(one.verifier().verify(&credential("t")).await.is_ok());
    assert!(many.verifier().verify(&credential("t")).await.is_ok());
    assert_eq!(
        other.verifier().verify(&credential("t")).await,
        Err(IdentityError::Rejected)
    );
    assert_eq!(
        absent.verifier().verify(&credential("t")).await,
        Err(IdentityError::Rejected)
    );
}

#[tokio::test]
async fn names_the_configured_issuer_where_the_response_has_none() {
    let without = Stub::answering(json!({"active": true, "aud": "codehub", "sub": "u1"})).await;
    let with = Stub::answering(
        json!({"active": true, "aud": "codehub", "sub": "u1", "iss": "https://from.response"}),
    )
    .await;

    let fallback = without.verifier().verify(&credential("t")).await.unwrap();
    let named = with.verifier().verify(&credential("t")).await.unwrap();

    assert_eq!(fallback.issuer(), "https://configured.example");
    assert_eq!(named.issuer(), "https://from.response");
}

#[tokio::test]
async fn rejects_an_inactive_token_and_one_without_a_subject() {
    let inactive = Stub::answering(json!({"active": false, "aud": "codehub", "sub": "u1"})).await;
    let no_subject = Stub::answering(json!({"active": true, "aud": "codehub"})).await;

    assert_eq!(
        inactive.verifier().verify(&credential("t")).await,
        Err(IdentityError::Rejected)
    );
    assert_eq!(
        no_subject.verifier().verify(&credential("t")).await,
        Err(IdentityError::Rejected)
    );
}

#[tokio::test]
async fn fails_on_a_non_success_status_and_an_unparsable_body() {
    let refused = Stub::start(
        "500 Internal Server Error",
        r#"{"active":true,"aud":"codehub","sub":"u1"}"#,
    )
    .await;
    let garbled = Stub::start("200 OK", "not json").await;
    let no_active = Stub::answering(json!({"aud": "codehub", "sub": "u1"})).await;

    for stub in [refused, garbled, no_active] {
        let result = stub.verifier().verify(&credential("t")).await;
        assert!(
            matches!(result, Err(IdentityError::Backend(_))),
            "{result:?}"
        );
    }
}

#[tokio::test]
async fn fails_where_the_endpoint_is_unreachable() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/introspect", listener.local_addr().unwrap());
    drop(listener);
    let verifier =
        IntrospectionIdentityVerification::new(&url, "c", "s", "https://i.example", "codehub")
            .unwrap();

    let result = verifier.verify(&credential("t")).await;

    assert!(
        matches!(result, Err(IdentityError::Backend(_))),
        "{result:?}"
    );
}
