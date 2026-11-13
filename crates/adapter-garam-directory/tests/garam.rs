//! The garam adapter against an in-process stand-in that speaks the contract of garam's ADR-0149
//! and its HTTP binding: a token endpoint that issues one token per organisation, and an
//! `ApplyEntityChanges` endpoint that accepts only the token of the organisation the entity is in.

use std::str::FromStr;
use std::sync::{Arc, Mutex};

use adapter_garam_directory::GaramDirectoryChanges;
use app::domain::name::{OrganisationName, RepositoryName};
use app::domain::repository::RepositoryId;
use app::port::driven::directory_changes::{
    ChangeKind, DirectoryChanges, OrganisationChange, RepositoryChange,
};
use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use oauth_client::{Authentication, TokenClient};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[cfg(test)]
const ENTITY_CHANGES: &str = "/garam/policy/v1/entity-changes";

/// What the stand-in answers an entity-changes request: a status line and a JSON body.
#[cfg(test)]
type Reply = (&'static str, String);

/// The stand-in server and the requests it has received, as text.
#[cfg(test)]
struct StandIn {
    url: String,
    requests: Arc<Mutex<Vec<String>>>,
}

#[cfg(test)]
impl StandIn {
    /// Serves a token endpoint that issues `token-<organisation>` to the client of that
    /// organisation, and the entity-changes endpoint through `entity_changes`, which is given the
    /// request's bearer token and body.
    async fn start(entity_changes: impl Fn(&str, &Value) -> Reply + Send + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let kept = Arc::clone(&requests);
        tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let request = read_request(&mut socket).await;
                let (status, body) = if request.starts_with("POST /token ") {
                    token_reply(&request)
                } else {
                    entity_changes(&bearer(&request), &body_of(&request))
                };
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

    /// An adapter delivering to this stand-in, authenticating as each organisation's client.
    fn adapter(&self) -> GaramDirectoryChanges {
        GaramDirectoryChanges::new(&self.url, tokens(&format!("{}/token", self.url))).unwrap()
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

/// The JSON body of a request, `null` where there is none.
#[cfg(test)]
fn body_of(request: &str) -> Value {
    serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap_or(Value::Null)
}

/// The bearer token of a request, empty where it sends none.
#[cfg(test)]
fn bearer(request: &str) -> String {
    request
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(": ")?;
            name.eq_ignore_ascii_case("authorization")
                .then(|| value.strip_prefix("Bearer ").unwrap_or_default().to_owned())
        })
        .unwrap_or_default()
}

/// The token of the organisation whose client asked: the assertion's client id is
/// `grn:<organisation>/iam/service/codehub`, form-encoded.
#[cfg(test)]
fn token_reply(request: &str) -> Reply {
    let form = request.split_once("\r\n\r\n").unwrap().1;
    let client = form
        .split('&')
        .find_map(|pair| pair.strip_prefix("client_id="))
        .unwrap()
        .replace("%3A", ":")
        .replace("%2F", "/");
    let organisation = client
        .strip_prefix("grn:")
        .and_then(|rest| rest.split('/').next())
        .unwrap();
    (
        "200 OK",
        json!({
            "access_token": format!("token-{organisation}"),
            "token_type": "Bearer",
            "expires_in": 3600,
        })
        .to_string(),
    )
}

/// A fresh P-256 key as a PEM `PRIVATE KEY` block.
#[cfg(test)]
fn key_pem() -> Vec<u8> {
    let pkcs8 =
        EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &SystemRandom::new())
            .unwrap();
    let body = STANDARD.encode(pkcs8.as_ref());
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

/// A token client that authenticates by an assertion, one client per organisation.
#[cfg(test)]
fn tokens(token_endpoint: &str) -> Arc<TokenClient> {
    Arc::new(
        TokenClient::new(
            token_endpoint,
            None,
            Authentication::PrivateKeyJwt {
                client_id_template: "grn:{organisation}/iam/service/codehub".to_owned(),
                key_pem: key_pem(),
                key_id: None,
                issuer: "https://garam.test".to_owned(),
            },
        )
        .unwrap(),
    )
}

#[cfg(test)]
fn change(
    organisation: &str,
    name: &str,
    id: u64,
    kind: ChangeKind,
    sequence: u64,
) -> RepositoryChange {
    RepositoryChange {
        organisation: OrganisationName::from_str(organisation).unwrap(),
        name: RepositoryName::from_str(name).unwrap(),
        repository: RepositoryId::new(id),
        kind,
        sequence,
    }
}

/// The reply of garam to a change it applied.
#[cfg(test)]
fn written() -> Reply {
    (
        "200 OK",
        json!({"written": 1, "dropped": 0, "refusals": []}).to_string(),
    )
}

/// A stand-in whose entity-changes endpoint accepts the token of the organisation the entity is
/// in, as garam does for a caller bound to an organisation, and answers 403 to any other.
#[cfg(test)]
async fn bound_to_the_entitys_organisation() -> StandIn {
    StandIn::start(|bearer, body| {
        let id = body
            .pointer("/changes/0/entity/id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let organisation = id
            .strip_prefix("grn:")
            .and_then(|rest| rest.split('/').next())
            .unwrap_or_default();
        if bearer == format!("token-{organisation}") {
            written()
        } else {
            ("403 Forbidden", "wrong caller".to_owned())
        }
    })
    .await
}

#[tokio::test]
async fn sends_garams_body_and_the_organisations_token() {
    let stand_in = bound_to_the_entitys_organisation().await;
    let adapter = stand_in.adapter();

    let created = adapter
        .repository_changed(&change("acme", "app", 7, ChangeKind::Created, 1))
        .await;
    let other = adapter
        .repository_changed(&change("beta", "lib", 8, ChangeKind::Created, 3))
        .await;
    let deleted = adapter
        .repository_changed(&change("acme", "app", 7, ChangeKind::Deleted, 2))
        .await;

    assert!(created && other && deleted);
    let requests = stand_in.requests_to(ENTITY_CHANGES);
    let [first, second, third] = requests.as_slice() else {
        panic!("expected three requests: {requests:?}");
    };
    assert_eq!(
        body_of(first),
        json!({"changes": [{
            "entity": {"type": "grn", "id": "grn:acme/codehub/repo/7"},
            "sequence": "1",
            "upsert": {
                "org": {"type": "grn", "id": "grn:root/iam/org/acme"},
                "parents": [{"type": "grn", "id": "grn:root/iam/org/acme"}],
            },
        }]})
    );
    assert_eq!(bearer(first), "token-acme");
    assert_eq!(
        body_of(second),
        json!({"changes": [{
            "entity": {"type": "grn", "id": "grn:beta/codehub/repo/8"},
            "sequence": "3",
            "upsert": {
                "org": {"type": "grn", "id": "grn:root/iam/org/beta"},
                "parents": [{"type": "grn", "id": "grn:root/iam/org/beta"}],
            },
        }]})
    );
    assert_eq!(bearer(second), "token-beta");
    assert_eq!(
        body_of(third),
        json!({"changes": [{
            "entity": {"type": "grn", "id": "grn:acme/codehub/repo/7"},
            "sequence": "2",
            "delete": {},
        }]})
    );
    assert_eq!(bearer(third), "token-acme");
}

#[tokio::test]
async fn does_not_accept_a_change_garam_refuses_for_the_caller() {
    // The control for the 403: the same stand-in accepted the organisation's own token above.
    let stand_in = StandIn::start(|_, _| ("403 Forbidden", "wrong caller".to_owned())).await;

    let accepted = stand_in
        .adapter()
        .repository_changed(&change("acme", "app", 7, ChangeKind::Created, 1))
        .await;

    assert!(!accepted);
}

#[tokio::test]
async fn counts_written_and_dropped_as_accepted_and_a_refusal_as_not() {
    for (answer, expected) in [
        (json!({"written": 1, "dropped": 0, "refusals": []}), true),
        (json!({"written": 0, "dropped": 1, "refusals": []}), true),
        (
            json!({"written": 0, "dropped": 0, "refusals": [{"index": 0, "error": "no manifest"}]}),
            false,
        ),
        (json!({"written": 0, "dropped": 0, "refusals": []}), false),
        (
            json!({"written": 1, "dropped": 0, "refusals": [{"index": 0, "error": "no manifest"}]}),
            false,
        ),
    ] {
        let reply = answer.to_string();
        let stand_in = StandIn::start(move |_, _| ("200 OK", reply.clone())).await;

        let accepted = stand_in
            .adapter()
            .repository_changed(&change("acme", "app", 7, ChangeKind::Created, 1))
            .await;

        assert_eq!(accepted, expected, "{answer}");
    }
}

#[tokio::test]
async fn does_not_accept_a_change_while_garam_is_down_or_unreachable() {
    let down = StandIn::start(|_, _| ("503 Service Unavailable", "store down".to_owned())).await;
    let unreachable = {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        GaramDirectoryChanges::new(&url, tokens(&format!("{url}/token"))).unwrap()
    };
    let repository = change("acme", "app", 7, ChangeKind::Created, 1);

    assert!(!down.adapter().repository_changed(&repository).await);
    assert!(!unreachable.repository_changed(&repository).await);
    // The control: the same change is accepted where garam answers.
    let up = bound_to_the_entitys_organisation().await;
    assert!(up.adapter().repository_changed(&repository).await);
}

#[tokio::test]
async fn obtains_another_token_where_garam_refuses_the_first() {
    let seen = Arc::new(Mutex::new(0_usize));
    let counted = Arc::clone(&seen);
    let stand_in = StandIn::start(move |_, _| {
        let mut seen = counted.lock().unwrap();
        *seen += 1;
        if *seen == 1 {
            ("401 Unauthorized", String::new())
        } else {
            written()
        }
    })
    .await;

    let accepted = stand_in
        .adapter()
        .repository_changed(&change("acme", "app", 7, ChangeKind::Created, 1))
        .await;

    assert!(accepted);
    assert_eq!(stand_in.requests_to(ENTITY_CHANGES).len(), 2);
    assert_eq!(stand_in.requests_to("/token").len(), 2);
}

#[tokio::test]
async fn sends_nothing_for_an_organisation_change() {
    let stand_in = bound_to_the_entitys_organisation().await;
    let adapter = stand_in.adapter();

    adapter
        .organisation_changed(&OrganisationChange::Created(
            OrganisationName::from_str("acme").unwrap(),
        ))
        .await;

    assert!(stand_in.requests.lock().unwrap().is_empty());
    // The control: a repository change does reach the stand-in.
    adapter
        .repository_changed(&change("acme", "app", 7, ChangeKind::Created, 1))
        .await;
    assert_eq!(stand_in.requests_to(ENTITY_CHANGES).len(), 1);
}
