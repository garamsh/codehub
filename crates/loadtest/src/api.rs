//! The HTTP API of codehub, and the presigned URLs it hands out, as one agent calls them. Every
//! call answers the HTTP status it ended with, or 0 where no answer came.

use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};

use reqwest::{Client, RequestBuilder, Response};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::stats::{Status, succeeded};

/// The client every agent shares, and the count of requests made to the API.
pub struct Api {
    http: Client,
    requests: AtomicU64,
}

impl Api {
    /// A client with a connection pool of its own.
    pub fn new() -> Self {
        Self {
            http: Client::new(),
            requests: AtomicU64::new(0),
        }
    }

    /// How many requests have been made to the API, the transfers to the object store apart.
    pub fn requests(&self) -> u64 {
        self.requests.load(Ordering::Relaxed)
    }

    /// The API of the repository `repository` of the organisation `organisation` at `base`,
    /// called with `token`.
    pub const fn session<'a>(
        &'a self,
        base: &'a str,
        token: &'a str,
        organisation: &'a str,
        repository: &'a str,
    ) -> Session<'a> {
        Session {
            api: self,
            base,
            token,
            organisation,
            repository,
        }
    }
}

#[derive(Deserialize)]
struct Header {
    name: String,
    value: String,
}

#[derive(Deserialize)]
struct Grant {
    url: String,
    method: String,
    headers: Vec<Header>,
}

#[derive(Deserialize)]
struct File {
    download: Grant,
}

#[derive(Deserialize)]
struct Problem {
    detail: String,
}

/// Whether the `detail` of a 409 says the compare-and-swap on `main` was lost: the draft is intact
/// and landing it again rebases it onto the new `main`. The other 409s (a conflict, or a base not
/// on `main`) need new work.
fn is_lost_swap(detail: &str) -> bool {
    detail.starts_with("main moved")
}

/// How a landing ended.
pub struct Landing {
    /// The HTTP status; 0 where no answer came.
    pub status: Status,
    /// A 409 that says `main` moved while landing.
    pub lost_swap: bool,
}

/// The address of `bytes`: `sha256:` and the digest in lowercase hex.
fn address(bytes: &[u8]) -> String {
    let mut address = String::from("sha256:");
    for byte in Sha256::digest(bytes) {
        // Writing to a String does not fail.
        let _ = write!(address, "{byte:02x}");
    }
    address
}

/// One repository at one server, called with one token.
pub struct Session<'a> {
    api: &'a Api,
    base: &'a str,
    token: &'a str,
    organisation: &'a str,
    repository: &'a str,
}

impl Session<'_> {
    fn repository_url(&self, tail: &str) -> String {
        format!(
            "{}/v1/orgs/{}/repos/{}/{tail}",
            self.base, self.organisation, self.repository
        )
    }

    /// Sends a request to the API; `None` where it got no answer.
    async fn send(&self, request: RequestBuilder) -> Option<Response> {
        self.api.requests.fetch_add(1, Ordering::Relaxed);
        request.bearer_auth(self.token).send().await.ok()
    }

    /// Moves bytes the way `grant` says, with no token; `body` is what an upload sends.
    async fn transfer(&self, grant: &Grant, body: Vec<u8>) -> Result<Vec<u8>, Status> {
        let put = grant.method == "PUT";
        let mut request = self.api.http.request(
            if put {
                reqwest::Method::PUT
            } else {
                reqwest::Method::GET
            },
            &grant.url,
        );
        for header in &grant.headers {
            request = request.header(&header.name, &header.value);
        }
        if put {
            request = request.body(body);
        }
        let answer = request.send().await.map_err(|_| Status::MIN)?;
        let status = answer.status().as_u16();
        if !succeeded(status) {
            return Err(status);
        }
        answer
            .bytes()
            .await
            .map(|bytes| bytes.to_vec())
            .map_err(|_| Status::MIN)
    }

    /// Stages `content` at `path` in the draft `draft`: the grant, the upload to the object store
    /// and the staging.
    pub async fn stage(&self, draft: &str, path: &str, content: &[u8]) -> Status {
        let address = address(content);
        let upload = json!({ "address": address, "size": content.len() });
        let request = self
            .api
            .http
            .post(self.repository_url("uploads"))
            .json(&upload);
        let Some(answer) = self.send(request).await else {
            return 0;
        };
        let status = answer.status().as_u16();
        if !succeeded(status) {
            return status;
        }
        let Ok(grant) = answer.json::<Grant>().await else {
            return 0;
        };
        if let Err(status) = self.transfer(&grant, content.to_vec()).await {
            return status;
        }
        let staged = json!({ "kind": "regular", "content": address });
        let request = self
            .api
            .http
            .put(self.repository_url(&format!("drafts/{draft}/files/{path}")))
            .json(&staged);
        self.send(request)
            .await
            .map_or(0, |answer| answer.status().as_u16())
    }

    /// Lands the draft `draft` with `message`.
    pub async fn land(&self, draft: &str, message: &str) -> Landing {
        let request = self
            .api
            .http
            .post(self.repository_url(&format!("drafts/{draft}/land")))
            .json(&json!({ "message": message }));
        let Some(answer) = self.send(request).await else {
            return Landing {
                status: 0,
                lost_swap: false,
            };
        };
        let status = answer.status().as_u16();
        let detail = if status == 409 {
            answer.json::<Problem>().await.map(|problem| problem.detail)
        } else {
            Ok(String::new())
        };
        Landing {
            status,
            lost_swap: detail.is_ok_and(|detail| is_lost_swap(&detail)),
        }
    }

    /// The bytes of the file at `path` on `main`: the lookup and the download from the object
    /// store; the status is the first one that is not a success.
    pub async fn read(&self, path: &str) -> (Status, Vec<u8>) {
        let request = self
            .api
            .http
            .get(self.repository_url(&format!("files/{path}")));
        let Some(answer) = self.send(request).await else {
            return (0, Vec::new());
        };
        let status = answer.status().as_u16();
        if !succeeded(status) {
            return (status, Vec::new());
        }
        let Ok(file) = answer.json::<File>().await else {
            return (0, Vec::new());
        };
        match self.transfer(&file.download, Vec::new()).await {
            Ok(bytes) => (status, bytes),
            Err(status) => (status, Vec::new()),
        }
    }

    /// The history of the file at `path`.
    pub async fn log(&self, path: &str) -> Status {
        let request = self
            .api
            .http
            .get(self.repository_url(&format!("history/{path}")));
        self.send(request)
            .await
            .map_or(0, |answer| answer.status().as_u16())
    }

    /// Sends `body` as the JSON of a request of `method` to `url` below the API root, for the
    /// administration a scenario sets up with; the answer is `None` where it got none.
    pub async fn administer(
        &self,
        method: reqwest::Method,
        url: &str,
        body: &serde_json::Value,
    ) -> Status {
        let request = self
            .api
            .http
            .request(method, format!("{}/v1/{url}", self.base))
            .json(body);
        self.send(request)
            .await
            .map_or(0, |answer| answer.status().as_u16())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_bytes_by_their_sha256_in_lowercase_hex() {
        assert_eq!(
            address(b"abc"),
            "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn tells_a_lost_swap_from_a_conflict_by_the_detail() {
        assert!(is_lost_swap("main moved while landing; land again"));
        assert!(is_lost_swap("main moved; try again"));
        assert!(!is_lost_swap(
            "the draft conflicts with changes landed on main since its base"
        ));
        assert!(!is_lost_swap("the draft's base is not on main"));
    }
}
