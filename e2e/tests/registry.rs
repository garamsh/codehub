//! The image registry (Distribution v3, the `registry` profile of `compose.yaml`) accepting the
//! tokens the built `codehub` signs (ADR 0053), driven by a real OCI client: a push and a pull
//! with a codehub token as the Basic password, and what a missing grant, a missing token and an
//! unknown repository are answered.
//!
//! These tests need the registry that `make e2e-registry` starts and stops, and `make e2e` skips
//! them. They share the registry's one token realm, the port `codehub` listens on, so all of it is
//! one test that runs its steps in order.

#[cfg(test)]
pub mod common;

use std::path::PathBuf;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use common::{Server, TestDatabase, TokenUser, env, keycloak_token};
use oci_client::client::{ClientConfig, ClientProtocol, Config, ImageLayer};
use oci_client::manifest::IMAGE_LAYER_MEDIA_TYPE;
use oci_client::secrets::RegistryAuth;
use oci_client::{Client, Reference};
use reqwest::StatusCode;
use serde_json::{Value, json};

/// A person of Keycloak and the token codehub accepts for them: the subject and issuer a grant
/// names, and the token a registry client presents as the Basic password.
#[cfg(test)]
struct Person {
    subject: String,
    issuer: String,
    token: String,
}

#[cfg(test)]
impl Person {
    async fn new(user: &str) -> Self {
        let token = keycloak_token("TEST", user).await;
        let claims = claims(&token);
        Self {
            subject: claims["sub"].as_str().unwrap().to_owned(),
            issuer: claims["iss"].as_str().unwrap().to_owned(),
            token,
        }
    }

    /// The credentials of a registry client: the user name is ignored, the password is the token.
    fn auth(&self) -> RegistryAuth {
        RegistryAuth::Basic("codehub".to_owned(), self.token.clone())
    }
}

/// The claims of the JWT `token`, whose signature is not checked: a test reads what it was given.
#[cfg(test)]
fn claims(token: &str) -> Value {
    let payload = token.split('.').nth(1).unwrap();
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).unwrap()).unwrap()
}

/// A client of the registry over plain HTTP, which is how the check registry listens.
#[cfg(test)]
fn client() -> Client {
    Client::new(ClientConfig {
        protocol: ClientProtocol::Http,
        ..ClientConfig::default()
    })
}

/// The image `web:latest` of the repository `org/repo` (a repository of codehub is `org/repo`, and
/// the registry's repository `org/repo/web`).
#[cfg(test)]
fn image(repository: &str) -> Reference {
    let host = env("CODEHUB_TEST_REGISTRY_URL");
    let host = host.strip_prefix("http://").unwrap();
    format!("{host}/{repository}/web:latest").parse().unwrap()
}

/// The layer of the small image every push carries.
#[cfg(test)]
const LAYER: &[u8] = b"a layer of the e2e image\n";

#[cfg(test)]
async fn push(
    person: &Person,
    repository: &str,
) -> Result<(), oci_client::errors::OciDistributionError> {
    let layers = [ImageLayer::new(
        LAYER,
        IMAGE_LAYER_MEDIA_TYPE.to_owned(),
        None,
    )];
    let config = Config::oci_v1(b"{}".to_vec(), None);
    client()
        .push(&image(repository), &layers, config, &person.auth(), None)
        .await
        .map(|_| ())
}

#[cfg(test)]
async fn pull(
    person: &Person,
    repository: &str,
) -> Result<Vec<u8>, oci_client::errors::OciDistributionError> {
    let data = client()
        .pull(
            &image(repository),
            &person.auth(),
            vec![IMAGE_LAYER_MEDIA_TYPE],
        )
        .await?;
    Ok(data.layers[0].data.to_vec())
}

/// The `access` claim of the token codehub gives `person` for `scope`, asked at the realm as the
/// registry's client does.
#[cfg(test)]
async fn granted(server: &Server, person: &Person, scope: &str) -> Value {
    let response = reqwest::Client::new()
        .get(format!("{}/v1/registry/token", server.base))
        .query(&[
            ("service", env("CODEHUB_TEST_REGISTRY_SERVICE").as_str()),
            ("scope", scope),
        ])
        .basic_auth("codehub", Some(&person.token))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    claims(body["token"].as_str().unwrap())["access"].clone()
}

#[tokio::test]
async fn registry_accepts_what_codehub_grants_and_refuses_the_rest() {
    let keys = PathBuf::from(env("CODEHUB_REGISTRY_E2E_DIR"));
    let database = TestDatabase::new().await;
    let server = Server::start_registry(&database, &keys);
    let alice = TokenUser::new(&server, "TEST", "alice").await;
    alice.ok(&["org", "create", "acme"]).await;
    alice.ok(&["repo", "create", "acme/app"]).await;
    alice.ok(&["repo", "create", "acme/pushonly"]).await;
    let (bob, carol) = (Person::new("bob").await, Person::new("carol").await);
    let grant = |repository: &'static str, person: &Person, flags: &'static [&'static str]| {
        let (alice, subject, issuer) = (&alice, person.subject.clone(), person.issuer.clone());
        async move {
            alice.ok(&["use", repository]).await;
            let mut arguments = vec!["member", "add", &subject, "--issuer", &issuer];
            arguments.extend(flags);
            alice.ok(&arguments).await;
        }
    };
    // Bob may pull and push acme/app, carol may only pull it, and bob may only push acme/pushonly.
    grant("acme/app", &bob, &["--image-pull", "--image-push"]).await;
    grant("acme/app", &carol, &["--image-pull"]).await;
    grant("acme/pushonly", &bob, &["--image-push"]).await;

    // Step 1: bob pushes a small image with his codehub token as the Basic password and pulls it
    // back.
    let pushed = push(&bob, "acme/app").await;
    let pulled = pull(&bob, "acme/app").await;
    eprintln!(
        "step 1: push {pushed:?}; pull {:?}",
        pulled.as_ref().map(Vec::len)
    );
    pushed.unwrap();
    assert_eq!(pulled.unwrap(), LAYER);

    // Step 2: carol holds image_pull and not image_push, and is refused a push. The control: the
    // same carol pulls what bob pushed.
    let refused = push(&carol, "acme/app").await;
    eprintln!("step 2: carol's push {refused:?}");
    assert!(refused.is_err());
    assert_eq!(pull(&carol, "acme/app").await.unwrap(), LAYER);

    // Step 3: bob holds image_push and not image_pull on acme/pushonly, and is refused a pull of
    // it. The control: the token codehub gives bob for it does grant push. A client cannot push
    // with that grant alone, because Distribution asks for pull as well as push of every push.
    let refused = pull(&bob, "acme/pushonly").await;
    let push_only = granted(&server, &bob, "repository:acme/pushonly/web:pull,push").await;
    eprintln!(
        "step 3: bob's pull of acme/pushonly {:?}; his token grants {push_only}",
        refused.as_ref().map(Vec::len)
    );
    assert!(refused.is_err());
    assert_eq!(
        push_only,
        json!([{"type": "repository", "name": "acme/pushonly/web", "actions": ["push"]}])
    );

    // Step 4: a request with no token is refused. The control: the registry answers the same
    // request with the token as the registry's client presents it.
    let host = env("CODEHUB_TEST_REGISTRY_URL");
    let anonymous = reqwest::Client::new()
        .get(format!("{host}/v2/acme/app/web/tags/list"))
        .send()
        .await
        .unwrap();
    let challenge = anonymous.headers()["www-authenticate"]
        .to_str()
        .unwrap()
        .to_owned();
    eprintln!("step 4: no token {} with {challenge}", anonymous.status());
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
    assert!(challenge.starts_with("Bearer realm="));
    let anonymous = client()
        .pull(
            &image("acme/app"),
            &RegistryAuth::Anonymous,
            vec![IMAGE_LAYER_MEDIA_TYPE],
        )
        .await;
    assert!(anonymous.is_err());

    // Step 5: a scope on a repository codehub does not hold is answered as a denied one is: the
    // token grants no action in either case (carol holds nothing on acme/pushonly), and the registry refuses the push in both.
    let unknown = granted(&server, &bob, "repository:acme/nothing/web:pull,push").await;
    let denied = granted(&server, &carol, "repository:acme/pushonly/web:pull,push").await;
    eprintln!("step 5: unknown repository grants {unknown}; denied scope grants {denied}");
    assert_eq!(
        unknown,
        json!([{"type": "repository", "name": "acme/nothing/web", "actions": []}])
    );
    assert_eq!(
        denied,
        json!([{"type": "repository", "name": "acme/pushonly/web", "actions": []}])
    );
    let missing = push(&bob, "acme/nothing").await;
    eprintln!("step 5: bob's push to an unknown repository {missing:?}");
    assert!(missing.is_err());
}
