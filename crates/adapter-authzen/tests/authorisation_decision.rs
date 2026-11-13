//! The AuthZEN adapter against the test decision point (Cerbos, with Keycloak issuing its bearer
//! token), and against a stub server for the requests it sends and the answers the decision point
//! cannot be made to give.

use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use adapter_authzen::{AuthzenAuthorisationDecision, Profile};
use app::domain::name::{OrganisationName, RepositoryName};
use app::domain::path::{Component, Path};
use app::domain::principal::Principal;
use app::domain::repository::RepositoryId;
use app::port::driven::authorisation_decision::{
    Action, AuthorisationDecision, AuthorisationError, Decision, Resource,
};
use aws_lc_rs::encoding::{AsDer, Pkcs8V1Der};
use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::rsa::{KeyPair as RsaKeyPair, KeySize};
use aws_lc_rs::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair as _};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use oauth_client::{Authentication, TokenClient};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[cfg(test)]
fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} is not set; run through `make test`"))
}

#[cfg(test)]
fn organisation(name: &str) -> OrganisationName {
    OrganisationName::from_str(name).unwrap()
}

#[cfg(test)]
fn org(name: &str) -> Resource {
    Resource::Organisation(organisation(name))
}

#[cfg(test)]
fn repository(organisation_name: &str, name: &str) -> Resource {
    Resource::Repository {
        organisation: organisation(organisation_name),
        name: RepositoryName::from_str(name).unwrap(),
        repository: RepositoryId::new(7),
    }
}

#[cfg(test)]
fn path(organisation_name: &str, name: &str, path: &str) -> Resource {
    Resource::Path {
        organisation: organisation(organisation_name),
        name: RepositoryName::from_str(name).unwrap(),
        repository: RepositoryId::new(7),
        path: Path::from_str(path).unwrap(),
    }
}

/// The principal `subject` of the test identity provider.
#[cfg(test)]
fn principal(subject: &str) -> Principal {
    Principal::new(&env("CODEHUB_TEST_KEYCLOAK_ISSUER"), subject).unwrap()
}

#[cfg(test)]
fn alice() -> Principal {
    principal(&env("CODEHUB_TEST_KEYCLOAK_SUBJECT_ALICE"))
}

#[cfg(test)]
fn bob() -> Principal {
    principal(&env("CODEHUB_TEST_KEYCLOAK_SUBJECT_BOB"))
}

/// A token client for `token_endpoint` that authenticates as `client_id` with `secret`.
#[cfg(test)]
fn secret_tokens(token_endpoint: &str, client_id: &str, secret: &str) -> Arc<TokenClient> {
    Arc::new(
        TokenClient::new(
            token_endpoint,
            None,
            Authentication::ClientSecret {
                client_id: client_id.to_owned(),
                client_secret: secret.to_owned(),
            },
        )
        .unwrap(),
    )
}

#[cfg(test)]
fn keycloak_token_endpoint() -> String {
    format!(
        "{}/protocol/openid-connect/token",
        env("CODEHUB_TEST_KEYCLOAK_ISSUER")
    )
}

/// An adapter for the test decision point, with a token from the test identity provider as the
/// client with `secret`.
#[cfg(test)]
fn real_adapter_with_secret(secret: &str) -> AuthzenAuthorisationDecision {
    AuthzenAuthorisationDecision::new(
        &env("CODEHUB_TEST_AUTHZEN_URL"),
        secret_tokens(
            &keycloak_token_endpoint(),
            &env("CODEHUB_TEST_KEYCLOAK_AUTHZEN_CLIENT_ID"),
            secret,
        ),
        Profile::Plain,
    )
    .unwrap()
}

#[cfg(test)]
fn real_adapter() -> AuthzenAuthorisationDecision {
    real_adapter_with_secret(&env("CODEHUB_TEST_KEYCLOAK_AUTHZEN_CLIENT_SECRET"))
}

#[cfg(test)]
async fn decide(
    adapter: &AuthzenAuthorisationDecision,
    subject: &Principal,
    action: Action,
    resource: &Resource,
) -> Decision {
    adapter.decide(subject, action, resource).await.unwrap()
}

#[tokio::test]
async fn decides_actions_on_organisations() {
    let adapter = real_adapter();

    assert_eq!(
        decide(&adapter, &alice(), Action::CreateOrganisation, &org("acme")).await,
        Decision::Allow
    );
    assert_eq!(
        decide(&adapter, &alice(), Action::AdministerMembers, &org("acme")).await,
        Decision::Allow
    );
    assert_eq!(
        decide(&adapter, &alice(), Action::CreateRepository, &org("beta")).await,
        Decision::Allow
    );
    // The control for the denials: the same principal and organisation, a granted action.
    assert_eq!(
        decide(&adapter, &alice(), Action::CreateOrganisation, &org("beta")).await,
        Decision::Deny
    );
    assert_eq!(
        decide(
            &adapter,
            &alice(),
            Action::CreateOrganisation,
            &org("gamma")
        )
        .await,
        Decision::Deny
    );
    assert_eq!(
        decide(&adapter, &bob(), Action::CreateOrganisation, &org("acme")).await,
        Decision::Deny
    );
}

#[tokio::test]
async fn decides_actions_on_repositories() {
    let adapter = real_adapter();
    let app = repository("acme", "app");

    assert_eq!(
        decide(&adapter, &bob(), Action::Read, &app).await,
        Decision::Allow
    );
    assert_eq!(
        decide(&adapter, &alice(), Action::Write, &app).await,
        Decision::Allow
    );
    // A repository asked without a path means "write somewhere in it", the question before an
    // upload, as the built-in adapter reads it: bob, who may write under docs/, is allowed it.
    assert_eq!(
        decide(&adapter, &bob(), Action::Write, &app).await,
        Decision::Allow
    );
    // Controls: bob may not read another repository, nor alice one outside acme.
    assert_eq!(
        decide(&adapter, &bob(), Action::Read, &repository("acme", "lib")).await,
        Decision::Deny
    );
    assert_eq!(
        decide(&adapter, &alice(), Action::Read, &repository("beta", "app")).await,
        Decision::Deny
    );
}

#[tokio::test]
async fn decides_a_path_by_whole_components_of_the_granted_prefix() {
    let adapter = real_adapter();
    let write = |p: &str| path("acme", "app", p);

    for granted in ["docs", "docs/x", "docs/x/y.md"] {
        assert_eq!(
            decide(&adapter, &bob(), Action::Write, &write(granted)).await,
            Decision::Allow,
            "{granted}"
        );
    }
    // A sibling that shares the prefix's characters but not its component, and paths outside it.
    for refused in ["docsx/y", "docs-x", "src/main.rs", "x/docs"] {
        assert_eq!(
            decide(&adapter, &bob(), Action::Write, &write(refused)).await,
            Decision::Deny,
            "{refused}"
        );
    }
    // The control: bob reads every path of the repository, so the refusals above are write's.
    assert_eq!(
        decide(&adapter, &bob(), Action::Read, &write("docsx/y")).await,
        Decision::Allow
    );
    assert_eq!(
        decide(&adapter, &alice(), Action::Write, &write("docsx/y")).await,
        Decision::Allow
    );
}

#[tokio::test]
async fn decides_for_the_principal_of_its_issuer_and_subject_together() {
    let adapter = real_adapter();
    let elsewhere = Principal::new("https://elsewhere.example", alice().subject()).unwrap();

    assert_eq!(
        decide(&adapter, &alice(), Action::Read, &repository("acme", "app")).await,
        Decision::Allow
    );
    assert_eq!(
        decide(
            &adapter,
            &elsewhere,
            Action::Read,
            &repository("acme", "app")
        )
        .await,
        Decision::Deny
    );
}

#[tokio::test]
async fn fails_where_the_token_endpoint_refuses_the_client() {
    let refused = real_adapter_with_secret("wrong-secret")
        .decide(&alice(), Action::Read, &repository("acme", "app"))
        .await;

    assert!(
        matches!(&refused, Err(AuthorisationError::Backend(text)) if !text.contains("wrong-secret")),
        "{refused:?}"
    );
    assert_eq!(
        real_adapter()
            .decide(&alice(), Action::Read, &repository("acme", "app"))
            .await,
        Ok(Decision::Allow)
    );
}

#[tokio::test]
async fn fails_where_the_decision_point_is_unreachable() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let adapter = AuthzenAuthorisationDecision::new(
        &url,
        secret_tokens(
            &keycloak_token_endpoint(),
            &env("CODEHUB_TEST_KEYCLOAK_AUTHZEN_CLIENT_ID"),
            &env("CODEHUB_TEST_KEYCLOAK_AUTHZEN_CLIENT_SECRET"),
        ),
        Profile::Plain,
    )
    .unwrap();

    let result = adapter
        .decide(&alice(), Action::Read, &repository("acme", "app"))
        .await;

    assert!(
        matches!(result, Err(AuthorisationError::Backend(_))),
        "{result:?}"
    );
}

/// What a stub answers: a status line and a JSON body.
#[cfg(test)]
type Reply = (&'static str, String);

/// A local server that answers each request through `handler`, given the request as text, and
/// keeps the requests.
#[cfg(test)]
struct Stub {
    url: String,
    requests: Arc<Mutex<Vec<String>>>,
}

#[cfg(test)]
impl Stub {
    async fn start(handler: impl Fn(&str) -> Reply + Send + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let kept = Arc::clone(&requests);
        tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let request = read_request(&mut socket).await;
                let (status, body) = handler(&request);
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

    fn adapter(&self) -> AuthzenAuthorisationDecision {
        AuthzenAuthorisationDecision::new(
            &self.url,
            secret_tokens(
                &format!("{}/token", self.url),
                "resource-client",
                "resource-secret",
            ),
            Profile::Plain,
        )
        .unwrap()
    }

    /// An adapter that sends decisions on many resources as one evaluations request where the
    /// metadata advertises it.
    fn adapter_with_evaluations(&self) -> AuthzenAuthorisationDecision {
        self.adapter().with_access_evaluations(true)
    }

    /// The requests received at `route`, as text.
    fn requests_to(&self, route: &str) -> Vec<String> {
        let prefix = format!("POST {route} ");
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request.starts_with(&prefix))
            .cloned()
            .collect()
    }
}

/// One whole HTTP request: the head, and as many body bytes as its content length names.
#[cfg(test)]
async fn read_request(socket: &mut tokio::net::TcpStream) -> String {
    let mut received = Vec::new();
    let mut buffer = vec![0; 4096];
    loop {
        let read = socket.read(&mut buffer).await.unwrap();
        received.extend_from_slice(buffer.get(..read).unwrap());
        let text = String::from_utf8_lossy(&received).into_owned();
        if let Some((head, body)) = text.split_once("\r\n\r\n") {
            let length = head
                .to_lowercase()
                .lines()
                .find_map(|line| line.strip_prefix("content-length: ")?.parse().ok())
                .unwrap_or(0);
            if body.len() >= length || read == 0 {
                return text;
            }
        } else if read == 0 {
            return text;
        }
    }
}

/// The JSON body of a request.
#[cfg(test)]
fn body_of(request: &str) -> Value {
    serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap()
}

#[cfg(test)]
fn token_reply(token: &str, expires_in: u64) -> Reply {
    (
        "200 OK",
        json!({"access_token": token, "token_type": "Bearer", "expires_in": expires_in})
            .to_string(),
    )
}

#[cfg(test)]
fn decision_reply(decision: bool) -> Reply {
    ("200 OK", json!({"decision": decision}).to_string())
}

/// A stub that issues a numbered token, valid for `expires_in` seconds, and answers every
/// evaluation with `evaluation`.
#[cfg(test)]
async fn stub_with(expires_in: u64, evaluation: Reply) -> Stub {
    let issued = AtomicUsize::new(0);
    Stub::start(move |request| {
        if request.starts_with("POST /token ") {
            let number = issued.fetch_add(1, Ordering::SeqCst) + 1;
            token_reply(&format!("token-{number}"), expires_in)
        } else {
            evaluation.clone()
        }
    })
    .await
}

#[tokio::test]
async fn sends_the_mapped_request_for_each_resource_kind() {
    let stub = stub_with(3600, decision_reply(true)).await;
    let adapter = stub.adapter();
    let subject = Principal::new("https://issuer.example", "user-1").unwrap();

    for (action, resource) in [
        (Action::CreateOrganisation, org("acme")),
        (Action::Read, repository("acme", "app")),
        (Action::Write, path("acme", "app", "docs/guide/a.md")),
    ] {
        adapter.decide(&subject, action, &resource).await.unwrap();
    }

    let subject_value = json!({
        "type": "principal",
        "id": "user-1",
        "properties": {"issuer": "https://issuer.example"},
    });
    let repository_properties = json!({"organisation": "acme", "repository": "app"});
    let sent: Vec<Value> = stub
        .requests_to("/access/v1/evaluation")
        .iter()
        .map(|request| body_of(request))
        .collect();
    assert_eq!(
        sent,
        vec![
            json!({
                "subject": subject_value,
                "action": {"name": "create_organisation"},
                "resource": {"type": "organisation", "id": "acme"},
            }),
            json!({
                "subject": subject_value,
                "action": {"name": "read"},
                "resource": {
                    "type": "repository",
                    "id": "acme/app",
                    "properties": repository_properties,
                },
            }),
            json!({
                "subject": subject_value,
                "action": {"name": "write"},
                "resource": {
                    "type": "repository",
                    "id": "acme/app",
                    "properties": {
                        "organisation": "acme",
                        "repository": "app",
                        "path": "docs/guide/a.md",
                    },
                },
            }),
        ]
    );
}

#[tokio::test]
async fn names_every_action_on_the_wire() {
    let stub = stub_with(3600, decision_reply(true)).await;
    let adapter = stub.adapter();
    let subject = Principal::new("https://issuer.example", "user-1").unwrap();

    for action in [
        Action::CreateOrganisation,
        Action::CreateRepository,
        Action::AdministerMembers,
        Action::Read,
        Action::Write,
    ] {
        adapter
            .decide(&subject, action, &org("acme"))
            .await
            .unwrap();
    }

    let names: Vec<String> = stub
        .requests_to("/access/v1/evaluation")
        .iter()
        .map(|request| {
            body_of(request)["action"]["name"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(
        names,
        [
            "create_organisation",
            "create_repository",
            "administer_members",
            "read",
            "write"
        ]
    );
}

#[tokio::test]
async fn reads_the_decision_member_as_allow_or_deny() {
    let allow = stub_with(3600, decision_reply(true)).await;
    let deny = stub_with(3600, decision_reply(false)).await;
    let subject = Principal::new("https://issuer.example", "user-1").unwrap();

    assert_eq!(
        allow
            .adapter()
            .decide(&subject, Action::Read, &repository("acme", "app"))
            .await,
        Ok(Decision::Allow)
    );
    assert_eq!(
        deny.adapter()
            .decide(&subject, Action::Read, &repository("acme", "app"))
            .await,
        Ok(Decision::Deny)
    );
}

#[tokio::test]
async fn obtains_its_token_by_client_credentials_and_presents_it_as_a_bearer() {
    let stub = stub_with(3600, decision_reply(true)).await;
    let subject = Principal::new("https://issuer.example", "user-1").unwrap();

    stub.adapter()
        .decide(&subject, Action::Read, &repository("acme", "app"))
        .await
        .unwrap();

    let token_requests = stub.requests_to("/token");
    let token_request = token_requests.first().unwrap();
    // base64 of `resource-client:resource-secret`
    assert!(
        token_request.contains("authorization: Basic cmVzb3VyY2UtY2xpZW50OnJlc291cmNlLXNlY3JldA=="),
        "{token_request}"
    );
    assert!(
        token_request.ends_with("grant_type=client_credentials"),
        "{token_request}"
    );
    let evaluations = stub.requests_to("/access/v1/evaluation");
    let evaluation = evaluations.first().unwrap();
    assert!(
        evaluation.contains("authorization: Bearer token-1"),
        "{evaluation}"
    );
}

#[tokio::test]
async fn form_urlencodes_the_client_credentials_before_basic_encoding_them() {
    let stub = stub_with(3600, decision_reply(true)).await;
    let adapter = AuthzenAuthorisationDecision::new(
        &stub.url,
        secret_tokens(&format!("{}/token", stub.url), "cli:ent", "p:w%d s"),
        Profile::Plain,
    )
    .unwrap();
    let subject = Principal::new("https://issuer.example", "user-1").unwrap();

    adapter
        .decide(&subject, Action::Read, &repository("acme", "app"))
        .await
        .unwrap();

    let token_requests = stub.requests_to("/token");
    let token_request = token_requests.first().unwrap();
    // base64 of `cli%3Aent:p%3Aw%25d+s`
    assert!(
        token_request.contains("authorization: Basic Y2xpJTNBZW50OnAlM0F3JTI1ZCtz"),
        "{token_request}"
    );
}

#[tokio::test]
async fn keeps_its_token_until_it_is_near_its_expiry() {
    let long_lived = stub_with(3600, decision_reply(true)).await;
    // Valid for less than the renewal margin, so every decision renews it.
    let short_lived = stub_with(2, decision_reply(true)).await;
    let subject = Principal::new("https://issuer.example", "user-1").unwrap();
    let resource = repository("acme", "app");

    for stub in [&long_lived, &short_lived] {
        let adapter = stub.adapter();
        adapter
            .decide(&subject, Action::Read, &resource)
            .await
            .unwrap();
        adapter
            .decide(&subject, Action::Read, &resource)
            .await
            .unwrap();
    }

    assert_eq!(long_lived.requests_to("/token").len(), 1);
    assert_eq!(short_lived.requests_to("/token").len(), 2);
    let second = short_lived
        .requests_to("/access/v1/evaluation")
        .pop()
        .unwrap();
    assert!(second.contains("authorization: Bearer token-2"), "{second}");
}

#[tokio::test]
async fn obtains_another_token_and_asks_again_after_a_401() {
    let issued = AtomicUsize::new(0);
    let stub = Stub::start(move |request| {
        if request.starts_with("POST /token ") {
            let number = issued.fetch_add(1, Ordering::SeqCst) + 1;
            token_reply(&format!("token-{number}"), 3600)
        } else if request.contains("authorization: Bearer token-2") {
            decision_reply(true)
        } else {
            ("401 Unauthorized", "{}".to_owned())
        }
    })
    .await;
    let subject = Principal::new("https://issuer.example", "user-1").unwrap();

    let decided = stub
        .adapter()
        .decide(&subject, Action::Read, &repository("acme", "app"))
        .await;

    assert_eq!(decided, Ok(Decision::Allow));
    assert_eq!(stub.requests_to("/token").len(), 2);
    assert_eq!(stub.requests_to("/access/v1/evaluation").len(), 2);
}

#[tokio::test]
async fn fails_where_the_decision_point_keeps_refusing_the_bearer_token() {
    let stub = stub_with(3600, ("401 Unauthorized", "{}".to_owned())).await;
    let subject = Principal::new("https://issuer.example", "user-1").unwrap();

    let decided = stub
        .adapter()
        .decide(&subject, Action::Read, &repository("acme", "app"))
        .await;

    assert!(
        matches!(decided, Err(AuthorisationError::Backend(_))),
        "{decided:?}"
    );
    // One retry only.
    assert_eq!(stub.requests_to("/access/v1/evaluation").len(), 2);
}

#[tokio::test]
async fn fails_on_an_answer_that_is_not_a_decision() {
    let subject = Principal::new("https://issuer.example", "user-1").unwrap();
    let answers = [
        (
            "500 Internal Server Error",
            json!({"decision": true}).to_string(),
        ),
        ("403 Forbidden", json!({"decision": true}).to_string()),
        ("200 OK", json!({"context": {}}).to_string()),
        ("200 OK", json!({"decision": "true"}).to_string()),
        ("200 OK", "not json".to_owned()),
    ];

    for answer in answers {
        let stub = stub_with(3600, answer.clone()).await;
        let decided = stub
            .adapter()
            .decide(&subject, Action::Read, &repository("acme", "app"))
            .await;
        assert!(
            matches!(decided, Err(AuthorisationError::Backend(_))),
            "{answer:?}: {decided:?}"
        );
    }
}

#[tokio::test]
async fn fails_where_the_token_endpoint_issues_no_usable_token() {
    let subject = Principal::new("https://issuer.example", "user-1").unwrap();
    let answers = [
        (
            "401 Unauthorized",
            json!({"error": "invalid_client"}).to_string(),
        ),
        (
            "200 OK",
            json!({"access_token": "t", "token_type": "mac"}).to_string(),
        ),
        ("200 OK", "not json".to_owned()),
    ];

    for answer in answers {
        let stub = Stub::start(move |request| {
            if request.starts_with("POST /token ") {
                answer.clone()
            } else {
                decision_reply(true)
            }
        })
        .await;
        let decided = stub
            .adapter()
            .decide(&subject, Action::Read, &repository("acme", "app"))
            .await;
        assert!(
            matches!(decided, Err(AuthorisationError::Backend(_))),
            "{decided:?}"
        );
        assert!(stub.requests_to("/access/v1/evaluation").is_empty());
    }
}

#[tokio::test]
async fn names_no_secret_in_a_failure() {
    let stub = stub_with(3600, ("500 Internal Server Error", "{}".to_owned())).await;
    let subject = Principal::new("https://issuer.example", "user-1").unwrap();

    let Err(AuthorisationError::Backend(text)) = stub
        .adapter()
        .decide(&subject, Action::Read, &repository("acme", "app"))
        .await
    else {
        panic!("expected a backend failure");
    };

    assert!(
        !text.contains("resource-secret") && !text.contains("token-1"),
        "{text}"
    );
}

/// The `host` header of a request, as `<host>:<port>`.
#[cfg(test)]
fn host_of(request: &str) -> String {
    request
        .lines()
        .find_map(|line| {
            line.to_lowercase()
                .strip_prefix("host: ")
                .map(str::to_owned)
        })
        .unwrap()
}

/// Whether the stub allows a resource: where its path ends in `a`.
#[cfg(test)]
fn allowed_by_path_suffix(resource: &Value) -> bool {
    resource["properties"]["path"]
        .as_str()
        .is_some_and(|path| path.ends_with('a'))
}

/// One decision per evaluation in the request body, as `allowed_by_path_suffix` decides it.
#[cfg(test)]
fn answers_each_evaluation(body: &Value) -> Reply {
    let decisions: Vec<Value> = body["evaluations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|evaluation| json!({"decision": allowed_by_path_suffix(&evaluation["resource"])}))
        .collect();
    ("200 OK", json!({"evaluations": decisions}).to_string())
}

/// A stub decision point: it issues a token, serves metadata that advertises its `/evaluations`
/// endpoint or not, answers that endpoint through `evaluations`, and answers the evaluation
/// endpoint as `allowed_by_path_suffix` decides.
#[cfg(test)]
async fn decision_point(advertises_evaluations: bool, evaluations: fn(&Value) -> Reply) -> Stub {
    Stub::start(move |request| {
        if request.starts_with("POST /token ") {
            token_reply("token", 3600)
        } else if request.starts_with("GET /.well-known/authzen-configuration ") {
            let metadata = if advertises_evaluations {
                json!({"access_evaluations_endpoint":
                    format!("http://{}/evaluations", host_of(request))})
            } else {
                json!({})
            };
            ("200 OK", metadata.to_string())
        } else if request.starts_with("POST /evaluations ") {
            evaluations(&body_of(request))
        } else {
            decision_reply(allowed_by_path_suffix(&body_of(request)["resource"]))
        }
    })
    .await
}

/// Three paths of one repository, only the first of which the stub allows.
#[cfg(test)]
fn three_paths() -> Vec<Resource> {
    ["docs/a", "docs/b", "docs/c"]
        .iter()
        .map(|p| path("acme", "app", p))
        .collect()
}

#[cfg(test)]
fn user() -> Principal {
    Principal::new("https://issuer.example", "user-1").unwrap()
}

#[cfg(test)]
fn requests_count(stub: &Stub, start: &str) -> usize {
    stub.requests
        .lock()
        .unwrap()
        .iter()
        .filter(|request| request.starts_with(start))
        .count()
}

#[tokio::test]
async fn decides_many_resources_in_one_evaluations_request_where_the_metadata_advertises_it() {
    let stub = decision_point(true, answers_each_evaluation).await;
    let adapter = stub.adapter_with_evaluations();
    let resources = three_paths();

    let first = adapter
        .decide_all(&user(), Action::Write, &resources)
        .await
        .unwrap();
    let second = adapter
        .decide_all(&user(), Action::Write, &resources)
        .await
        .unwrap();

    let expected = vec![Decision::Allow, Decision::Deny, Decision::Deny];
    assert_eq!(first, expected);
    assert_eq!(second, expected);
    // One request each time, to the advertised endpoint and to no other, carrying the subject
    // and action once and one evaluation per resource in order.
    assert_eq!(stub.requests_to("/evaluations").len(), 2);
    assert!(stub.requests_to("/access/v1/evaluation").is_empty());
    let sent = body_of(stub.requests_to("/evaluations").first().unwrap());
    assert_eq!(
        sent["subject"],
        json!({
            "type": "principal",
            "id": "user-1",
            "properties": {"issuer": "https://issuer.example"},
        })
    );
    assert_eq!(sent["action"], json!({"name": "write"}));
    let sent_paths: Vec<&str> = sent["evaluations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|evaluation| {
            assert_eq!(evaluation["resource"]["id"], "acme/app");
            evaluation["resource"]["properties"]["path"]
                .as_str()
                .unwrap()
        })
        .collect();
    assert_eq!(sent_paths, ["docs/a", "docs/b", "docs/c"]);
    // The metadata was read once for the two calls.
    assert_eq!(
        requests_count(&stub, "GET /.well-known/authzen-configuration "),
        1
    );
}

#[tokio::test]
async fn asks_one_evaluation_per_resource_where_the_metadata_does_not_advertise_evaluations() {
    let stub = decision_point(false, answers_each_evaluation).await;

    let decisions = stub
        .adapter_with_evaluations()
        .decide_all(&user(), Action::Write, &three_paths())
        .await
        .unwrap();

    assert_eq!(
        decisions,
        vec![Decision::Allow, Decision::Deny, Decision::Deny]
    );
    assert_eq!(stub.requests_to("/access/v1/evaluation").len(), 3);
    assert!(stub.requests_to("/evaluations").is_empty());
}

#[tokio::test]
async fn asks_one_evaluation_per_resource_by_default_though_the_metadata_advertises_evaluations() {
    let stub = decision_point(true, answers_each_evaluation).await;

    let decisions = stub
        .adapter()
        .decide_all(&user(), Action::Write, &three_paths())
        .await
        .unwrap();

    assert_eq!(
        decisions,
        vec![Decision::Allow, Decision::Deny, Decision::Deny]
    );
    assert_eq!(stub.requests_to("/access/v1/evaluation").len(), 3);
    assert!(stub.requests_to("/evaluations").is_empty());
    // The metadata is not read where the batch is not enabled.
    assert_eq!(
        requests_count(&stub, "GET /.well-known/authzen-configuration "),
        0
    );

    // The control: enabled, the same decision point is asked once, at its evaluations endpoint.
    let enabled = decision_point(true, answers_each_evaluation).await;
    enabled
        .adapter_with_evaluations()
        .decide_all(&user(), Action::Write, &three_paths())
        .await
        .unwrap();
    assert_eq!(enabled.requests_to("/evaluations").len(), 1);
}

#[tokio::test]
async fn decides_a_resource_given_twice_twice_and_none_without_asking() {
    let stub = decision_point(true, answers_each_evaluation).await;
    let adapter = stub.adapter_with_evaluations();
    let a = path("acme", "app", "docs/a");

    let none = adapter.decide_all(&user(), Action::Write, &[]).await;
    assert_eq!(none, Ok(Vec::new()));
    assert!(stub.requests.lock().unwrap().is_empty());

    let twice = adapter
        .decide_all(&user(), Action::Write, &[a.clone(), a])
        .await;
    assert_eq!(twice, Ok(vec![Decision::Allow, Decision::Allow]));
    let sent = body_of(stub.requests_to("/evaluations").first().unwrap());
    assert_eq!(sent["evaluations"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn fails_on_an_evaluations_answer_with_another_number_of_decisions_than_resources() {
    fn decisions(count: usize) -> Reply {
        let evaluations = vec![json!({"decision": true}); count];
        ("200 OK", json!({"evaluations": evaluations}).to_string())
    }
    fn one_decision(_: &Value) -> Reply {
        decisions(1)
    }
    fn four_decisions(_: &Value) -> Reply {
        decisions(4)
    }

    for evaluations in [one_decision, four_decisions] {
        let stub = decision_point(true, evaluations).await;
        let result = stub
            .adapter_with_evaluations()
            .decide_all(&user(), Action::Write, &three_paths())
            .await;
        assert!(
            matches!(&result, Err(AuthorisationError::Backend(message)) if message.contains("3 resources")),
            "{result:?}"
        );
    }

    // The control: an answer with one decision per resource is accepted.
    let stub = decision_point(true, answers_each_evaluation).await;
    assert!(
        stub.adapter_with_evaluations()
            .decide_all(&user(), Action::Write, &three_paths())
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn fails_where_the_evaluations_call_fails() {
    fn server_error(_: &Value) -> Reply {
        ("500 Internal Server Error", "{}".to_owned())
    }
    let stub = decision_point(true, server_error).await;

    let result = stub
        .adapter_with_evaluations()
        .decide_all(&user(), Action::Write, &three_paths())
        .await;

    assert!(
        matches!(result, Err(AuthorisationError::Backend(_))),
        "{result:?}"
    );
}

#[tokio::test]
async fn reads_the_metadata_again_after_a_failed_read() {
    let metadata_reads = AtomicUsize::new(0);
    let stub = Stub::start(move |request| {
        if request.starts_with("POST /token ") {
            token_reply("token", 3600)
        } else if request.starts_with("GET /.well-known/authzen-configuration ") {
            if metadata_reads.fetch_add(1, Ordering::SeqCst) == 0 {
                ("500 Internal Server Error", "{}".to_owned())
            } else {
                let endpoint = format!("http://{}/evaluations", host_of(request));
                (
                    "200 OK",
                    json!({"access_evaluations_endpoint": endpoint}).to_string(),
                )
            }
        } else if request.starts_with("POST /evaluations ") {
            answers_each_evaluation(&body_of(request))
        } else {
            decision_reply(true)
        }
    })
    .await;
    let adapter = stub.adapter_with_evaluations();
    let resources = three_paths();

    let failed = adapter.decide_all(&user(), Action::Write, &resources).await;
    assert!(
        matches!(failed, Err(AuthorisationError::Backend(_))),
        "{failed:?}"
    );
    // Nothing was asked of the decision point while the metadata was unknown.
    assert!(stub.requests_to("/access/v1/evaluation").is_empty());

    let retried = adapter.decide_all(&user(), Action::Write, &resources).await;
    assert_eq!(
        retried,
        Ok(vec![Decision::Allow, Decision::Deny, Decision::Deny])
    );
    // The second read was kept and used: one evaluations request, no single evaluation.
    assert_eq!(stub.requests_to("/evaluations").len(), 1);
    assert!(stub.requests_to("/access/v1/evaluation").is_empty());
}

#[tokio::test]
async fn denies_the_path_the_policy_denies_among_many_against_the_test_decision_point() {
    let adapter = real_adapter();
    let docs = path("acme", "app", "docs/x");
    let src = path("acme", "app", "src/main.rs");

    let decisions = adapter
        .decide_all(&bob(), Action::Write, &[docs.clone(), src.clone()])
        .await
        .unwrap();

    // Cerbos 0.57.0 answers an evaluations request for resources of one id with the first
    // decision for all of them (ADR 0041), so the default must not send one.
    assert_eq!(decisions, [Decision::Allow, Decision::Deny]);
    let reversed = adapter
        .decide_all(&bob(), Action::Write, &[src, docs.clone()])
        .await
        .unwrap();
    assert_eq!(reversed, [Decision::Deny, Decision::Allow]);
    // The control: the allowed path alone is allowed.
    assert_eq!(
        decide(&adapter, &bob(), Action::Write, &docs).await,
        Decision::Allow
    );
}

/// A private key for signing assertions, and what verifies them.
#[cfg(test)]
struct SigningKey {
    pem: Vec<u8>,
    verifier: DecodingKey,
    algorithm: Algorithm,
}

/// `der` as a PEM `PRIVATE KEY` block.
#[cfg(test)]
fn pem(der: &[u8]) -> Vec<u8> {
    let body = STANDARD.encode(der);
    let lines: Vec<&str> = body
        .as_bytes()
        .chunks(64)
        .map(|line| std::str::from_utf8(line).unwrap())
        .collect();
    format!(
        "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n",
        lines.join("\n")
    )
    .into_bytes()
}

/// A freshly generated P-256 key, PKCS#8.
#[cfg(test)]
fn ec_key() -> SigningKey {
    let pkcs8 =
        EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &SystemRandom::new())
            .unwrap();
    let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref()).unwrap();
    SigningKey {
        pem: pem(pkcs8.as_ref()),
        verifier: DecodingKey::from_ec_der(pair.public_key().as_ref()),
        algorithm: Algorithm::ES256,
    }
}

/// A freshly generated 2048-bit RSA key, PKCS#8.
#[cfg(test)]
fn rsa_key() -> SigningKey {
    let pair = RsaKeyPair::generate(KeySize::Rsa2048).unwrap();
    let der = AsDer::<Pkcs8V1Der>::as_der(&pair).unwrap();
    SigningKey {
        pem: pem(der.as_ref()),
        verifier: DecodingKey::from_rsa_der(pair.public_key().as_ref()),
        algorithm: Algorithm::RS256,
    }
}

#[cfg(test)]
const ISSUER: &str = "https://garam.test";

#[cfg(test)]
const TEMPLATE: &str = "grn:{organisation}/iam/service/codehub";

/// What the stand-in is set up to do.
#[cfg(test)]
struct Options {
    profile: Profile,
    kid: Option<&'static str>,
    audience: Option<&'static str>,
    /// Organisations whose token request the stand-in refuses as `unauthorized_client`.
    unauthorized: &'static [&'static str],
    expires_in: u64,
    /// Whether the decision point refuses each organisation's first token with 401.
    revoke_first: bool,
    /// Whether the adapter may send evaluations requests, which the metadata advertises.
    evaluations: bool,
}

#[cfg(test)]
impl Default for Options {
    fn default() -> Self {
        Self {
            profile: Profile::Grn,
            kid: None,
            audience: None,
            unauthorized: &[],
            expires_in: 3600,
            revoke_first: false,
            evaluations: false,
        }
    }
}

/// An in-process token endpoint and decision point. It verifies each assertion's signature, claims
/// and header against ADR 0042, issues a token bound to the organisation of the client id, and
/// answers a decision only for a token of the organisation the resource is in. What it finds wrong
/// it keeps in `rejections`.
#[cfg(test)]
struct Idp {
    verifier: DecodingKey,
    algorithm: Algorithm,
    options: Options,
    seen_jti: Mutex<HashSet<String>>,
    rejections: Mutex<Vec<String>>,
    /// The tokens issued, by organisation.
    issued: Mutex<HashMap<String, usize>>,
    /// The header and claims of each verified assertion.
    assertions: Mutex<Vec<(Value, Value)>>,
}

#[cfg(test)]
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::new();
    let mut at = 0;
    while let Some(&byte) = bytes.get(at) {
        match byte {
            b'+' => decoded.push(b' '),
            b'%' => {
                let hex = std::str::from_utf8(bytes.get(at + 1..at + 3).unwrap()).unwrap();
                decoded.push(u8::from_str_radix(hex, 16).unwrap());
                at += 2;
            }
            _ => decoded.push(byte),
        }
        at += 1;
    }
    String::from_utf8(decoded).unwrap()
}

/// The form parameters of a request body.
#[cfg(test)]
fn form_of(request: &str) -> HashMap<String, String> {
    request
        .split_once("\r\n\r\n")
        .unwrap()
        .1
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .map(|(name, value)| (percent_decode(name), percent_decode(value)))
        .collect()
}

/// The organisation a resource id is in, in either profile.
#[cfg(test)]
fn organisation_of_id(id: &str) -> &str {
    if let Some(organisation) = id.strip_prefix("grn:root/iam/org/") {
        organisation
    } else {
        id.strip_prefix("grn:")
            .unwrap_or(id)
            .split('/')
            .next()
            .unwrap()
    }
}

#[cfg(test)]
impl Idp {
    fn reject(&self, reason: &str) {
        self.rejections.lock().unwrap().push(reason.to_owned());
    }

    fn answer(&self, request: &str) -> Reply {
        if request.starts_with("POST /token ") {
            self.token(request)
        } else if request.starts_with("GET /.well-known/authzen-configuration ") {
            let metadata = if self.options.evaluations {
                json!({"access_evaluations_endpoint":
                    format!("http://{}/evaluations", host_of(request))})
            } else {
                json!({})
            };
            ("200 OK", metadata.to_string())
        } else {
            self.decision(request)
        }
    }

    /// Checks the token request and issues a token.
    fn token(&self, request: &str) -> Reply {
        let invalid = |reason: &str| {
            self.reject(reason);
            (
                "401 Unauthorized",
                json!({"error": "invalid_client"}).to_string(),
            )
        };
        let form = form_of(request);
        if request.contains("authorization: Basic") || form.contains_key("client_secret") {
            return invalid("a client secret was sent");
        }
        if form.get("grant_type").map(String::as_str) != Some("client_credentials") {
            return invalid("grant_type");
        }
        if form.get("client_assertion_type").map(String::as_str)
            != Some("urn:ietf:params:oauth:client-assertion-type:jwt-bearer")
        {
            return invalid("client_assertion_type");
        }
        if form.get("audience").map(String::as_str) != self.options.audience {
            return invalid("the audience parameter");
        }
        let (prefix, suffix) = TEMPLATE.split_once("{organisation}").unwrap();
        let Some(client_id) = form.get("client_id") else {
            return invalid("no client_id");
        };
        let Some(organisation) = client_id
            .strip_prefix(prefix)
            .and_then(|rest| rest.strip_suffix(suffix))
        else {
            return invalid("the client_id does not fit the template");
        };
        let Some(assertion) = form.get("client_assertion") else {
            return invalid("no client_assertion");
        };
        let Ok(header) = jsonwebtoken::decode_header(assertion) else {
            return invalid("the assertion has no readable header");
        };
        if header.typ.as_deref() != Some("client-authentication+jwt") {
            return invalid("typ");
        }
        if header.alg != self.algorithm {
            return invalid("alg");
        }
        if header.kid.as_deref() != self.options.kid {
            return invalid("kid");
        }
        let mut validation = Validation::new(self.algorithm);
        validation.set_required_spec_claims(&["exp", "aud", "iss", "sub"]);
        validation.set_audience(&[ISSUER]);
        validation.set_issuer(&[client_id.as_str()]);
        let Ok(decoded) = jsonwebtoken::decode::<Value>(assertion, &self.verifier, &validation)
        else {
            return invalid("the signature or a registered claim does not verify");
        };
        let claims = decoded.claims;
        if claims["sub"] != *client_id {
            return invalid("sub");
        }
        if !claims["aud"].is_string() {
            return invalid("aud is not one string");
        }
        let lifetime = claims["exp"]
            .as_u64()
            .zip(claims["iat"].as_u64())
            .map(|(exp, iat)| exp.saturating_sub(iat));
        if !lifetime.is_some_and(|seconds| (1..=60).contains(&seconds)) {
            return invalid("the lifetime is not 60 seconds or less");
        }
        let Some(jti) = claims["jti"].as_str() else {
            return invalid("no jti");
        };
        if !self.seen_jti.lock().unwrap().insert(jti.to_owned()) {
            return invalid("a jti was used twice");
        }
        self.assertions
            .lock()
            .unwrap()
            .push((serde_json::to_value(&header).unwrap(), claims));
        if self.options.unauthorized.contains(&organisation) {
            return (
                "400 Bad Request",
                json!({"error": "unauthorized_client"}).to_string(),
            );
        }
        let mut issued = self.issued.lock().unwrap();
        let number = issued.entry(organisation.to_owned()).or_insert(0);
        *number += 1;
        token_reply(
            &format!("token-{organisation}-{number}"),
            self.options.expires_in,
        )
    }

    /// Answers an evaluation or evaluations request for a token of the resource's organisation:
    /// allowed where the path ends in `a`.
    fn decision(&self, request: &str) -> Reply {
        let forbidden = |reason: &str| {
            self.reject(reason);
            ("403 Forbidden", "{}".to_owned())
        };
        let Some(bearer) = request
            .lines()
            .find_map(|line| line.strip_prefix("authorization: Bearer "))
        else {
            return forbidden("no bearer token");
        };
        let Some((token_organisation, number)) = bearer
            .strip_prefix("token-")
            .and_then(|rest| rest.rsplit_once('-'))
        else {
            return forbidden("an unknown token");
        };
        if self.options.revoke_first && number == "1" {
            return ("401 Unauthorized", "{}".to_owned());
        }
        let body = body_of(request);
        let batch = request.starts_with("POST /evaluations ");
        let entries: Vec<Value> = if batch {
            body["evaluations"].as_array().unwrap().clone()
        } else {
            vec![body.clone()]
        };
        let mut decisions = Vec::new();
        for entry in &entries {
            let id = entry["resource"]["id"].as_str().unwrap();
            if organisation_of_id(id) != token_organisation {
                return forbidden("a token of another organisation than the resource's");
            }
            let path = entry["context"]["path"]
                .as_str()
                .or_else(|| entry["resource"]["properties"]["path"].as_str());
            decisions.push(json!({"decision": path.is_some_and(|path| path.ends_with('a'))}));
        }
        if batch {
            ("200 OK", json!({"evaluations": decisions}).to_string())
        } else {
            ("200 OK", decisions.first().unwrap().to_string())
        }
    }

    fn rejections(&self) -> Vec<String> {
        self.rejections.lock().unwrap().clone()
    }

    fn issued(&self, organisation: &str) -> usize {
        self.issued
            .lock()
            .unwrap()
            .get(organisation)
            .copied()
            .unwrap_or(0)
    }
}

/// The stand-in, its server, and an adapter that authenticates to it by `private_key_jwt`.
#[cfg(test)]
struct Fixture {
    idp: Arc<Idp>,
    stub: Stub,
    adapter: AuthzenAuthorisationDecision,
}

/// A token client for `token_endpoint` that authenticates by `key`.
#[cfg(test)]
fn assertion_tokens(
    token_endpoint: &str,
    key: &SigningKey,
    kid: Option<&str>,
    audience: Option<&str>,
) -> Arc<TokenClient> {
    Arc::new(
        TokenClient::new(
            token_endpoint,
            audience.map(str::to_owned),
            Authentication::PrivateKeyJwt {
                client_id_template: TEMPLATE.to_owned(),
                key_pem: key.pem.clone(),
                key_id: kid.map(str::to_owned),
                issuer: ISSUER.to_owned(),
            },
        )
        .unwrap(),
    )
}

#[cfg(test)]
async fn fixture(key: &SigningKey, options: Options) -> Fixture {
    let (profile, evaluations) = (options.profile, options.evaluations);
    let (kid, audience) = (options.kid, options.audience);
    let idp = Arc::new(Idp {
        verifier: key.verifier.clone(),
        algorithm: key.algorithm,
        options,
        seen_jti: Mutex::new(HashSet::new()),
        rejections: Mutex::new(Vec::new()),
        issued: Mutex::new(HashMap::new()),
        assertions: Mutex::new(Vec::new()),
    });
    let served = Arc::clone(&idp);
    let stub = Stub::start(move |request| served.answer(request)).await;
    let adapter = AuthzenAuthorisationDecision::new(
        &stub.url,
        assertion_tokens(&format!("{}/token", stub.url), key, kid, audience),
        profile,
    )
    .unwrap()
    .with_access_evaluations(evaluations);
    Fixture { idp, stub, adapter }
}

#[tokio::test]
async fn authenticates_by_an_assertion_with_the_claims_and_header_of_adr_0042() {
    for key in [ec_key(), rsa_key()] {
        for (kid, audience) in [(Some("thumbprint"), Some("policy")), (None, None)] {
            let Fixture { idp, stub, adapter } = fixture(
                &key,
                Options {
                    kid,
                    audience,
                    ..Options::default()
                },
            )
            .await;

            let decided = adapter
                .decide(&user(), Action::Read, &repository("acme", "app"))
                .await;

            assert_eq!(decided, Ok(Decision::Deny), "{kid:?} {audience:?}");
            assert_eq!(idp.rejections(), Vec::<String>::new());
            let assertions = idp.assertions.lock().unwrap().clone();
            let [(header, claims)] = assertions.as_slice() else {
                panic!("expected one assertion: {assertions:?}");
            };
            let principal = "grn:acme/iam/service/codehub";
            assert_eq!(header["typ"], "client-authentication+jwt");
            assert_eq!(header["alg"], format!("{:?}", key.algorithm));
            assert_eq!(header.get("kid").and_then(Value::as_str), kid);
            assert_eq!(claims["iss"], principal);
            assert_eq!(claims["sub"], principal);
            assert_eq!(claims["aud"], ISSUER);
            assert!(claims["jti"].as_str().is_some_and(|jti| !jti.is_empty()));
            assert!(claims["exp"].as_u64() > claims["iat"].as_u64());
            let token_request = stub.requests_to("/token").pop().unwrap();
            assert!(
                token_request.contains("client_id=grn%3Aacme%2Fiam%2Fservice%2Fcodehub"),
                "{token_request}"
            );
            assert_eq!(
                token_request.contains("audience=policy"),
                audience.is_some()
            );
            let evaluation = stub.requests_to("/access/v1/evaluation").pop().unwrap();
            assert!(
                evaluation.contains("authorization: Bearer token-acme-1"),
                "{evaluation}"
            );
        }
    }
}

#[tokio::test]
async fn holds_one_token_per_organisation_and_renews_each_near_its_expiry() {
    let key = ec_key();
    let long_lived = fixture(&key, Options::default()).await;
    // Valid for less than the renewal margin, so every decision renews it.
    let short_lived = fixture(
        &key,
        Options {
            expires_in: 2,
            ..Options::default()
        },
    )
    .await;

    for organisation in ["acme", "beta", "acme", "beta"] {
        long_lived
            .adapter
            .decide(&user(), Action::Read, &repository(organisation, "app"))
            .await
            .unwrap();
    }
    for _ in 0..2 {
        short_lived
            .adapter
            .decide(&user(), Action::Read, &repository("acme", "app"))
            .await
            .unwrap();
    }

    assert_eq!(long_lived.idp.issued("acme"), 1);
    assert_eq!(long_lived.idp.issued("beta"), 1);
    assert_eq!(short_lived.idp.issued("acme"), 2);
    // Each decision presented its own organisation's token, and each assertion had its own jti.
    assert_eq!(long_lived.idp.rejections(), Vec::<String>::new());
    assert_eq!(short_lived.idp.rejections(), Vec::<String>::new());
    let bearers: Vec<String> = long_lived
        .stub
        .requests_to("/access/v1/evaluation")
        .iter()
        .map(|request| {
            request
                .lines()
                .find_map(|line| line.strip_prefix("authorization: Bearer "))
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(
        bearers,
        [
            "token-acme-1",
            "token-beta-1",
            "token-acme-1",
            "token-beta-1"
        ]
    );
    let second = short_lived
        .stub
        .requests_to("/access/v1/evaluation")
        .pop()
        .unwrap();
    assert!(
        second.contains("authorization: Bearer token-acme-2"),
        "{second}"
    );
}

#[tokio::test]
async fn obtains_another_token_by_a_fresh_assertion_and_asks_again_after_a_401() {
    let fixture = fixture(
        &ec_key(),
        Options {
            revoke_first: true,
            ..Options::default()
        },
    )
    .await;

    let decided = fixture
        .adapter
        .decide(&user(), Action::Read, &repository("acme", "app"))
        .await;

    assert_eq!(decided, Ok(Decision::Deny));
    assert_eq!(fixture.idp.issued("acme"), 2);
    assert_eq!(fixture.stub.requests_to("/access/v1/evaluation").len(), 2);
    // A reused jti would have been rejected.
    assert_eq!(fixture.idp.rejections(), Vec::<String>::new());
}

/// Resources of three organisations, allowed where the path ends in `a`.
#[cfg(test)]
fn three_organisations() -> Vec<Resource> {
    vec![
        path("acme", "app", "docs/a"),
        path("beta", "app", "docs/b"),
        path("acme", "app", "docs/c"),
        path("beta", "app", "docs/a"),
        path("gamma", "app", "docs/a"),
    ]
}

#[tokio::test]
async fn splits_a_call_by_organisation_and_keeps_the_order_of_the_answers() {
    let expected = vec![
        Decision::Allow,
        Decision::Deny,
        Decision::Deny,
        Decision::Allow,
        Decision::Allow,
    ];

    // Evaluations requests: one per organisation, in the order they first appear, each entry
    // with its own context.
    let batched = fixture(
        &ec_key(),
        Options {
            evaluations: true,
            ..Options::default()
        },
    )
    .await;
    let decisions = batched
        .adapter
        .decide_all(&user(), Action::Write, &three_organisations())
        .await
        .unwrap();
    assert_eq!(decisions, expected);
    assert_eq!(batched.idp.rejections(), Vec::<String>::new());
    assert!(batched.stub.requests_to("/access/v1/evaluation").is_empty());
    let sent: Vec<Value> = batched
        .stub
        .requests_to("/evaluations")
        .iter()
        .map(|request| body_of(request))
        .collect();
    let entry = |organisation: &str, path: &str| {
        json!({
            "resource": {"type": "grn", "id": format!("grn:{organisation}/codehub/repo/7")},
            "context": {"path": path},
        })
    };
    let request = |entries: Vec<Value>| {
        json!({
            "subject": {"type": "grn", "id": "user-1"},
            "action": {"name": "codehub:repo.write"},
            "evaluations": entries,
        })
    };
    assert_eq!(
        sent,
        vec![
            request(vec![entry("acme", "docs/a"), entry("acme", "docs/c")]),
            request(vec![entry("beta", "docs/b"), entry("beta", "docs/a")]),
            request(vec![entry("gamma", "docs/a")]),
        ]
    );

    // One evaluation per resource, in the order of the call, where evaluations are not enabled.
    let single = fixture(&ec_key(), Options::default()).await;
    let decisions = single
        .adapter
        .decide_all(&user(), Action::Write, &three_organisations())
        .await
        .unwrap();
    assert_eq!(decisions, expected);
    assert_eq!(single.idp.rejections(), Vec::<String>::new());
    assert_eq!(single.stub.requests_to("/access/v1/evaluation").len(), 5);
    assert_eq!(
        [
            single.idp.issued("acme"),
            single.idp.issued("beta"),
            single.idp.issued("gamma")
        ],
        [1, 1, 1]
    );
}

#[cfg(test)]
fn token_requests_for(stub: &Stub, organisation: &str) -> usize {
    let client_id = format!("client_id=grn%3A{organisation}%2F");
    stub.requests_to("/token")
        .iter()
        .filter(|request| request.contains(&client_id))
        .count()
}

#[tokio::test]
async fn denies_an_organisation_the_token_endpoint_refuses_as_unauthorized_client() {
    let fixture = fixture(
        &ec_key(),
        Options {
            unauthorized: &["ghost"],
            ..Options::default()
        },
    )
    .await;
    let adapter = &fixture.adapter;

    let alone = adapter
        .decide(&user(), Action::Read, &path("ghost", "app", "docs/a"))
        .await;
    // The control: the same path of an organisation the endpoint serves is allowed.
    let served = adapter
        .decide(&user(), Action::Read, &path("acme", "app", "docs/a"))
        .await;
    assert_eq!((alone, served), (Ok(Decision::Deny), Ok(Decision::Allow)));
    assert_eq!(fixture.stub.requests_to("/access/v1/evaluation").len(), 1);

    let mixed = adapter
        .decide_all(
            &user(),
            Action::Read,
            &[
                path("acme", "app", "docs/a"),
                path("ghost", "app", "docs/a"),
                path("acme", "app", "docs/a"),
            ],
        )
        .await;
    assert_eq!(
        mixed,
        Ok(vec![Decision::Allow, Decision::Deny, Decision::Allow])
    );
    // Nothing of the refused organisation reached the decision point, and nothing is cached: each
    // call asked for its token again, where acme's was held.
    assert_eq!(fixture.stub.requests_to("/access/v1/evaluation").len(), 3);
    assert_eq!(token_requests_for(&fixture.stub, "ghost"), 2);
    assert_eq!(token_requests_for(&fixture.stub, "acme"), 1);
    assert_eq!(fixture.idp.rejections(), Vec::<String>::new());
}

#[tokio::test]
async fn fails_as_a_server_error_on_any_other_refusal_of_a_token_request() {
    let answers = [
        (
            "400 Bad Request",
            json!({"error": "invalid_client"}).to_string(),
        ),
        (
            "401 Unauthorized",
            json!({"error": "invalid_client"}).to_string(),
        ),
        (
            "400 Bad Request",
            json!({"error": "invalid_request"}).to_string(),
        ),
        ("400 Bad Request", "not json".to_owned()),
        ("500 Internal Server Error", "{}".to_owned()),
    ];
    let key = ec_key();

    for answer in answers {
        let stub = Stub::start(move |request| {
            if request.starts_with("POST /token ") {
                answer.clone()
            } else {
                decision_reply(true)
            }
        })
        .await;
        let tokens = assertion_tokens(&format!("{}/token", stub.url), &key, None, None);
        let adapter = AuthzenAuthorisationDecision::new(&stub.url, tokens, Profile::Grn).unwrap();

        let decided = adapter
            .decide(&user(), Action::Read, &repository("acme", "app"))
            .await;

        assert!(
            matches!(decided, Err(AuthorisationError::Backend(_))),
            "{decided:?}"
        );
        assert!(stub.requests_to("/access/v1/evaluation").is_empty());
    }

    // `unauthorized_client` denies only where the client is one per organisation: with a client
    // secret it is a server error.
    let stub = Stub::start(|request| {
        if request.starts_with("POST /token ") {
            (
                "400 Bad Request",
                json!({"error": "unauthorized_client"}).to_string(),
            )
        } else {
            decision_reply(true)
        }
    })
    .await;
    let decided = stub
        .adapter()
        .decide(&user(), Action::Read, &repository("acme", "app"))
        .await;
    assert!(
        matches!(decided, Err(AuthorisationError::Backend(_))),
        "{decided:?}"
    );
}

#[tokio::test]
async fn sends_garams_json_for_each_action_in_the_grn_profile() {
    let fixture = fixture(&ec_key(), Options::default()).await;
    let adapter = &fixture.adapter;
    let docs = path("acme", "app", "docs/guide/a.md");
    let cases = [
        (Action::CreateOrganisation, org("acme")),
        (Action::CreateRepository, org("acme")),
        (Action::AdministerRunners, org("acme")),
        (Action::Read, repository("acme", "app")),
        (Action::Write, repository("acme", "app")),
        (Action::Read, docs.clone()),
        (Action::Write, docs),
    ];

    for (action, resource) in &cases {
        adapter.decide(&user(), *action, resource).await.unwrap();
    }

    let subject = json!({"type": "grn", "id": "user-1"});
    let organisation = json!({"type": "grn", "id": "grn:root/iam/org/acme"});
    let repository = json!({"type": "grn", "id": "grn:acme/codehub/repo/7"});
    let request = |name: &str, resource: &Value, context: Option<Value>| {
        let mut body = json!({"subject": subject, "action": {"name": name}, "resource": resource});
        if let Some(context) = context {
            body["context"] = context;
        }
        body
    };
    let sent: Vec<Value> = fixture
        .stub
        .requests_to("/access/v1/evaluation")
        .iter()
        .map(|request| body_of(request))
        .collect();
    assert_eq!(
        sent,
        vec![
            request("codehub:org.use", &organisation, None),
            request("codehub:repo.create", &organisation, None),
            request("codehub:runner.administer", &organisation, None),
            request("codehub:repo.read", &repository, None),
            request("codehub:repo.write", &repository, None),
            request(
                "codehub:repo.read",
                &repository,
                Some(json!({"path": "docs/guide/a.md"}))
            ),
            request(
                "codehub:repo.write",
                &repository,
                Some(json!({"path": "docs/guide/a.md"}))
            ),
        ]
    );
    assert_eq!(fixture.idp.rejections(), Vec::<String>::new());

    // Administering members has no action in this profile: a server error, and nothing is asked.
    let asked = fixture.stub.requests.lock().unwrap().len();
    let members = adapter
        .decide(&user(), Action::AdministerMembers, &org("acme"))
        .await;
    assert!(
        matches!(members, Err(AuthorisationError::Backend(_))),
        "{members:?}"
    );
    assert_eq!(fixture.stub.requests.lock().unwrap().len(), asked);
}

/// A path whose second component is not valid UTF-8.
#[cfg(test)]
fn non_utf8_path(organisation: &str) -> Resource {
    Resource::Path {
        organisation: organisation.parse().unwrap(),
        name: "app".parse().unwrap(),
        repository: RepositoryId::new(7),
        path: Path::new(vec![
            Component::new(b"docs".to_vec()).unwrap(),
            Component::new(vec![0xff, b'a']).unwrap(),
        ])
        .unwrap(),
    }
}

#[tokio::test]
async fn denies_a_path_that_is_not_utf8_without_asking_in_either_profile() {
    let grn = fixture(
        &ec_key(),
        Options {
            evaluations: true,
            ..Options::default()
        },
    )
    .await;
    let plain_stub = stub_with(3600, decision_reply(true)).await;
    let plain = plain_stub.adapter();

    for action in [Action::Read, Action::Write] {
        assert_eq!(
            grn.adapter
                .decide(&user(), action, &non_utf8_path("acme"))
                .await,
            Ok(Decision::Deny)
        );
        assert_eq!(
            plain.decide(&user(), action, &non_utf8_path("acme")).await,
            Ok(Decision::Deny)
        );
    }
    // Not even a token was asked for.
    assert!(grn.stub.requests.lock().unwrap().is_empty());
    assert!(plain_stub.requests.lock().unwrap().is_empty());

    // Among others, only the paths that are text are asked, and the answers keep the order of
    // the call; a path of non-ASCII text is sent as it is.
    let decisions = grn
        .adapter
        .decide_all(
            &user(),
            Action::Write,
            &[
                non_utf8_path("acme"),
                path("acme", "app", "docs/a"),
                path("acme", "app", "é/a"),
                non_utf8_path("acme"),
            ],
        )
        .await;
    assert_eq!(
        decisions,
        Ok(vec![
            Decision::Deny,
            Decision::Allow,
            Decision::Allow,
            Decision::Deny
        ])
    );
    let sent = body_of(grn.stub.requests_to("/evaluations").first().unwrap());
    let paths: Vec<&str> = sent["evaluations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["context"]["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, ["docs/a", "é/a"]);
}
