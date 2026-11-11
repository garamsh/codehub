//! Where a token comes from when `ch login` is not used: a file or an environment variable, and
//! the RFC 8693 exchange that may follow (ADR 0031).

use std::convert::Infallible;
use std::env::VarError;
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use reqwest::Client;
use serde::Deserialize;

use crate::api::encode;
use crate::failure::Failure;

/// The grant type of the exchange request (RFC 8693, section 2.1).
const GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:token-exchange";

/// The token type of the subject token and of the one requested (RFC 8693, section 3).
const ACCESS_TOKEN: &str = "urn:ietf:params:oauth:token-type:access_token";

/// A client secret, which `Debug` does not print.
#[derive(Clone)]
pub struct Secret(String);

impl FromStr for Secret {
    type Err = Infallible;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Ok(Self(text.to_owned()))
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

/// Where the token is read from, on every invocation.
#[derive(Debug, PartialEq, Eq)]
pub enum Source {
    /// The content of the file at this path.
    File(PathBuf),
    /// The value of the environment variable of this name.
    Variable(String),
}

impl Source {
    /// The token: the file's content or the variable's value, without surrounding whitespace.
    ///
    /// # Errors
    ///
    /// [`Failure::NotLoggedIn`], naming the source, where the file cannot be read, the variable is
    /// not set, or what is read is empty.
    pub fn read(&self) -> Result<String, Failure> {
        let (name, text) = match self {
            Self::File(path) => (
                format!("the token file {}", path.display()),
                std::fs::read_to_string(path).map_err(|error| error.to_string()),
            ),
            Self::Variable(variable) => (
                format!("the environment variable {variable}"),
                std::env::var(variable).map_err(|error| match error {
                    VarError::NotPresent => "it is not set".to_owned(),
                    VarError::NotUnicode(_) => "its value is not UTF-8".to_owned(),
                }),
            ),
        };
        match text {
            Ok(text) if text.trim().is_empty() => {
                Err(Failure::NotLoggedIn(format!("{name} is empty")))
            }
            Ok(text) => Ok(text.trim().to_owned()),
            Err(reason) => Err(Failure::NotLoggedIn(format!(
                "cannot read {name}: {reason}"
            ))),
        }
    }
}

/// The client `ch` authenticates as at the token endpoint.
#[derive(Debug)]
pub struct ClientCredentials {
    /// The client's identifier.
    pub id: String,
    /// The client's secret.
    pub secret: Secret,
}

/// Where to exchange the token, and for what.
#[derive(Debug)]
pub struct Exchange {
    /// The token endpoint.
    pub endpoint: String,
    /// The audience of the token requested.
    pub audience: String,
    /// The client to authenticate as, where the endpoint wants one.
    pub client: Option<ClientCredentials>,
}

#[derive(Deserialize)]
struct Answer {
    access_token: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

impl Exchange {
    /// Trades `subject_token` for an access token for the configured audience (RFC 8693). The
    /// answer is held by the caller and written nowhere.
    ///
    /// # Errors
    ///
    /// [`Failure::NotLoggedIn`], saying why, where the endpoint cannot be reached, answers other
    /// than success or answers without an `access_token`.
    pub async fn exchange(&self, http: &Client, subject_token: &str) -> Result<String, Failure> {
        let failed =
            |reason: String| Failure::NotLoggedIn(format!("token exchange failed: {reason}"));
        let body = format!(
            "grant_type={}&subject_token={}&subject_token_type={}&requested_token_type={}&audience={}",
            encode(GRANT_TYPE),
            encode(subject_token),
            encode(ACCESS_TOKEN),
            encode(ACCESS_TOKEN),
            encode(&self.audience),
        );
        let mut request = http
            .post(&self.endpoint)
            .header("content-type", "application/x-www-form-urlencoded")
            .body(body);
        if let Some(client) = &self.client {
            // RFC 6749, section 2.3.1: the identifier and the secret are form-encoded first.
            request = request.basic_auth(encode(&client.id), Some(encode(&client.secret.0)));
        }
        let response = request
            .send()
            .await
            .map_err(|error| failed(error.without_url().to_string()))?;
        let status = response.status();
        let answer: Result<Answer, _> = response.json().await;
        match (status.is_success(), answer) {
            (
                true,
                Ok(Answer {
                    access_token: Some(token),
                    ..
                }),
            ) if !token.is_empty() => Ok(token),
            (true, _) => Err(failed(
                "the token endpoint answered no access_token".to_owned(),
            )),
            (
                false,
                Ok(Answer {
                    error: Some(error),
                    error_description,
                    ..
                }),
            ) => Err(failed(match error_description {
                Some(description) => format!("{status}: {error}: {description}"),
                None => format!("{status}: {error}"),
            })),
            (false, _) => Err(failed(format!("the token endpoint answered {status}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_file_without_its_surrounding_whitespace() {
        let path = std::env::temp_dir().join(format!("ch-token-{}", std::process::id()));
        std::fs::write(&path, "\n  abc.def \n").unwrap();
        let token = Source::File(path.clone()).read();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(token.unwrap(), "abc.def");
    }

    #[test]
    fn fails_not_logged_in_naming_a_file_that_is_missing_or_empty() {
        let path = std::env::temp_dir().join(format!("ch-token-none-{}", std::process::id()));
        let missing = Source::File(path.clone()).read().unwrap_err();
        std::fs::write(&path, " \n").unwrap();
        let empty = Source::File(path.clone()).read().unwrap_err();
        std::fs::remove_file(&path).unwrap();
        for failure in [missing, empty] {
            assert!(matches!(failure, Failure::NotLoggedIn(_)));
            assert!(failure.to_string().contains(&path.display().to_string()));
        }
    }

    #[test]
    fn fails_not_logged_in_naming_a_variable_that_is_not_set() {
        let failure = Source::Variable("CH_TEST_NEVER_SET_VARIABLE".to_owned())
            .read()
            .unwrap_err();
        assert!(matches!(failure, Failure::NotLoggedIn(_)));
        assert!(failure.to_string().contains("CH_TEST_NEVER_SET_VARIABLE"));
    }

    #[test]
    fn does_not_print_a_secret() {
        assert!(!format!("{:?}", Secret("hunter2".to_owned())).contains("hunter2"));
    }
}
