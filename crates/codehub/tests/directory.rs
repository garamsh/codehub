//! The shipped `codehub` binary delivering repository changes to garam from the directory outbox,
//! against an in-process stand-in for garam's token endpoint, decision point and
//! `ApplyEntityChanges`; identity is introspection against the test identity provider.

#[cfg(test)]
pub mod common;

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use aws_lc_rs::encoding::{AsDer, Pkcs8V1Der};
use aws_lc_rs::rsa::{KeyPair, KeySize};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use common::{Client, TestDatabase, count, env, introspection_settings, pairs};
use reqwest::{Method, StatusCode};
use serde_json::json;
use test_support::{Server, keycloak_token};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const BINARY: &str = env!("CARGO_BIN_EXE_codehub");

/// The route garam's entity changes are posted to.
#[cfg(test)]
const ENTITY_CHANGES: &str = "/garam/policy/v1/entity-changes";

/// The stand-in for garam: it issues a token to every client, allows every decision, and answers
/// entity changes as the test says.
#[cfg(test)]
struct Garam {
    url: String,
    down: Arc<AtomicBool>,
    changes: Arc<Mutex<Vec<String>>>,
}

#[cfg(test)]
impl Garam {
    async fn start(down: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let down = Arc::new(AtomicBool::new(down));
        let changes = Arc::new(Mutex::new(Vec::new()));
        let (is_down, kept) = (Arc::clone(&down), Arc::clone(&changes));
        tokio::spawn(async move {
            loop {
                let (socket, _) = listener.accept().await.unwrap();
                let (is_down, kept) = (Arc::clone(&is_down), Arc::clone(&kept));
                tokio::spawn(async move { answer(socket, &is_down, &kept).await });
            }
        });
        Self { url, down, changes }
    }

    /// The bodies of the entity-changes requests received so far.
    fn received(&self) -> Vec<String> {
        self.changes.lock().unwrap().clone()
    }
}

/// Reads one request from `socket` and answers it.
#[cfg(test)]
async fn answer(mut socket: TcpStream, down: &AtomicBool, kept: &Mutex<Vec<String>>) {
    let mut received = Vec::new();
    let mut buffer = vec![0; 4096];
    let request = loop {
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
                break text;
            }
        } else if read == 0 {
            break text;
        }
    };
    let body = request.split_once("\r\n\r\n").map_or("", |(_, body)| body);
    let (status, reply) = if request.starts_with("POST /token ") {
        (
            "200 OK",
            json!({"access_token": "token", "token_type": "Bearer", "expires_in": 3600}),
        )
    } else if request.starts_with("POST /access/v1/evaluation ") {
        ("200 OK", json!({"decision": true}))
    } else if request.starts_with(&format!("POST {ENTITY_CHANGES} ")) {
        kept.lock().unwrap().push(body.to_owned());
        if down.load(Ordering::SeqCst) {
            ("503 Service Unavailable", json!({}))
        } else {
            (
                "200 OK",
                json!({"written": 1, "dropped": 0, "refusals": []}),
            )
        }
    } else {
        ("404 Not Found", json!({}))
    };
    let reply = reply.to_string();
    let response = format!(
        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\
         connection: close\r\n\r\n{reply}",
        reply.len()
    );
    socket.write_all(response.as_bytes()).await.unwrap();
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

/// The settings that deliver to `garam` under the `grn` profile, waiting two seconds on a creation.
#[cfg(test)]
fn garam_settings(garam: &Garam, key_file: &str) -> Vec<(&'static str, String)> {
    let mut settings = introspection_settings();
    settings.extend([
        ("CODEHUB_AUTHORISATION", "authzen".to_owned()),
        ("CODEHUB_AUTHZEN_URL", garam.url.clone()),
        (
            "CODEHUB_AUTHZEN_TOKEN_ENDPOINT",
            format!("{}/token", garam.url),
        ),
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
        ("CODEHUB_DIRECTORY_CHANGES", "garam".to_owned()),
        ("CODEHUB_DIRECTORY_GARAM_URL", garam.url.clone()),
        ("CODEHUB_DIRECTORY_DELIVERY_WAIT", "2".to_owned()),
    ]);
    settings
}

/// Creates `acme/app` as alice and returns the status.
#[cfg(test)]
async fn create_repository(server: &Server) -> StatusCode {
    let client = Client::new(&server.base, Some(keycloak_token("TEST", "alice").await));
    let (status, _) = client
        .send(
            Method::POST,
            "/v1/orgs/acme/repos",
            Some(json!({"name": "app"})),
        )
        .await;
    status
}

/// The entries the database's outbox holds.
#[cfg(test)]
async fn entries(database: &TestDatabase) -> i64 {
    count(database, "select count(*) from directory_outbox").await
}

#[tokio::test]
async fn answers_201_for_a_creation_whose_change_garam_accepts_within_the_wait() {
    let database = TestDatabase::new().await;
    let garam = Garam::start(false).await;
    let key = write_rsa_key("directory-accepting.pem");
    let settings = garam_settings(&garam, &key);
    let server = Server::start_with(Path::new(BINARY), &database, &pairs(&settings));

    let status = create_repository(&server).await;

    assert_eq!(status, StatusCode::CREATED);
    // Delivered and deleted before the answer, so the creation waited for it.
    assert_eq!(entries(&database).await, 0);
    let received = garam.received();
    assert_eq!(received.len(), 1, "{received:?}");
    assert!(
        received
            .iter()
            .all(|body| body.contains("grn:acme/codehub/repo/")),
        "{received:?}"
    );
}

#[tokio::test]
async fn answers_201_while_garam_is_down_and_delivers_the_change_once_it_is_up() {
    let database = TestDatabase::new().await;
    let garam = Garam::start(true).await;
    let key = write_rsa_key("directory-down.pem");
    let settings = garam_settings(&garam, &key);
    let server = Server::start_with(Path::new(BINARY), &database, &pairs(&settings));

    let status = create_repository(&server).await;

    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(entries(&database).await, 1);
    assert!(!garam.received().is_empty());

    garam.down.store(false, Ordering::SeqCst);
    let deadline = Instant::now() + Duration::from_secs(30);
    while entries(&database).await != 0 {
        assert!(
            Instant::now() < deadline,
            "the change was not delivered after garam came up"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

#[tokio::test]
async fn does_not_start_delivering_to_garam_without_the_grn_profile() {
    let database = TestDatabase::new().await;
    let mut settings = introspection_settings();
    settings.extend([
        ("CODEHUB_DIRECTORY_CHANGES", "garam".to_owned()),
        (
            "CODEHUB_DIRECTORY_GARAM_URL",
            "http://garam.test".to_owned(),
        ),
    ]);

    let started = Server::try_start_with(Path::new(BINARY), &database, &pairs(&settings));

    let Err(line) = started else {
        panic!("codehub started delivering to garam without the grn profile");
    };
    assert_eq!(
        line,
        "codehub: CODEHUB_DIRECTORY_CHANGES=garam needs CODEHUB_AUTHZEN_PROFILE=grn"
    );
}

#[tokio::test]
async fn does_not_start_delivering_to_garam_under_the_plain_profile() {
    let database = TestDatabase::new().await;
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
        ("CODEHUB_DIRECTORY_CHANGES", "garam".to_owned()),
        (
            "CODEHUB_DIRECTORY_GARAM_URL",
            "http://garam.test".to_owned(),
        ),
    ]);

    let started = Server::try_start_with(Path::new(BINARY), &database, &pairs(&settings));

    let Err(line) = started else {
        panic!("codehub started delivering to garam under the plain profile");
    };
    assert_eq!(
        line,
        "codehub: CODEHUB_DIRECTORY_CHANGES=garam needs CODEHUB_AUTHZEN_PROFILE=grn"
    );
}
