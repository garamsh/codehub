//! The client of the HTTP API (ADR 0024) and of the presigned URLs it hands out (ADR 0023).

use std::fmt::Write;

use reqwest::{Client, Method, RequestBuilder, Response};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::failure::{Failure, Operation};
use crate::state::RepositoryRef;
use crate::wire::{
    Access, Checks, Created, CreatedRepository, Description, Diff, File, Files, Grant, History,
    Landed, Landing, Member, NewName, NewRunner, Plan, Principal, Ref, Runner, Runners, Snapshot,
    StagedFile, TransferGrant, TransferMethod, Upload, Workspace,
};

/// Percent-encodes `text` for use as a URL path segment or a form value.
#[must_use]
pub fn encode(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            // Writing to a String does not fail.
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

/// `path`'s segments, each encoded.
fn encode_path(path: &str) -> String {
    path.split('/').map(encode).collect::<Vec<_>>().join("/")
}

/// The API of one server, called with one token.
pub struct Api<'a> {
    http: &'a Client,
    server: &'a str,
    token: &'a str,
}

impl<'a> Api<'a> {
    /// The API at `server`, called with `token`.
    #[must_use]
    pub const fn new(http: &'a Client, server: &'a str, token: &'a str) -> Self {
        Self {
            http,
            server,
            token,
        }
    }

    fn request(&self, method: Method, path: &str) -> RequestBuilder {
        self.http
            .request(method, format!("{}/v1/{path}", self.server))
            .bearer_auth(self.token)
    }

    async fn send(request: RequestBuilder, operation: Operation) -> Result<Response, Failure> {
        let response = request.send().await?;
        if response.status().is_success() {
            return Ok(response);
        }
        let status = response.status();
        let problem = response
            .json()
            .await
            .unwrap_or_else(|_| crate::wire::Problem {
                kind: "about:blank".to_owned(),
                title: String::new(),
                status: status.as_u16(),
                detail: format!("the server answered {status}"),
                paths: Vec::new(),
            });
        Err(Failure::from_problem(operation, status.as_u16(), &problem))
    }

    fn build<B: Serialize + Sync>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
    ) -> RequestBuilder {
        let request = self.request(method, path);
        match body {
            Some(body) => request.json(body),
            None => request,
        }
    }

    async fn call<B: Serialize + Sync, R: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
        operation: Operation,
    ) -> Result<R, Failure> {
        let request = self.build(method, path, body);
        Ok(Self::send(request, operation).await?.json().await?)
    }

    async fn call_empty<B: Serialize + Sync>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
    ) -> Result<(), Failure> {
        let request = self.build(method, path, body);
        Self::send(request, Operation::Other).await.map(|_| ())
    }

    /// A DELETE that names its target in the query string; DELETE carries no body (ADR 0032).
    async fn delete_with_query(&self, path: &str, query: &[(&str, &str)]) -> Result<(), Failure> {
        let query: Vec<String> = query
            .iter()
            .map(|(name, value)| format!("{name}={}", encode(value)))
            .collect();
        let request = self.request(Method::DELETE, &format!("{path}?{}", query.join("&")));
        Self::send(request, Operation::Other).await.map(|_| ())
    }

    /// Creates an organisation.
    ///
    /// # Errors
    ///
    /// Fails where the request or the server's answer fails.
    pub async fn create_organisation(&self, name: &str) -> Result<Created, Failure> {
        let body = NewName {
            name: name.to_owned(),
        };
        self.call(Method::POST, "orgs", Some(&body), Operation::Other)
            .await
    }

    /// Creates a repository.
    ///
    /// # Errors
    ///
    /// Fails where the request or the server's answer fails.
    pub async fn create_repository(
        &self,
        repository: &RepositoryRef,
    ) -> Result<CreatedRepository, Failure> {
        let body = NewName {
            name: repository.repository.clone(),
        };
        let path = format!("orgs/{}/repos", encode(&repository.organisation));
        self.call(Method::POST, &path, Some(&body), Operation::Other)
            .await
    }

    /// Sets a member's role in an organisation.
    ///
    /// # Errors
    ///
    /// Fails where the request or the server's answer fails.
    pub async fn put_member(&self, organisation: &str, member: &Member) -> Result<(), Failure> {
        let path = format!("orgs/{}/members", encode(organisation));
        self.call_empty(Method::PUT, &path, Some(member)).await
    }

    /// Removes a member and its grants.
    ///
    /// # Errors
    ///
    /// Fails where the request or the server's answer fails.
    pub async fn delete_member(
        &self,
        organisation: &str,
        member: &Principal,
    ) -> Result<(), Failure> {
        let path = format!("orgs/{}/members", encode(organisation));
        self.delete_with_query(
            &path,
            &[("issuer", &member.issuer), ("subject", &member.subject)],
        )
        .await
    }

    /// Adds (`add`) or removes a grant on a repository.
    ///
    /// # Errors
    ///
    /// Fails where the request or the server's answer fails.
    pub async fn change_grant(
        &self,
        repository: &RepositoryRef,
        grant: &Grant,
        add: bool,
    ) -> Result<(), Failure> {
        let path = format!("{}/grants", repo_path(repository));
        if add {
            self.call_empty(Method::PUT, &path, Some(grant)).await
        } else {
            let access = match grant.access {
                Access::Read => "read",
                Access::Write => "write",
            };
            let query = [
                ("issuer", grant.issuer.as_str()),
                ("subject", &grant.subject),
                ("prefix", &grant.prefix),
                ("access", access),
            ];
            self.delete_with_query(&path, &query).await
        }
    }

    /// Asks for a grant to upload an object.
    ///
    /// # Errors
    ///
    /// Fails where the request or the server's answer fails.
    pub async fn request_upload(
        &self,
        repository: &RepositoryRef,
        upload: &Upload,
    ) -> Result<TransferGrant, Failure> {
        let path = format!("{}/uploads", repo_path(repository));
        self.call(Method::POST, &path, Some(upload), Operation::Other)
            .await
    }

    /// Stages `file` at `path` in `draft`.
    ///
    /// # Errors
    ///
    /// Fails where the request or the server's answer fails.
    pub async fn stage(
        &self,
        repository: &RepositoryRef,
        draft: &str,
        path: &str,
        file: &StagedFile,
    ) -> Result<(), Failure> {
        let path = draft_file(repository, draft, path);
        self.call_empty(Method::PUT, &path, Some(file)).await
    }

    /// Stages the deletion of `path` in `draft`.
    ///
    /// # Errors
    ///
    /// Fails where the request or the server's answer fails.
    pub async fn stage_deletion(
        &self,
        repository: &RepositoryRef,
        draft: &str,
        path: &str,
    ) -> Result<(), Failure> {
        let path = draft_file(repository, draft, path);
        self.call_empty(Method::DELETE, &path, None::<&()>).await
    }

    /// Lands `draft`.
    ///
    /// # Errors
    ///
    /// Fails where the request or the server's answer fails, and with
    /// [`Failure::LandingRejected`] where the landing is rejected.
    pub async fn land(
        &self,
        repository: &RepositoryRef,
        draft: &str,
        message: &str,
    ) -> Result<Landed, Failure> {
        let path = format!("{}/drafts/{}/land", repo_path(repository), encode(draft));
        let body = Landing {
            message: message.to_owned(),
        };
        self.call(Method::POST, &path, Some(&body), Operation::Land)
            .await
    }

    /// The file at `path`.
    ///
    /// # Errors
    ///
    /// Fails where the request or the server's answer fails.
    pub async fn file(&self, repository: &RepositoryRef, path: &str) -> Result<File, Failure> {
        let path = format!("{}/files/{}", repo_path(repository), encode_path(path));
        self.call(Method::GET, &path, None::<&()>, Operation::Other)
            .await
    }

    /// Opens `draft` as a workspace, creating it on `main` where it does not exist.
    ///
    /// # Errors
    ///
    /// Fails where the request or the server's answer fails.
    pub async fn open_workspace(
        &self,
        repository: &RepositoryRef,
        draft: &str,
    ) -> Result<Workspace, Failure> {
        let path = draft_path(repository, draft);
        self.call(Method::PUT, &path, None::<&()>, Operation::Other)
            .await
    }

    /// The workspace `draft`.
    ///
    /// # Errors
    ///
    /// Fails where the request or the server's answer fails.
    pub async fn workspace(
        &self,
        repository: &RepositoryRef,
        draft: &str,
    ) -> Result<Workspace, Failure> {
        let path = draft_path(repository, draft);
        self.call(Method::GET, &path, None::<&()>, Operation::Other)
            .await
    }

    /// Sets the description of `draft`.
    ///
    /// # Errors
    ///
    /// Fails where the request or the server's answer fails.
    pub async fn describe(
        &self,
        repository: &RepositoryRef,
        draft: &str,
        description: &str,
    ) -> Result<(), Failure> {
        let path = format!("{}/description", draft_path(repository, draft));
        let body = Description {
            description: description.to_owned(),
        };
        self.call_empty(Method::PUT, &path, Some(&body)).await
    }

    /// Records `snapshot` in `draft`, all of it or none.
    ///
    /// # Errors
    ///
    /// Fails where the request or the server's answer fails.
    pub async fn snapshot(
        &self,
        repository: &RepositoryRef,
        draft: &str,
        snapshot: &Snapshot,
    ) -> Result<(), Failure> {
        let path = format!("{}/snapshot", draft_path(repository, draft));
        self.call_empty(Method::POST, &path, Some(snapshot)).await
    }

    /// Removes the change `draft` holds at `path`.
    ///
    /// # Errors
    ///
    /// Fails where the request or the server's answer fails.
    pub async fn restore(
        &self,
        repository: &RepositoryRef,
        draft: &str,
        path: &str,
    ) -> Result<(), Failure> {
        let path = format!(
            "{}/changes/{}",
            draft_path(repository, draft),
            encode_path(path)
        );
        self.call_empty(Method::DELETE, &path, None::<&()>).await
    }

    /// The differences of `draft` against its base, of `path` alone where one is given.
    ///
    /// # Errors
    ///
    /// Fails where the request or the server's answer fails.
    pub async fn diff(
        &self,
        repository: &RepositoryRef,
        draft: &str,
        path: Option<&str>,
    ) -> Result<Diff, Failure> {
        let mut path_and_query = format!("{}/diff", draft_path(repository, draft));
        if let Some(path) = path {
            path_and_query.push_str("?path=");
            path_and_query.push_str(&encode(path));
        }
        self.call(Method::GET, &path_and_query, None::<&()>, Operation::Other)
            .await
    }

    /// The files of `changeset` under `prefix`.
    ///
    /// # Errors
    ///
    /// Fails where the request or the server's answer fails.
    pub async fn files(
        &self,
        repository: &RepositoryRef,
        changeset: &str,
        prefix: &str,
    ) -> Result<Files, Failure> {
        let path = format!(
            "{}/changesets/{}/files?prefix={}",
            repo_path(repository),
            encode(changeset),
            encode(prefix)
        );
        self.call(Method::GET, &path, None::<&()>, Operation::Other)
            .await
    }

    /// Registers a runner in `organisation`.
    ///
    /// # Errors
    ///
    /// Fails where the request or the server's answer fails.
    pub async fn register_runner(
        &self,
        organisation: &str,
        runner: &NewRunner,
    ) -> Result<Runner, Failure> {
        let path = format!("orgs/{}/runners", encode(organisation));
        self.call(Method::POST, &path, Some(runner), Operation::Other)
            .await
    }

    /// The runners registered in `organisation`.
    ///
    /// # Errors
    ///
    /// Fails where the request or the server's answer fails.
    pub async fn runners(&self, organisation: &str) -> Result<Runners, Failure> {
        let path = format!("orgs/{}/runners", encode(organisation));
        self.call(Method::GET, &path, None::<&()>, Operation::Other)
            .await
    }

    /// Removes the registration `id` from `organisation`.
    ///
    /// # Errors
    ///
    /// Fails where the request or the server's answer fails.
    pub async fn remove_runner(&self, organisation: &str, id: u64) -> Result<(), Failure> {
        let path = format!("orgs/{}/runners/{id}", encode(organisation));
        self.call_empty(Method::DELETE, &path, None::<&()>).await
    }

    /// The checks of `changeset`.
    ///
    /// # Errors
    ///
    /// Fails where the request or the server's answer fails.
    pub async fn checks(
        &self,
        repository: &RepositoryRef,
        changeset: &str,
    ) -> Result<Checks, Failure> {
        let path = format!(
            "{}/changesets/{}/checks",
            repo_path(repository),
            encode(changeset)
        );
        self.call(Method::GET, &path, None::<&()>, Operation::Other)
            .await
    }

    /// The plan of the draft `draft` of `repository`.
    ///
    /// # Errors
    ///
    /// Fails where the request or the server's answer fails.
    pub async fn plan(&self, repository: &RepositoryRef, draft: &str) -> Result<Plan, Failure> {
        let path = format!(
            "{}/drafts/{}/checks/plan",
            repo_path(repository),
            encode(draft)
        );
        self.call(Method::GET, &path, None::<&()>, Operation::Other)
            .await
    }

    /// The changeset the ref `name`, `main` or `last-green`, points at.
    ///
    /// # Errors
    ///
    /// Fails where the request or the server's answer fails, and with [`Failure::Denied`] where
    /// the ref is not set.
    pub async fn reference(&self, repository: &RepositoryRef, name: &str) -> Result<Ref, Failure> {
        let path = format!("{}/refs/{}", repo_path(repository), encode(name));
        self.call(Method::GET, &path, None::<&()>, Operation::Other)
            .await
    }

    /// The versions of the file at `path`.
    ///
    /// # Errors
    ///
    /// Fails where the request or the server's answer fails.
    pub async fn history(
        &self,
        repository: &RepositoryRef,
        path: &str,
    ) -> Result<History, Failure> {
        let path = format!("{}/history/{}", repo_path(repository), encode_path(path));
        self.call(Method::GET, &path, None::<&()>, Operation::Other)
            .await
    }
}

fn repo_path(repository: &RepositoryRef) -> String {
    format!(
        "orgs/{}/repos/{}",
        encode(&repository.organisation),
        encode(&repository.repository)
    )
}

fn draft_path(repository: &RepositoryRef, draft: &str) -> String {
    format!("{}/drafts/{}", repo_path(repository), encode(draft))
}

fn draft_file(repository: &RepositoryRef, draft: &str, path: &str) -> String {
    format!(
        "{}/drafts/{}/files/{}",
        repo_path(repository),
        encode(draft),
        encode_path(path)
    )
}

/// Moves bytes the way `grant` says: its method, its URL and exactly its headers, with no token.
/// `body` is what an upload sends; the answer's bytes are what a download returns.
///
/// # Errors
///
/// Fails where the transfer fails or the store answers with a failure status.
pub async fn transfer(
    http: &Client,
    grant: &TransferGrant,
    body: Vec<u8>,
) -> Result<Vec<u8>, Failure> {
    let method = match grant.method {
        TransferMethod::Get => Method::GET,
        TransferMethod::Put => Method::PUT,
    };
    let mut request = http.request(method, &grant.url);
    for header in &grant.headers {
        request = request.header(&header.name, &header.value);
    }
    if matches!(grant.method, TransferMethod::Put) {
        request = request.body(body);
    }
    let response = request.send().await?;
    if !response.status().is_success() {
        return Err(Failure::Other(format!(
            "the object store answered {}",
            response.status()
        )));
    }
    Ok(response.bytes().await?.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leaves_unreserved_characters_and_encodes_the_rest() {
        assert_eq!(encode("a-b.c_d~E9"), "a-b.c_d~E9");
        assert_eq!(encode("a b/c?d#é"), "a%20b%2Fc%3Fd%23%C3%A9");
    }

    #[test]
    fn keeps_the_slashes_between_path_segments() {
        assert_eq!(encode_path("src/a b.txt"), "src/a%20b.txt");
    }
}
