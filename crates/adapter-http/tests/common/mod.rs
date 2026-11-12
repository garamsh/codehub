//! The router over the in-memory adapters, and a client that sends it requests in process.
//!
//! Every helper panics when a reply is not the one expected, as a failed assertion does.

use adapter_http::{Pause, RunnerSettings};
use adapter_memory::{
    MemoryAuthorisationDecision, MemoryChangesetLog, MemoryDerivedData, MemoryDraftStore,
    MemoryIdentityVerification, MemoryJobQueue, MemoryMembership, MemoryObjectStore,
    MemoryRepositoryDirectory, MemoryRunnerRegistry,
};
use std::time::Duration;

use app::domain::address::Address;
use app::domain::check::{CheckDefinition, Command, Timeout};
use app::domain::job::NewJob;
use app::domain::name::{OrganisationName, RepositoryName};
use app::domain::principal::Principal;
use app::domain::repository::RepositoryId;
use app::port::driven::job_queue::JobQueue;
use app::port::driven::object_store::ObjectStore;
use app::port::driven::repository_directory::RepositoryDirectory;
use app::port::driving::members::Members;
use app::port::driving::repositories::Repositories;
use app::port::driving::source_control::SourceControl;
use app::use_case::administration::Administration;
use app::use_case::authentication::AuthenticationService;
use app::use_case::checks::{ChecksAdapters, ChecksService};
use app::use_case::jobs::{JobsAdapters, JobsService};
use app::use_case::members::{MembersAdapters, MembersService};
use app::use_case::repositories::{RepositoriesAdapters, RepositoriesService};
use app::use_case::runners::{RunnersAdapters, RunnersService};
use app::use_case::source_control::{SourceControlAdapters, SourceControlService};
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use serde_json::{Value, json};
use tower::ServiceExt;

/// The largest upload the fixture's source control grants.
pub const MAX_UPLOAD_SIZE: u64 = 1024;

/// How long the fixture holds a claim that finds no job.
pub const HOLD: Duration = Duration::from_millis(300);

/// How long the fixture's jobs service holds a job between heartbeats, in seconds.
const LEASE: i64 = 60;

/// The in-memory adapters of one deployment.
pub struct Stores {
    /// The object store.
    pub objects: MemoryObjectStore,
    /// The changeset log.
    pub log: MemoryChangesetLog,
    /// The derived data.
    pub derived: MemoryDerivedData,
    /// The draft store.
    pub drafts: MemoryDraftStore,
    /// The repository directory.
    pub directory: MemoryRepositoryDirectory,
    /// Identity verification, where tokens are registered.
    pub identity: MemoryIdentityVerification,
    /// Membership.
    pub membership: MemoryMembership,
    /// The authorisation decision over `membership`.
    pub authorisation: MemoryAuthorisationDecision,
    /// The runner registry.
    pub registry: MemoryRunnerRegistry,
    /// The job queue.
    pub queue: MemoryJobQueue,
}

impl Stores {
    /// Adapters that live as long as the process: the services borrow them, and a router holds its
    /// ports for `'static`.
    fn leaked() -> &'static Self {
        let membership = MemoryMembership::default();
        Box::leak(Box::new(Self {
            objects: MemoryObjectStore::default(),
            log: MemoryChangesetLog::default(),
            derived: MemoryDerivedData::default(),
            drafts: MemoryDraftStore::default(),
            directory: MemoryRepositoryDirectory::default(),
            identity: MemoryIdentityVerification::default(),
            authorisation: membership.authorisation_decision(),
            membership,
            registry: MemoryRunnerRegistry::default(),
            queue: MemoryJobQueue::default(),
        }))
    }
}

/// What the router answered.
pub struct Reply {
    /// The status code.
    pub status: StatusCode,
    /// The `Content-Type` header, empty where absent.
    pub content_type: String,
    /// The JSON body, `null` where empty.
    pub body: Value,
}

/// A router over fresh in-memory adapters.
pub struct Api {
    /// The adapters behind the router.
    pub stores: &'static Stores,
    router: Router,
}

/// The router over `stores`, with the repositories port replaced by `repositories`.
pub fn router_with<R>(stores: &'static Stores, repositories: R) -> Router
where
    R: Repositories + Send + Sync + 'static,
{
    adapter_http::router(
        AuthenticationService::new(&stores.identity),
        repositories,
        members(stores),
        source_control(stores),
    )
}

fn repositories(stores: &'static Stores) -> impl Repositories + Send + Sync + 'static {
    RepositoriesService::new(
        RepositoriesAdapters {
            directory: &stores.directory,
            authorisation: &stores.authorisation,
            membership: &stores.membership,
            changes: &(),
        },
        Administration::BuiltIn,
    )
}

fn pause(duration: Duration) -> Pause {
    Box::pin(tokio::time::sleep(duration))
}

fn runner_router(stores: &'static Stores) -> Router {
    adapter_http::runner_router(
        AuthenticationService::new(&stores.identity),
        repositories(stores),
        RunnersService::new(RunnersAdapters {
            registry: &stores.registry,
            directory: &stores.directory,
            authorisation: &stores.authorisation,
        }),
        JobsService::new(
            JobsAdapters {
                registry: &stores.registry,
                queue: &stores.queue,
                objects: &stores.objects,
                derived: &stores.derived,
                log: &stores.log,
            },
            LEASE,
        ),
        ChecksService::new(ChecksAdapters {
            queue: &stores.queue,
            log: &stores.log,
            directory: &stores.directory,
            authorisation: &stores.authorisation,
        }),
        RunnerSettings {
            hold: HOLD,
            retry_interval: Duration::from_millis(50),
            heartbeat_interval: Duration::from_secs(10),
            pause,
        },
    )
}

fn members(stores: &'static Stores) -> impl Members + Send + Sync + 'static {
    MembersService::new(
        MembersAdapters {
            directory: &stores.directory,
            authorisation: &stores.authorisation,
            membership: &stores.membership,
        },
        Administration::BuiltIn,
    )
}

fn source_control(stores: &'static Stores) -> impl SourceControl + Send + Sync + 'static {
    SourceControlService::new(
        SourceControlAdapters {
            objects: &stores.objects,
            log: &stores.log,
            derived: &stores.derived,
            drafts: &stores.drafts,
            directory: &stores.directory,
            authorisation: &stores.authorisation,
        },
        MAX_UPLOAD_SIZE,
    )
}

impl Default for Api {
    fn default() -> Self {
        let stores = Stores::leaked();
        Self {
            stores,
            router: router_with(stores, repositories(stores)).merge(runner_router(stores)),
        }
    }
}

impl Api {
    /// A router over fresh in-memory adapters.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The bearer token the fixture registers for the principal `name`.
    ///
    /// # Panics
    ///
    /// Panics when `name` is not a valid principal subject.
    #[must_use]
    pub fn token(&self, name: &str) -> String {
        let token = format!("token-{name}");
        self.stores
            .identity
            .register(&token.parse().unwrap(), Self::principal(name));
        token
    }

    /// The principal the fixture names `name`.
    ///
    /// # Panics
    ///
    /// Panics when `name` is not a valid principal subject.
    #[must_use]
    pub fn principal(name: &str) -> Principal {
        Principal::new("https://id.test", name).unwrap()
    }

    /// Sends one request; `token` is its bearer token and `body` its JSON body.
    pub async fn send(
        &self,
        method: Method,
        uri: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> Reply {
        send(self.router.clone(), method, uri, token, body).await
    }

    /// Sends a `GET` with `token`.
    pub async fn get(&self, uri: &str, token: &str) -> Reply {
        self.send(Method::GET, uri, Some(token), None).await
    }

    /// Sends a `POST` with `token` and a JSON `body`.
    pub async fn post(&self, uri: &str, token: &str, body: Value) -> Reply {
        self.send(Method::POST, uri, Some(token), Some(body)).await
    }

    /// Sends a `PUT` with `token` and a JSON `body`.
    pub async fn put(&self, uri: &str, token: &str, body: Value) -> Reply {
        self.send(Method::PUT, uri, Some(token), Some(body)).await
    }

    /// Creates the organisation `org` for `token`'s principal, and its repository `app`.
    ///
    /// # Panics
    ///
    /// Panics when either is not created.
    pub async fn repository(&self, token: &str, org: &str) {
        let created = self.post("/v1/orgs", token, json!({"name": org})).await;
        assert_eq!(created.status, StatusCode::CREATED);
        let created = self
            .post(
                &format!("/v1/orgs/{org}/repos"),
                token,
                json!({"name": "app"}),
            )
            .await;
        assert_eq!(created.status, StatusCode::CREATED);
    }

    /// The identifier of `org`/`app`, which stands in for the object store's side of a grant.
    ///
    /// # Panics
    ///
    /// Panics when `org`/`app` does not exist.
    pub async fn repository_id(&self, org: &str) -> RepositoryId {
        let org: OrganisationName = org.parse().unwrap();
        let name: RepositoryName = "app".parse().unwrap();
        self.stores
            .directory
            .find_repository(&org, &name)
            .await
            .unwrap()
    }

    /// Stands in for the client of an upload grant: asks for the grant, checks it is for a `PUT`
    /// of `content`'s address, and puts the bytes where the grant leads.
    ///
    /// # Panics
    ///
    /// Panics when the grant is not as expected or the bytes are not stored.
    pub async fn upload(&self, token: &str, org: &str, content: &str) -> String {
        let address = Address::of(content.as_bytes()).to_string();
        let reply = self
            .post(
                &format!("/v1/orgs/{org}/repos/app/uploads"),
                token,
                json!({"address": address, "size": content.len()}),
            )
            .await;
        assert_eq!(reply.status, StatusCode::OK);
        assert_eq!(reply.body["method"], "PUT");
        assert!(reply.body["url"].as_str().unwrap().ends_with(&address));
        let id = self.repository_id(org).await;
        self.stores
            .objects
            .put(id, content.as_bytes().to_vec())
            .await
            .unwrap();
        address
    }

    /// Uploads `content` and stages it at `path` in `token`'s draft `draft`.
    ///
    /// # Panics
    ///
    /// Panics when the upload or the staging fails.
    pub async fn stage(&self, token: &str, org: &str, draft: &str, path: &str, content: &str) {
        let address = self.upload(token, org, content).await;
        let reply = self
            .put(
                &format!("/v1/orgs/{org}/repos/app/drafts/{draft}/files/{path}"),
                token,
                json!({"kind": "regular", "content": address}),
            )
            .await;
        assert_eq!(reply.status, StatusCode::NO_CONTENT);
    }

    /// Lands `token`'s draft `draft`.
    pub async fn land(&self, token: &str, org: &str, draft: &str) -> Reply {
        self.post(
            &format!("/v1/orgs/{org}/repos/app/drafts/{draft}/land"),
            token,
            json!({"message": "land"}),
        )
        .await
    }

    /// Queues the check `name`, blocking and with `labels`, against `changeset` of `org`/`app`.
    ///
    /// # Panics
    ///
    /// Panics when the queue refuses it.
    pub async fn queue_check(&self, org: &str, changeset: &str, name: &str, labels: &[&str]) {
        let job = NewJob {
            organisation: org.parse().unwrap(),
            repository: self.repository_id(org).await,
            changeset: changeset.parse().unwrap(),
            check: CheckDefinition {
                name: name.parse().unwrap(),
                command: Command::new("true").unwrap(),
                labels: labels.iter().map(|label| label.parse().unwrap()).collect(),
                blocking: true,
                timeout: Timeout::new(60).unwrap(),
            },
        };
        self.stores.queue.queue(vec![job]).await.unwrap();
    }

    /// Follows a download grant: the bytes the object store holds at the address the grant names.
    ///
    /// # Panics
    ///
    /// Panics when the grant is not a `GET` to an address the store holds.
    pub async fn download(&self, org: &str, grant: &Value) -> String {
        assert_eq!(grant["method"], "GET");
        let url = grant["url"].as_str().unwrap();
        let address: Address = url.rsplit('/').next().unwrap().parse().unwrap();
        let id = self.repository_id(org).await;
        String::from_utf8(self.stores.objects.get(id, address).await.unwrap()).unwrap()
    }
}

/// Sends one request to `router`.
///
/// # Panics
///
/// Panics when the router fails or answers with a body that is not JSON.
pub async fn send(
    router: Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> Reply {
    let mut request = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let body = body.map_or_else(Body::empty, |body| Body::from(body.to_string()));
    let response = router.oneshot(request.body(body).unwrap()).await.unwrap();
    let status = response.status();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .map(|value| value.to_str().unwrap().to_owned())
        .unwrap_or_default();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    Reply {
        status,
        content_type,
        body,
    }
}
