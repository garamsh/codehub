//! Where the runner gets the bearer token it sends to the server.

use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;
use tokio::sync::Mutex;
use tokio::time::Instant;

/// How long before a token expires the runner stops using it.
const EXPIRY_MARGIN: Duration = Duration::from_secs(30);

/// How long a token request may take.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// A secret whose text never reaches a log.
#[derive(Clone)]
pub struct Secret(String);

impl Secret {
    /// Holds `value`.
    #[must_use]
    pub const fn new(value: String) -> Self {
        Self(value)
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(..)")
    }
}

/// Why no token could be obtained.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TokenError(pub String);

impl fmt::Display for TokenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for TokenError {}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: Option<u64>,
}

struct Cached {
    token: String,
    until: Instant,
}

/// The OAuth client-credentials grant, with the token kept until shortly before it expires.
pub struct ClientCredentials {
    http: reqwest::Client,
    endpoint: String,
    id: String,
    secret: Secret,
    cached: Mutex<Option<Cached>>,
}

impl ClientCredentials {
    /// A source that asks `endpoint` for tokens as client `id`.
    #[must_use]
    pub fn new(http: reqwest::Client, endpoint: String, id: String, secret: Secret) -> Self {
        Self {
            http,
            endpoint,
            id,
            secret,
            cached: Mutex::new(None),
        }
    }

    async fn token(&self) -> Result<String, TokenError> {
        let mut cached = self.cached.lock().await;
        if let Some(held) = cached.as_ref()
            && Instant::now() < held.until
        {
            return Ok(held.token.clone());
        }
        let fresh = self.request().await?;
        let token = fresh.access_token;
        *cached = fresh
            .expires_in
            .and_then(|seconds| Duration::from_secs(seconds).checked_sub(EXPIRY_MARGIN))
            .map(|lifetime| Cached {
                token: token.clone(),
                until: Instant::now() + lifetime,
            });
        Ok(token)
    }

    async fn request(&self) -> Result<TokenResponse, TokenError> {
        let response = self
            .http
            .post(&self.endpoint)
            .basic_auth(form_encode(&self.id), Some(form_encode(&self.secret.0)))
            .header("content-type", "application/x-www-form-urlencoded")
            .body("grant_type=client_credentials")
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|error| TokenError(format!("the token endpoint did not answer: {error}")))?;
        let status = response.status();
        if !status.is_success() {
            return Err(TokenError(format!("the token endpoint answered {status}")));
        }
        response
            .json()
            .await
            .map_err(|error| TokenError(format!("the token response is malformed: {error}")))
    }
}

/// Where a token comes from.
pub enum TokenSource {
    /// A file whose first line is the token, read again before each request.
    File(PathBuf),
    /// The client-credentials grant.
    Credentials(ClientCredentials),
}

impl TokenSource {
    /// A token to send now.
    ///
    /// # Errors
    ///
    /// A [`TokenError`] where the file cannot be read or the endpoint gives none.
    pub async fn token(&self) -> Result<String, TokenError> {
        match self {
            Self::File(path) => {
                let text = tokio::fs::read_to_string(path).await.map_err(|error| {
                    TokenError(format!(
                        "the token file {} is unreadable: {error}",
                        path.display()
                    ))
                })?;
                let token = text.lines().next().unwrap_or("").trim();
                if token.is_empty() {
                    return Err(TokenError(format!(
                        "the token file {} is empty",
                        path.display()
                    )));
                }
                Ok(token.to_owned())
            }
            Self::Credentials(source) => source.token().await,
        }
    }
}

/// `application/x-www-form-urlencoded` encoding of one value, as RFC 6749 appendix B asks of a
/// client id and secret before they are joined into HTTP Basic credentials.
fn form_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'*' | b'-' | b'.' | b'_' | b'0'..=b'9' | b'A'..=b'Z' | b'a'..=b'z' => {
                out.push(char::from(byte));
            }
            b' ' => out.push('+'),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::Router;
    use axum::extract::State;
    use axum::http::HeaderMap;
    use axum::routing::post;

    use super::*;

    #[test]
    fn form_encoding_escapes_what_basic_credentials_must() {
        assert_eq!(form_encode("a b:c/d~é"), "a+b%3Ac%2Fd%7E%C3%A9");
        assert_eq!(form_encode("Az09*-._"), "Az09*-._");
    }

    #[test]
    fn a_secret_does_not_print() {
        assert_eq!(format!("{:?}", Secret::new("hunter2".into())), "Secret(..)");
    }

    #[tokio::test]
    async fn the_file_is_read_again_each_time() {
        let dir = std::env::temp_dir().join(format!("runner-token-{}", std::process::id()));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let path = dir.join("token");
        let source = TokenSource::File(path.clone());
        assert!(source.token().await.is_err());
        tokio::fs::write(&path, "one\n").await.unwrap();
        assert_eq!(source.token().await.unwrap(), "one");
        tokio::fs::write(&path, "two\n").await.unwrap();
        assert_eq!(source.token().await.unwrap(), "two");
        tokio::fs::write(&path, "\n").await.unwrap();
        assert!(source.token().await.is_err());
        tokio::fs::remove_dir_all(&dir).await.unwrap();
    }

    struct Stub {
        calls: AtomicUsize,
        expires_in: Option<u64>,
        authorization: std::sync::Mutex<String>,
    }

    async fn serve(stub: Arc<Stub>) -> String {
        async fn handle(State(stub): State<Arc<Stub>>, headers: HeaderMap, body: String) -> String {
            let call = stub.calls.fetch_add(1, Ordering::SeqCst);
            assert_eq!(body, "grant_type=client_credentials");
            *stub.authorization.lock().unwrap() = headers
                .get("authorization")
                .unwrap()
                .to_str()
                .unwrap()
                .to_owned();
            let expires = stub.expires_in.map_or(String::new(), |seconds| {
                format!(r#","expires_in":{seconds}"#)
            });
            format!(r#"{{"access_token":"token-{call}","token_type":"Bearer"{expires}}}"#)
        }
        let app = Router::new().route("/token", post(handle)).with_state(stub);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await });
        format!("http://{address}/token")
    }

    fn source(endpoint: String) -> ClientCredentials {
        ClientCredentials::new(
            reqwest::Client::new(),
            endpoint,
            "runner one".into(),
            Secret::new("s3cr:t".into()),
        )
    }

    #[tokio::test(start_paused = true)]
    async fn a_token_is_kept_until_shortly_before_it_expires() {
        let stub = Arc::new(Stub {
            calls: AtomicUsize::new(0),
            expires_in: Some(100),
            authorization: std::sync::Mutex::new(String::new()),
        });
        let source = source(serve(stub.clone()).await);
        assert_eq!(source.token().await.unwrap(), "token-0");
        tokio::time::advance(Duration::from_secs(60)).await;
        assert_eq!(source.token().await.unwrap(), "token-0");
        tokio::time::advance(Duration::from_secs(15)).await;
        assert_eq!(source.token().await.unwrap(), "token-1");
        assert_eq!(stub.calls.load(Ordering::SeqCst), 2);
        // "runner+one:s3cr%3At", base64.
        assert_eq!(
            *stub.authorization.lock().unwrap(),
            "Basic cnVubmVyK29uZTpzM2NyJTNBdA=="
        );
    }

    #[tokio::test]
    async fn a_token_without_a_lifetime_is_asked_for_each_time() {
        let stub = Arc::new(Stub {
            calls: AtomicUsize::new(0),
            expires_in: None,
            authorization: std::sync::Mutex::new(String::new()),
        });
        let source = source(serve(stub.clone()).await);
        assert_eq!(source.token().await.unwrap(), "token-0");
        assert_eq!(source.token().await.unwrap(), "token-1");
    }
}
