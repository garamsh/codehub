//! The running `codehub` binary, the database it runs against, and an HTTP client for its API.
//!
//! Every helper panics when something is not as expected, as a failed assertion does. Every wait
//! is bounded.

use std::ops::{Deref, DerefMut};
use std::path::Path;
use std::time::Duration;

use app::domain::address::Address;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

use sqlx::{Connection, PgConnection};
pub use test_support::{TestDatabase, env};

/// The audience the server accepts, which Dex gives every access token its client asks for.
const AUDIENCE: &str = "codehub-test";

/// How long a request may take.
const BOUND: Duration = Duration::from_secs(30);

/// A started `codehub` process, killed when dropped.
pub struct Server(test_support::Server);

impl Server {
    /// Starts the built `codehub` against `database`, configured by environment variables, and
    /// waits for the line that names its address.
    ///
    /// # Panics
    ///
    /// Panics if the process does not print that line within the bound.
    pub fn start(database: &TestDatabase) -> Self {
        Self(test_support::Server::start(
            Path::new(env!("CARGO_BIN_EXE_codehub")),
            database,
            AUDIENCE,
        ))
    }
}

impl Deref for Server {
    type Target = test_support::Server;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Server {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// An access token for `user` (`alice`, `bob`, `carol`) from Dex's password grant.
///
/// # Panics
///
/// Panics if Dex refuses the request.
pub async fn token(user: &str) -> String {
    let issuer = env("CODEHUB_TEST_OIDC_ISSUER");
    let password = env(&format!(
        "CODEHUB_TEST_OIDC_PASSWORD_{}",
        user.to_uppercase()
    ));
    let body = format!(
        "grant_type=password&username={user}%40example.test&password={password}&scope=openid"
    );
    let response = reqwest::Client::new()
        .post(format!("{issuer}/token"))
        .basic_auth(
            env("CODEHUB_TEST_OIDC_TEST_CLIENT_ID"),
            Some(env("CODEHUB_TEST_OIDC_TEST_CLIENT_SECRET")),
        )
        .header("content-type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .unwrap();
    assert!(
        response.status().is_success(),
        "Dex refused {user}'s token request"
    );
    let answer: Value = response.json().await.unwrap();
    answer["access_token"].as_str().unwrap().to_owned()
}

/// A client of the server's API, presenting one bearer token or none.
pub struct Client {
    http: reqwest::Client,
    base: String,
    token: Option<String>,
}

impl Client {
    /// A client of the server at `base` presenting `token`.
    ///
    /// # Panics
    ///
    /// Panics if the HTTP client cannot be built.
    pub fn new(base: &str, token: Option<String>) -> Self {
        Self {
            http: reqwest::Client::builder().timeout(BOUND).build().unwrap(),
            base: base.to_owned(),
            token,
        }
    }

    /// Sends `method` `path` with the JSON `body`, and returns the status and the JSON answer
    /// (`null` where the body is empty).
    ///
    /// # Panics
    ///
    /// Panics if the request fails or the answer is not JSON.
    pub async fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut request = self.http.request(method, format!("{}{path}", self.base));
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        let status = response.status();
        let bytes = response.bytes().await.unwrap();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap()
        };
        (status, body)
    }

    /// Creates the organisation `org` holding the repository `repo`.
    ///
    /// # Panics
    ///
    /// Panics if either is refused.
    pub async fn create_repository(&self, org: &str, repo: &str) {
        let (status, _) = self
            .send(Method::POST, "/v1/orgs", Some(json!({"name": org})))
            .await;
        assert_eq!(status, StatusCode::CREATED);
        let (status, _) = self
            .send(
                Method::POST,
                &format!("/v1/orgs/{org}/repos"),
                Some(json!({"name": repo})),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED);
    }

    /// Uploads `content` through an upload grant, stages it at `path` in the draft `draft` and
    /// lands the draft, in the repository `org`/`repo`.
    ///
    /// # Panics
    ///
    /// Panics if a step is refused.
    pub async fn land_file(&self, org: &str, repo: &str, draft: &str, path: &str, content: &str) {
        let address = Address::of(content.as_bytes()).to_string();
        let (status, grant) = self
            .send(
                Method::POST,
                &format!("/v1/orgs/{org}/repos/{repo}/uploads"),
                Some(json!({"address": address, "size": content.len()})),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        let mut upload = self
            .http
            .put(grant["url"].as_str().unwrap())
            .body(content.to_owned());
        for header in grant["headers"].as_array().unwrap() {
            upload = upload.header(
                header["name"].as_str().unwrap(),
                header["value"].as_str().unwrap(),
            );
        }
        assert!(upload.send().await.unwrap().status().is_success());
        let (status, _) = self
            .send(
                Method::PUT,
                &format!("/v1/orgs/{org}/repos/{repo}/drafts/{draft}/files/{path}"),
                Some(json!({"kind": "regular", "content": address})),
            )
            .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (status, _) = self
            .send(
                Method::POST,
                &format!("/v1/orgs/{org}/repos/{repo}/drafts/{draft}/land"),
                Some(json!({"message": "add a file"})),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
    }

    /// The content of the file at `path` in the repository `org`/`repo`, read through a download
    /// grant; `None` where the server answers 404.
    ///
    /// # Panics
    ///
    /// Panics if the server answers anything else, or the download fails.
    pub async fn read_file(&self, org: &str, repo: &str, path: &str) -> Option<String> {
        let (status, file) = self
            .send(
                Method::GET,
                &format!("/v1/orgs/{org}/repos/{repo}/files/{path}"),
                None,
            )
            .await;
        if status == StatusCode::NOT_FOUND {
            return None;
        }
        assert_eq!(status, StatusCode::OK);
        let download = self
            .http
            .get(file["download"]["url"].as_str().unwrap())
            .send()
            .await
            .unwrap();
        assert!(download.status().is_success());
        Some(download.text().await.unwrap())
    }
}

/// The settings that put `codehub` in introspection mode against Keycloak, as environment
/// variables.
pub fn introspection_settings() -> Vec<(&'static str, String)> {
    vec![
        ("CODEHUB_IDENTITY", "introspection".to_owned()),
        (
            "CODEHUB_INTROSPECTION_ENDPOINT",
            format!(
                "{}/protocol/openid-connect/token/introspect",
                env("CODEHUB_TEST_KEYCLOAK_ISSUER")
            ),
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

/// `settings` as the borrowed pairs `test_support::Server::start_with` takes.
pub fn pairs<'a>(settings: &'a [(&'static str, String)]) -> Vec<(&'a str, &'a str)> {
    settings
        .iter()
        .map(|(name, value)| (*name, value.as_str()))
        .collect()
}

/// The single number `query`, a `select count(*)`, returns in `database`.
///
/// # Panics
///
/// Panics if the database cannot be reached or the query fails.
pub async fn count(database: &TestDatabase, query: &'static str) -> i64 {
    let mut connection = PgConnection::connect(&database.url()).await.unwrap();
    sqlx::query_scalar(query)
        .fetch_one(&mut connection)
        .await
        .unwrap()
}
