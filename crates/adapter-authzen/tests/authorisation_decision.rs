//! The AuthZEN adapter against the test decision point (Cerbos, with Keycloak issuing its bearer
//! token), and against a stub server for the requests it sends and the answers the decision point
//! cannot be made to give.

use std::str::FromStr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use adapter_authzen::AuthzenAuthorisationDecision;
use app::domain::name::{OrganisationName, RepositoryName};
use app::domain::path::Path;
use app::domain::principal::Principal;
use app::domain::repository::RepositoryId;
use app::port::driven::authorisation_decision::{
    Action, AuthorisationDecision, AuthorisationError, Decision, Resource,
};
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
        &keycloak_token_endpoint(),
        &env("CODEHUB_TEST_KEYCLOAK_AUTHZEN_CLIENT_ID"),
        secret,
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
        &keycloak_token_endpoint(),
        &env("CODEHUB_TEST_KEYCLOAK_AUTHZEN_CLIENT_ID"),
        &env("CODEHUB_TEST_KEYCLOAK_AUTHZEN_CLIENT_SECRET"),
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
            &format!("{}/token", self.url),
            "resource-client",
            "resource-secret",
        )
        .unwrap()
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
        &format!("{}/token", stub.url),
        "cli:ent",
        "p:w%d s",
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

/// Three paths of one repository, the first and last of which the stub allows.
#[cfg(test)]
fn three_paths() -> Vec<Resource> {
    ["docs/a", "docs/b", "docs/ca"]
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
    let adapter = stub.adapter();
    let resources = three_paths();

    let first = adapter
        .decide_all(&user(), Action::Write, &resources)
        .await
        .unwrap();
    let second = adapter
        .decide_all(&user(), Action::Write, &resources)
        .await
        .unwrap();

    let expected = vec![Decision::Allow, Decision::Deny, Decision::Allow];
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
    assert_eq!(sent_paths, ["docs/a", "docs/b", "docs/ca"]);
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
        .adapter()
        .decide_all(&user(), Action::Write, &three_paths())
        .await
        .unwrap();

    assert_eq!(
        decisions,
        vec![Decision::Allow, Decision::Deny, Decision::Allow]
    );
    assert_eq!(stub.requests_to("/access/v1/evaluation").len(), 3);
    assert!(stub.requests_to("/evaluations").is_empty());
}

#[tokio::test]
async fn decides_a_resource_given_twice_twice_and_none_without_asking() {
    let stub = decision_point(true, answers_each_evaluation).await;
    let adapter = stub.adapter();
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
            .adapter()
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
        stub.adapter()
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
        .adapter()
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
                ("200 OK", json!({}).to_string())
            }
        } else {
            decision_reply(true)
        }
    })
    .await;
    let adapter = stub.adapter();
    let resources = three_paths();

    let failed = adapter.decide_all(&user(), Action::Write, &resources).await;
    assert!(
        matches!(failed, Err(AuthorisationError::Backend(_))),
        "{failed:?}"
    );
    // Nothing was asked of the decision point while the metadata was unknown.
    assert!(stub.requests_to("/access/v1/evaluation").is_empty());

    let retried = adapter.decide_all(&user(), Action::Write, &resources).await;
    assert_eq!(retried, Ok(vec![Decision::Allow; 3]));
}

#[tokio::test]
async fn decides_many_paths_against_the_test_decision_point_as_it_decides_each() {
    let adapter = real_adapter();
    let resources: Vec<Resource> = ["docs/x", "src/main.rs", "docs", "docsx/y", "docs/x"]
        .iter()
        .map(|p| path("acme", "app", p))
        .collect();

    let decisions = adapter
        .decide_all(&bob(), Action::Write, &resources)
        .await
        .unwrap();

    let mut each = Vec::new();
    for resource in &resources {
        each.push(decide(&adapter, &bob(), Action::Write, resource).await);
    }
    assert_eq!(decisions, each);
    // The control: the answers are not all one decision.
    assert_eq!(
        decisions,
        [
            Decision::Allow,
            Decision::Deny,
            Decision::Allow,
            Decision::Deny,
            Decision::Allow
        ]
    );
}
