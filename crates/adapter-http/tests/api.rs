//! The HTTP API over the in-memory adapters, through requests to the router.

#[cfg(test)]
pub mod common;

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use app::domain::address::Address;
use app::domain::name::{OrganisationName, RepositoryName};
use app::domain::repository::RepositoryId;
use app::port::driven::repository_directory::DirectoryError;
use app::port::driving::repositories::{Repositories, RepositoriesError};
use axum::http::{Method, StatusCode};
use common::{Api, HOLD, Reply, router_with, send};
use serde_json::{Value, json};

const PROBLEM: &str = "application/problem+json";

#[cfg(test)]
fn address(content: &str) -> String {
    Address::of(content.as_bytes()).to_string()
}

#[tokio::test]
async fn refuses_a_request_without_a_valid_token() {
    let api = Api::new();
    let ada = api.token("ada");
    let create = |token: Option<String>| {
        let api = &api;
        async move {
            api.send(
                Method::POST,
                "/v1/orgs",
                token.as_deref(),
                Some(json!({"name": "acme"})),
            )
            .await
        }
    };
    for token in [None, Some("unknown".to_owned())] {
        let reply = create(token).await;
        assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
        assert_eq!(reply.content_type, PROBLEM);
        assert_eq!(reply.body["status"], 401);
    }
    // An empty token and a scheme other than bearer are refused alike.
    for header in ["Bearer ", "Basic abc", "Bearer"] {
        let request = axum::http::Request::builder()
            .method(Method::POST)
            .uri("/v1/orgs")
            .header(axum::http::header::AUTHORIZATION, header)
            .body(axum::body::Body::from(json!({"name": "acme"}).to_string()))
            .unwrap();
        let response = tower::ServiceExt::oneshot(router_for(&api), request)
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{header}");
    }
    // The same request with a valid token is served, so the refusals were the token's.
    assert_eq!(create(Some(ada)).await.status, StatusCode::CREATED);
}

#[cfg(test)]
fn router_for(api: &Api) -> axum::Router {
    use app::use_case::administration::Administration;
    use app::use_case::repositories::{RepositoriesAdapters, RepositoriesService};
    let stores = api.stores;
    router_with(
        stores,
        RepositoriesService::new(
            RepositoriesAdapters {
                directory: &stores.directory,
                authorisation: &stores.authorisation,
                membership: &stores.membership,
                changes: &(),
            },
            Administration::BuiltIn,
        ),
    )
}

#[tokio::test]
async fn stages_lands_reads_and_lists_the_history_of_a_file() {
    let api = Api::new();
    let ada = api.token("ada");
    api.repository(&ada, "acme").await;

    api.stage(&ada, "acme", "first", "src/a.txt", "one\n").await;
    let landed = api.land(&ada, "acme", "first").await;
    assert_eq!(landed.status, StatusCode::OK);
    let first: Address = landed.body["changeset"].as_str().unwrap().parse().unwrap();

    api.stage(&ada, "acme", "second", "src/a.txt", "two\n")
        .await;
    let landed = api.land(&ada, "acme", "second").await;
    assert_eq!(landed.status, StatusCode::OK);
    let second = landed.body["changeset"].as_str().unwrap().to_owned();
    assert_ne!(first.to_string(), second);

    let file = api
        .get("/v1/orgs/acme/repos/app/files/src/a.txt", &ada)
        .await;
    assert_eq!(file.status, StatusCode::OK);
    assert_eq!(file.body["kind"], "regular");
    assert_eq!(api.download("acme", &file.body["download"]).await, "two\n");

    let history = api
        .get("/v1/orgs/acme/repos/app/history/src/a.txt", &ada)
        .await;
    assert_eq!(history.status, StatusCode::OK);
    let versions = history.body["versions"].as_array().unwrap();
    assert_eq!(versions.len(), 2);
    assert_eq!(versions[0]["changeset"], second);
    assert_eq!(versions[0]["content"], address("two\n"));
    assert_eq!(versions[1]["changeset"], first.to_string());
    assert_eq!(versions[1]["content"], address("one\n"));
    assert_eq!(versions[1]["path"], "src/a.txt");

    // As of the first changeset, the history is its one version.
    let earlier = api
        .get(
            &format!("/v1/orgs/acme/repos/app/history/src/a.txt?at={first}"),
            &ada,
        )
        .await;
    assert_eq!(earlier.body["versions"].as_array().unwrap().len(), 1);

    // A staged deletion lands, and the file is gone.
    let deleted = api
        .send(
            Method::DELETE,
            "/v1/orgs/acme/repos/app/drafts/gone/files/src/a.txt",
            Some(&ada),
            None,
        )
        .await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT);
    assert_eq!(api.land(&ada, "acme", "gone").await.status, StatusCode::OK);
    let file = api
        .get("/v1/orgs/acme/repos/app/files/src/a.txt", &ada)
        .await;
    assert_eq!(file.status, StatusCode::NOT_FOUND);
    assert_eq!(file.content_type, PROBLEM);
}

#[tokio::test]
async fn names_the_path_of_a_conflicting_landing() {
    let api = Api::new();
    let ada = api.token("ada");
    api.repository(&ada, "acme").await;
    api.stage(&ada, "acme", "base", "a.txt", "one\n").await;
    assert_eq!(api.land(&ada, "acme", "base").await.status, StatusCode::OK);

    // Both drafts start from the same `main`; one edits a.txt, the other a.txt and b.txt.
    api.stage(&ada, "acme", "x", "a.txt", "two\n").await;
    api.stage(&ada, "acme", "y", "a.txt", "three\n").await;
    api.stage(&ada, "acme", "y", "b.txt", "other\n").await;
    assert_eq!(api.land(&ada, "acme", "x").await.status, StatusCode::OK);

    let conflict = api.land(&ada, "acme", "y").await;
    assert_eq!(conflict.status, StatusCode::CONFLICT);
    assert_eq!(conflict.content_type, PROBLEM);
    // Only the conflicting path is named, not every path of the draft.
    assert_eq!(conflict.body["paths"], json!(["a.txt"]));

    // The control: a draft on a path `main` did not change lands.
    api.stage(&ada, "acme", "z", "c.txt", "fresh\n").await;
    assert_eq!(api.land(&ada, "acme", "z").await.status, StatusCode::OK);
}

#[cfg(test)]
async fn add_member(api: &Api, admin: &str, org: &str, name: &str, role: &str) -> StatusCode {
    api.put(
        &format!("/v1/orgs/{org}/members"),
        admin,
        json!({"issuer": "https://id.test", "subject": name, "role": role}),
    )
    .await
    .status
}

#[cfg(test)]
async fn delete(api: &Api, token: &str, uri: &str) -> Reply {
    api.send(Method::DELETE, uri, Some(token), None).await
}

#[cfg(test)]
async fn grant(api: &Api, method: Method, admin: &str, name: &str, prefix: &str) -> StatusCode {
    let path = "/v1/orgs/acme/repos/app/grants";
    if method == Method::DELETE {
        let query = format!("issuer=https://id.test&subject={name}&prefix={prefix}&access=write");
        return delete(api, admin, &format!("{path}?{query}")).await.status;
    }
    api.send(
        method,
        path,
        Some(admin),
        Some(json!({
            "issuer": "https://id.test",
            "subject": name,
            "prefix": prefix,
            "access": "write",
        })),
    )
    .await
    .status
}

#[tokio::test]
async fn names_the_paths_of_a_landing_the_caller_may_not_write() {
    let api = Api::new();
    let ada = api.token("ada");
    let bob = api.token("bob");
    api.repository(&ada, "acme").await;
    assert_eq!(
        add_member(&api, &ada, "acme", "bob", "member").await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        grant(&api, Method::PUT, &ada, "bob", "").await,
        StatusCode::NO_CONTENT
    );

    // bob stages under docs and src while he may write everywhere, then loses docs.
    api.stage(&bob, "acme", "work", "docs/x.txt", "x\n").await;
    api.stage(&bob, "acme", "work", "src/y.txt", "y\n").await;
    api.stage(&bob, "acme", "safe", "src/z.txt", "z\n").await;
    assert_eq!(
        grant(&api, Method::DELETE, &ada, "bob", "").await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        grant(&api, Method::PUT, &ada, "bob", "src").await,
        StatusCode::NO_CONTENT
    );

    let denied = api.land(&bob, "acme", "work").await;
    assert_eq!(denied.status, StatusCode::FORBIDDEN);
    assert_eq!(denied.content_type, PROBLEM);
    assert_eq!(denied.body["paths"], json!(["docs/x.txt"]));

    // The control: a draft inside what he may still write lands.
    assert_eq!(api.land(&bob, "acme", "safe").await.status, StatusCode::OK);
}

#[tokio::test]
async fn names_the_path_a_caller_may_not_stage() {
    let api = Api::new();
    let ada = api.token("ada");
    let bob = api.token("bob");
    api.repository(&ada, "acme").await;
    add_member(&api, &ada, "acme", "bob", "member").await;
    grant(&api, Method::PUT, &ada, "bob", "src").await;

    let content = api.upload(&bob, "acme", "x\n").await;
    let stage = |path: &'static str| {
        let (api, bob, content) = (&api, &bob, &content);
        async move {
            api.put(
                &format!("/v1/orgs/acme/repos/app/drafts/work/files/{path}"),
                bob,
                json!({"kind": "regular", "content": content}),
            )
            .await
        }
    };
    let denied = stage("docs/x.txt").await;
    assert_eq!(denied.status, StatusCode::FORBIDDEN);
    assert_eq!(denied.body["paths"], json!(["docs/x.txt"]));
    assert_eq!(stage("src/x.txt").await.status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn answers_404_alike_to_a_non_member_and_for_a_missing_repository() {
    let api = Api::new();
    let ada = api.token("ada");
    let carol = api.token("carol");
    api.repository(&ada, "acme").await;
    api.stage(&ada, "acme", "first", "a.txt", "one\n").await;
    api.land(&ada, "acme", "first").await;

    // The control: the admin reads what the non-member cannot.
    let read = api.get("/v1/orgs/acme/repos/app/files/a.txt", &ada).await;
    assert_eq!(read.status, StatusCode::OK);

    let hidden = api.get("/v1/orgs/acme/repos/app/files/a.txt", &carol).await;
    let missing = api
        .get("/v1/orgs/acme/repos/ghost/files/a.txt", &carol)
        .await;
    assert_eq!(hidden.status, StatusCode::NOT_FOUND);
    assert_eq!(hidden.content_type, PROBLEM);
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert_eq!(
        hidden.body["detail"]
            .as_str()
            .map(|d| d.replace("app", "ghost")),
        missing.body["detail"].as_str().map(str::to_owned),
        "a hidden repository reads as a missing one"
    );

    let uploads = api
        .post(
            "/v1/orgs/acme/repos/app/uploads",
            &carol,
            json!({"address": address("x"), "size": 1}),
        )
        .await;
    assert_eq!(uploads.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn rejects_a_malformed_name_path_address_or_body() {
    let api = Api::new();
    let ada = api.token("ada");
    api.repository(&ada, "acme").await;
    let content = api.upload(&ada, "acme", "x\n").await;
    let staged = json!({"kind": "regular", "content": content});

    let org = api
        .post("/v1/orgs", &ada, json!({"name": "Bad_Name"}))
        .await;
    let repo = api
        .post("/v1/orgs/acme/repos", &ada, json!({"name": "-bad"}))
        .await;
    let path = api
        .put(
            "/v1/orgs/acme/repos/app/drafts/d/files/a//b",
            &ada,
            staged.clone(),
        )
        .await;
    let draft = api
        .put(
            "/v1/orgs/acme/repos/app/drafts/Bad/files/a.txt",
            &ada,
            staged,
        )
        .await;
    let digest = api
        .put(
            "/v1/orgs/acme/repos/app/drafts/d/files/a.txt",
            &ada,
            json!({"kind": "regular", "content": "sha256:xyz"}),
        )
        .await;
    let body = api.post("/v1/orgs", &ada, json!({"nom": "acme2"})).await;
    let at = api
        .get("/v1/orgs/acme/repos/app/history/a.txt?at=nonsense", &ada)
        .await;
    let org_in_path = api.get("/v1/orgs/Acme/repos/app/files/a.txt", &ada).await;
    for (what, reply) in [
        ("organisation name", org),
        ("repository name", repo),
        ("path", path),
        ("draft name", draft),
        ("address", digest),
        ("body", body),
        ("history address", at),
        ("organisation in a path", org_in_path),
    ] {
        assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{what}");
        assert_eq!(reply.content_type, PROBLEM, "{what}");
    }

    // The control: well-formed values of the same requests are served.
    let ok = api.post("/v1/orgs", &ada, json!({"name": "acme2"})).await;
    assert_eq!(ok.status, StatusCode::CREATED);
}

#[tokio::test]
async fn adds_and_removes_members_named_by_issuer_and_subject() {
    let api = Api::new();
    let ada = api.token("ada");
    let bob = api.token("bob");
    api.repository(&ada, "acme").await;
    api.stage(&ada, "acme", "first", "a.txt", "one\n").await;
    api.land(&ada, "acme", "first").await;
    let read = || api.get("/v1/orgs/acme/repos/app/files/a.txt", &bob);

    assert_eq!(read().await.status, StatusCode::NOT_FOUND);
    // A member who is not an admin cannot add members.
    assert_eq!(
        add_member(&api, &bob, "acme", "bob", "admin").await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        add_member(&api, &ada, "acme", "bob", "member").await,
        StatusCode::NO_CONTENT
    );
    grant(&api, Method::PUT, &ada, "bob", "").await;
    assert_eq!(read().await.status, StatusCode::OK);

    let remove = |name: &'static str| {
        let (api, ada) = (&api, &ada);
        async move {
            let uri = format!("/v1/orgs/acme/members?issuer=https://id.test&subject={name}");
            delete(api, ada, &uri).await
        }
    };
    assert_eq!(remove("bob").await.status, StatusCode::NO_CONTENT);
    assert_eq!(read().await.status, StatusCode::NOT_FOUND);
    // Removing someone who is not a member is not found.
    assert_eq!(remove("bob").await.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn rejects_a_removal_with_a_missing_or_malformed_query_parameter() {
    let api = Api::new();
    let ada = api.token("ada");
    api.repository(&ada, "acme").await;
    add_member(&api, &ada, "acme", "bob", "member").await;
    let grants = "/v1/orgs/acme/repos/app/grants";
    for uri in [
        "/v1/orgs/acme/members".to_owned(),
        "/v1/orgs/acme/members?issuer=https://id.test".to_owned(),
        "/v1/orgs/acme/members?subject=bob".to_owned(),
        "/v1/orgs/acme/members?issuer=&subject=bob".to_owned(),
        grants.to_owned(),
        format!("{grants}?issuer=https://id.test&subject=bob&prefix="),
        format!("{grants}?issuer=https://id.test&subject=bob&access=write"),
        format!("{grants}?issuer=https://id.test&subject=bob&prefix=&access=owner"),
    ] {
        let reply = delete(&api, &ada, &uri).await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{uri}");
        assert_eq!(reply.content_type, PROBLEM, "{uri}");
    }

    // The control: the complete queries are served.
    let grant = format!("{grants}?issuer=https://id.test&subject=bob&prefix=&access=write");
    assert_eq!(
        delete(&api, &ada, &grant).await.status,
        StatusCode::NO_CONTENT
    );
    let member = "/v1/orgs/acme/members?issuer=https://id.test&subject=bob";
    assert_eq!(
        delete(&api, &ada, member).await.status,
        StatusCode::NO_CONTENT
    );
}

/// A router in the external administration mode, over the stores of `api`.
#[cfg(test)]
fn external_router(api: &Api) -> axum::Router {
    use app::use_case::administration::Administration;
    use app::use_case::authentication::AuthenticationService;
    use app::use_case::members::{MembersAdapters, MembersService};
    use app::use_case::repositories::{RepositoriesAdapters, RepositoriesService};
    use app::use_case::source_control::{SourceControlAdapters, SourceControlService};
    let stores = api.stores;
    adapter_http::router(
        AuthenticationService::new(&stores.identity),
        RepositoriesService::new(
            RepositoriesAdapters {
                directory: &stores.directory,
                authorisation: &stores.authorisation,
                membership: &stores.membership,
                changes: &(),
            },
            Administration::External,
        ),
        MembersService::new(
            MembersAdapters {
                directory: &stores.directory,
                authorisation: &stores.authorisation,
                membership: &stores.membership,
            },
            Administration::External,
        ),
        SourceControlService::new(
            SourceControlAdapters {
                objects: &stores.objects,
                log: &stores.log,
                derived: &stores.derived,
                drafts: &stores.drafts,
                directory: &stores.directory,
                authorisation: &stores.authorisation,
            },
            common::MAX_UPLOAD_SIZE,
        ),
    )
}

#[tokio::test]
async fn refuses_every_member_operation_in_external_mode_with_403_saying_so() {
    let api = Api::new();
    let ada = api.token("ada");
    api.repository(&ada, "acme").await;
    let grant =
        json!({"issuer": "https://id.test", "subject": "bob", "prefix": "", "access": "write"});
    let operations = [
        (
            Method::PUT,
            "/v1/orgs/acme/members".to_owned(),
            Some(json!({"issuer": "https://id.test", "subject": "bob", "role": "member"})),
        ),
        (
            Method::DELETE,
            "/v1/orgs/acme/members?issuer=https://id.test&subject=bob".to_owned(),
            None,
        ),
        (
            Method::PUT,
            "/v1/orgs/acme/repos/app/grants".to_owned(),
            Some(grant),
        ),
        (
            Method::DELETE,
            "/v1/orgs/acme/repos/app/grants?issuer=https://id.test&subject=bob&prefix=&access=write"
                .to_owned(),
            None,
        ),
    ];
    for (method, uri, body) in operations {
        let request = |token: Option<&str>| {
            let mut builder = axum::http::Request::builder()
                .method(method.clone())
                .uri(&uri)
                .header(axum::http::header::CONTENT_TYPE, "application/json");
            if let Some(token) = token {
                builder =
                    builder.header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"));
            }
            builder
                .body(axum::body::Body::from(
                    body.as_ref().map(ToString::to_string).unwrap_or_default(),
                ))
                .unwrap()
        };
        let response = tower::ServiceExt::oneshot(external_router(&api), request(Some(&ada)))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{method} {uri}");
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let problem: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            problem["detail"], "members are administered outside codehub",
            "{method} {uri}"
        );
        // The control: without a token the same request is 401, not 403.
        let response = tower::ServiceExt::oneshot(external_router(&api), request(None))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "{method} {uri}"
        );
    }
}

/// A repositories port whose directory fails with a description that must stay inside.
struct Failing;

impl Repositories for Failing {
    async fn create_organisation(
        &self,
        _caller: &app::domain::principal::Caller,
        _name: &OrganisationName,
    ) -> Result<(), RepositoriesError> {
        Err(failure())
    }

    async fn create_repository(
        &self,
        _caller: &app::domain::principal::Caller,
        _organisation: &OrganisationName,
        _repository: &RepositoryName,
    ) -> Result<RepositoryId, RepositoriesError> {
        Err(failure())
    }

    async fn find_repository(
        &self,
        _caller: &app::domain::principal::Caller,
        _organisation: &OrganisationName,
        _repository: &RepositoryName,
    ) -> Result<RepositoryId, RepositoriesError> {
        Err(failure())
    }
}

#[cfg(test)]
fn failure() -> RepositoriesError {
    RepositoriesError::Directory(DirectoryError::Backend(
        "postgres://secret@db.internal/prod".to_owned(),
    ))
}

#[tokio::test]
async fn answers_a_backend_failure_with_500_and_no_internals() {
    let api = Api::new();
    let ada = api.token("ada");
    let router = router_with(api.stores, Failing);

    let reply = send(
        router,
        Method::POST,
        "/v1/orgs",
        Some(&ada),
        Some(json!({"name": "acme"})),
    )
    .await;
    assert_eq!(reply.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(reply.content_type, PROBLEM);
    let text = reply.body.to_string();
    assert!(
        !text.contains("secret") && !text.contains("postgres"),
        "{text}"
    );
}

#[cfg(test)]
fn committed_document() -> Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/openapi.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[tokio::test]
async fn serves_the_committed_openapi_document() {
    let api = Api::new();
    let served = api.send(Method::GET, "/v1/openapi.json", None, None).await;
    assert_eq!(served.status, StatusCode::OK);
    assert_eq!(served.body, committed_document());
    assert_eq!(served.body["openapi"], "3.1.0");
}

/// The method and path template of each operation the document describes.
#[cfg(test)]
fn documented() -> BTreeSet<(String, String)> {
    let document = committed_document();
    let mut operations = BTreeSet::new();
    for (path, item) in document["paths"].as_object().unwrap() {
        for method in item.as_object().unwrap().keys() {
            operations.insert((method.clone(), path.clone()));
        }
    }
    operations
}

#[tokio::test]
async fn documents_the_403_of_creating_an_organisation() {
    let document = committed_document();
    let forbidden = &document["paths"]["/v1/orgs"]["post"]["responses"]["403"];
    assert_eq!(
        forbidden["content"]["application/problem+json"]["schema"]["$ref"],
        "#/components/schemas/Problem"
    );
    // The control: the member operations' 403 has the same schema, and the 409 sits beside it.
    let members = &document["paths"]["/v1/orgs/{org}/members"]["put"]["responses"]["403"];
    assert_eq!(
        forbidden["content"]["application/problem+json"],
        members["content"]["application/problem+json"]
    );
    assert!(document["paths"]["/v1/orgs"]["post"]["responses"]["409"].is_object());
}

#[tokio::test]
async fn documents_the_404_of_creating_a_repository() {
    let document = committed_document();
    let responses = &document["paths"]["/v1/orgs/{org}/repos"]["post"]["responses"];
    assert_eq!(
        responses["404"]["content"]["application/problem+json"]["schema"]["$ref"],
        "#/components/schemas/Problem"
    );
    // The control: the 403 and the 409 beside it stay documented.
    assert!(responses["403"].is_object());
    assert!(responses["409"].is_object());
}

#[tokio::test]
async fn documents_no_request_body_on_a_delete_operation() {
    let document = committed_document();
    let mut deletes = 0;
    for item in document["paths"].as_object().unwrap().values() {
        if let Some(operation) = item.get("delete") {
            deletes += 1;
            assert!(operation.get("requestBody").is_none(), "{operation}");
        }
    }
    // The control: the document has delete operations, and a put keeps its body.
    assert_eq!(deletes, 4);
    assert!(document["paths"]["/v1/orgs/{org}/members"]["put"]["requestBody"].is_object());
}

#[tokio::test]
async fn documents_every_route_and_routes_every_documented_operation() {
    let routed: BTreeSet<(String, String)> = adapter_http::ROUTES
        .iter()
        .chain(adapter_http::RUNNER_ROUTES)
        .chain(adapter_http::HEALTH_ROUTES)
        .flat_map(|(path, methods)| {
            methods
                .iter()
                .map(|method| ((*method).to_owned(), path.replace("{*path}", "{path}")))
        })
        .collect();
    let documented = documented();
    assert_eq!(routed, documented);

    // The router answers each documented operation: an unrouted one would be 404 or 405, not
    // the 401 of a handler that wants a token (the document needs none).
    let api = Api::new();
    for (method, template) in &documented {
        let mut uri = String::new();
        let mut rest = template.as_str();
        while let Some((before, after)) = rest.split_once('{') {
            uri.push_str(before);
            uri.push('x');
            rest = after.split_once('}').map_or("", |(_, tail)| tail);
        }
        uri.push_str(rest);
        let method = Method::from_bytes(method.to_uppercase().as_bytes()).unwrap();
        let probe = uri == "/healthz" || uri == "/readyz";
        let reply = if probe {
            send(health(None), method.clone(), &uri, None, None).await
        } else {
            api.send(method.clone(), &uri, None, None).await
        };
        let expected = if uri == "/v1/openapi.json" || probe {
            StatusCode::OK
        } else {
            StatusCode::UNAUTHORIZED
        };
        assert_eq!(reply.status, expected, "{method} {uri}");
    }

    // The control: a route the document does not have is not routed.
    let unknown = api.send(Method::GET, "/v1/nothing", None, None).await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);
    assert_eq!(unknown.content_type, PROBLEM);
}

/// Readiness that holds, or fails with `reason`.
#[cfg(test)]
struct Dependencies(Option<&'static str>);

#[cfg(test)]
impl adapter_http::Readiness for Dependencies {
    async fn check(&self) -> Result<(), adapter_http::NotReady> {
        self.0
            .map_or(Ok(()), |reason| Err(adapter_http::NotReady(reason)))
    }
}

#[cfg(test)]
fn health(failing: Option<&'static str>) -> axum::Router {
    adapter_http::health_router(Dependencies(failing))
}

#[tokio::test]
async fn answers_the_probes_without_a_token() {
    for path in ["/healthz", "/readyz"] {
        let reply = send(health(None), Method::GET, path, None, None).await;
        assert_eq!(reply.status, StatusCode::OK, "{path}");
    }
}

#[tokio::test]
async fn answers_readyz_with_503_and_the_reason_where_a_dependency_does_not_answer() {
    let reason = "PostgreSQL did not answer";
    let not_ready = send(health(Some(reason)), Method::GET, "/readyz", None, None).await;
    assert_eq!(not_ready.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(not_ready.content_type, PROBLEM);
    assert_eq!(not_ready.body["status"], 503);
    assert_eq!(not_ready.body["detail"], reason);
    // The controls: liveness depends on nothing, so it holds beside the failure, and the same
    // route answers 200 where the dependencies answer.
    let live = send(health(Some(reason)), Method::GET, "/healthz", None, None).await;
    assert_eq!(live.status, StatusCode::OK);
    let ready = send(health(None), Method::GET, "/readyz", None, None).await;
    assert_eq!(ready.status, StatusCode::OK);
}

#[cfg(test)]
const CHECKS: &str = "/v1/orgs/acme/repos/app/changesets";

/// Registers `name` as a runner of acme offering `labels`, as ada, and returns its token.
#[cfg(test)]
async fn register_runner(api: &Api, ada: &str, name: &str, labels: &[&str]) -> String {
    let reply = api
        .post(
            "/v1/orgs/acme/runners",
            ada,
            json!({"issuer": "https://id.test", "subject": name, "labels": labels}),
        )
        .await;
    assert_eq!(reply.status, StatusCode::CREATED, "{}", reply.body);
    api.token(name)
}

/// Lands a file as ada and returns the changeset, with one queued check `lint` wanting `linux`.
#[cfg(test)]
async fn landed_with_a_check(api: &Api, ada: &str) -> String {
    api.repository(ada, "acme").await;
    api.stage(ada, "acme", "d", "src/a.txt", "one\n").await;
    let landed = api.land(ada, "acme", "d").await;
    assert_eq!(landed.status, StatusCode::OK, "{}", landed.body);
    let changeset = landed.body["changeset"].as_str().unwrap().to_owned();
    api.queue_check("acme", &changeset, "lint", &["linux"])
        .await;
    changeset
}

#[cfg(test)]
async fn claim(api: &Api, runner: &str) -> Reply {
    api.post("/v1/runner/jobs/claim", runner, json!({})).await
}

#[tokio::test]
async fn registers_lists_and_removes_a_runner() {
    let api = Api::new();
    let ada = api.token("ada");
    let bob = api.token("bob");
    api.repository(&ada, "acme").await;

    let created = api
        .post(
            "/v1/orgs/acme/runners",
            &ada,
            json!({"issuer": "https://id.test", "subject": "r1", "labels": ["linux"], "repository": "app"}),
        )
        .await;
    assert_eq!(created.status, StatusCode::CREATED);
    let id = created.body["id"].as_u64().unwrap();
    let repository = api.repository_id("acme").await.value();
    assert_eq!(created.body["repository_id"], repository);
    assert_eq!(created.body["labels"], json!(["linux"]));

    let listed = api.get("/v1/orgs/acme/runners", &ada).await;
    assert_eq!(listed.status, StatusCode::OK);
    assert_eq!(listed.body["runners"].as_array().unwrap().len(), 1);
    assert_eq!(listed.body["runners"][0]["id"], id);
    assert_eq!(listed.body["runners"][0]["subject"], "r1");

    // Only an admin administers runners.
    let denied = api.get("/v1/orgs/acme/runners", &bob).await;
    assert_eq!(denied.status, StatusCode::FORBIDDEN);
    assert_eq!(denied.content_type, PROBLEM);
    let missing = api
        .post(
            "/v1/orgs/acme/runners",
            &ada,
            json!({"issuer": "https://id.test", "subject": "r2", "labels": [], "repository": "nope"}),
        )
        .await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);

    let uri = format!("/v1/orgs/acme/runners/{id}");
    let removed = api.send(Method::DELETE, &uri, Some(&ada), None).await;
    assert_eq!(removed.status, StatusCode::NO_CONTENT);
    let again = api.send(Method::DELETE, &uri, Some(&ada), None).await;
    assert_eq!(again.status, StatusCode::NOT_FOUND);
    let listed = api.get("/v1/orgs/acme/runners", &ada).await;
    assert!(listed.body["runners"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn claims_a_job_with_a_grant_for_each_file() {
    let api = Api::new();
    let ada = api.token("ada");
    let changeset = landed_with_a_check(&api, &ada).await;
    let runner = register_runner(&api, &ada, "r1", &["linux"]).await;

    let claimed = claim(&api, &runner).await;
    assert_eq!(claimed.status, StatusCode::OK, "{}", claimed.body);
    assert_eq!(claimed.body["organisation"], "acme");
    assert_eq!(claimed.body["changeset"], changeset);
    assert_eq!(claimed.body["check"]["name"], "lint");
    assert_eq!(claimed.body["check"]["command"], "true");
    assert_eq!(claimed.body["check"]["timeout"], 60);
    assert_eq!(claimed.body["heartbeat_interval"], 10);
    let files = claimed.body["files"].as_array().unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0]["path"], "src/a.txt");
    assert_eq!(
        files[0]["address"],
        "sha256:2c8b08da5ce60398e1f19af0e5dccc744df274b826abe585eaba68c525434806"
    );
    assert_eq!(files[0]["executable"], false);
    assert_eq!(api.download("acme", &files[0]["download"]).await, "one\n");
}

#[tokio::test]
async fn answers_no_content_when_the_hold_ends_without_a_job() {
    let api = Api::new();
    let ada = api.token("ada");
    api.repository(&ada, "acme").await;
    let runner = register_runner(&api, &ada, "r1", &["linux"]).await;

    let started = Instant::now();
    let claimed = claim(&api, &runner).await;
    assert_eq!(claimed.status, StatusCode::NO_CONTENT);
    assert!(started.elapsed() >= HOLD, "{:?}", started.elapsed());
}

#[tokio::test]
async fn hands_a_job_queued_during_the_hold_to_the_waiting_claim() {
    let api = Api::new();
    let ada = api.token("ada");
    let changeset = landed_with_a_check(&api, &ada).await;
    let runner = register_runner(&api, &ada, "r1", &["windows"]).await;

    let waiting = claim(&api, &runner);
    let queue = async {
        tokio::time::sleep(Duration::from_millis(100)).await;
        api.queue_check("acme", &changeset, "late", &["windows"])
            .await;
    };
    let (claimed, ()) = tokio::join!(waiting, queue);
    assert_eq!(claimed.status, StatusCode::OK, "{}", claimed.body);
    assert_eq!(claimed.body["check"]["name"], "late");
}

#[tokio::test]
async fn refuses_an_unregistered_principal_on_every_runner_call() {
    let api = Api::new();
    let ada = api.token("ada");
    landed_with_a_check(&api, &ada).await;
    let stranger = api.token("stranger");
    let body = json!({"status": "passed", "exit_code": 0, "output_tail": ""});
    for (uri, body) in [
        ("/v1/runner/jobs/claim", json!({})),
        ("/v1/runner/jobs/1/heartbeat", json!({})),
        ("/v1/runner/jobs/1/result", body),
    ] {
        let reply = api.post(uri, &stranger, body).await;
        assert_eq!(reply.status, StatusCode::FORBIDDEN, "{uri}");
        assert_eq!(reply.content_type, PROBLEM, "{uri}");
    }
}

#[tokio::test]
async fn answers_not_found_for_a_job_the_caller_does_not_hold() {
    let api = Api::new();
    let ada = api.token("ada");
    landed_with_a_check(&api, &ada).await;
    let holder = register_runner(&api, &ada, "holder", &["linux"]).await;
    let other = register_runner(&api, &ada, "other", &["linux"]).await;
    let id = claim(&api, &holder).await.body["id"].as_u64().unwrap();

    let heartbeat = api
        .post(
            &format!("/v1/runner/jobs/{id}/heartbeat"),
            &other,
            json!({}),
        )
        .await;
    assert_eq!(heartbeat.status, StatusCode::NOT_FOUND);
    let result = api
        .post(
            &format!("/v1/runner/jobs/{id}/result"),
            &other,
            json!({"status": "passed", "exit_code": 0, "output_tail": ""}),
        )
        .await;
    assert_eq!(result.status, StatusCode::NOT_FOUND);
    // The control: the holder is told to continue.
    let held = api
        .post(
            &format!("/v1/runner/jobs/{id}/heartbeat"),
            &holder,
            json!({}),
        )
        .await;
    assert_eq!(held.status, StatusCode::OK);
    assert_eq!(held.body, json!({"action": "continue"}));
}

#[tokio::test]
async fn tells_a_runner_to_stop_a_cancelled_job() {
    let api = Api::new();
    let ada = api.token("ada");
    landed_with_a_check(&api, &ada).await;
    let runner = register_runner(&api, &ada, "r1", &["linux"]).await;
    let id = claim(&api, &runner).await.body["id"].as_u64().unwrap();
    let cancel = format!("/v1/orgs/acme/repos/app/jobs/{id}/cancel");

    // A stranger to the repository cannot cancel; the admin can, once.
    let bob = api.token("bob");
    let denied = api.post(&cancel, &bob, json!({})).await;
    assert_eq!(denied.status, StatusCode::NOT_FOUND);
    let cancelled = api.post(&cancel, &ada, json!({})).await;
    assert_eq!(cancelled.status, StatusCode::NO_CONTENT);
    let heartbeat = api
        .post(
            &format!("/v1/runner/jobs/{id}/heartbeat"),
            &runner,
            json!({}),
        )
        .await;
    assert_eq!(heartbeat.status, StatusCode::OK);
    assert_eq!(
        heartbeat.body,
        json!({"action": "stop", "reason": "cancelled"})
    );

    let again = api.post(&cancel, &ada, json!({})).await;
    assert_eq!(again.status, StatusCode::CONFLICT);
    let unknown = api
        .post("/v1/orgs/acme/repos/app/jobs/999/cancel", &ada, json!({}))
        .await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn lists_the_checks_of_a_changeset_through_a_run() {
    let api = Api::new();
    let ada = api.token("ada");
    let changeset = landed_with_a_check(&api, &ada).await;
    let uri = format!("{CHECKS}/{changeset}/checks");

    let queued = api.get(&uri, &ada).await;
    assert_eq!(queued.status, StatusCode::OK, "{}", queued.body);
    let check = &queued.body["checks"][0];
    assert_eq!(check["name"], "lint");
    assert_eq!(check["blocking"], true);
    assert_eq!(check["labels"], json!(["linux"]));
    assert_eq!(check["state"], "queued");
    assert_eq!(check["exit_code"], Value::Null);
    assert_eq!(check["revert"], Value::Null);

    let runner = register_runner(&api, &ada, "r1", &["linux"]).await;
    let id = claim(&api, &runner).await.body["id"].as_u64().unwrap();
    let running = api.get(&uri, &ada).await;
    assert_eq!(running.body["checks"][0]["state"], "running");
    assert!(running.body["checks"][0]["deadline"].is_i64());
    assert!(running.body["checks"][0]["lease_expires_at"].is_i64());

    let reported = api
        .post(
            &format!("/v1/runner/jobs/{id}/result"),
            &runner,
            json!({"status": "failed", "exit_code": 3, "output_tail": "boom\n"}),
        )
        .await;
    assert_eq!(reported.status, StatusCode::NO_CONTENT);
    let failed = api.get(&uri, &ada).await;
    assert_eq!(failed.body["checks"][0]["state"], "failed");
    assert_eq!(failed.body["checks"][0]["exit_code"], 3);
    assert_eq!(failed.body["checks"][0]["output_tail"], "boom\n");
    // A failed blocking check reverts the changeset, and the listing says what landed.
    let revert = &failed.body["checks"][0]["revert"];
    assert_eq!(revert["outcome"], "landed");
    assert!(revert["changeset"].is_string(), "{revert}");

    let malformed = api.get(&format!("{CHECKS}/nope/checks"), &ada).await;
    assert_eq!(malformed.status, StatusCode::BAD_REQUEST);
    let bob = api.token("bob");
    let hidden = api.get(&uri, &bob).await;
    assert_eq!(hidden.status, StatusCode::NOT_FOUND);

    // A revert that failed after its reservation is listed with its reason.
    api.record_failed_revert(id, "revert not landed: log down")
        .await;
    let listed = api.get(&uri, &ada).await;
    assert_eq!(
        listed.body["checks"][0]["revert"],
        json!({"outcome": "failed", "reason": "revert not landed: log down"})
    );
}

#[tokio::test]
async fn serves_last_green_once_the_blocking_checks_pass() {
    let api = Api::new();
    let ada = api.token("ada");
    let changeset = landed_with_a_check(&api, &ada).await;
    let uri = "/v1/orgs/acme/repos/app/refs/last-green";

    let unset = api.get(uri, &ada).await;
    assert_eq!(unset.status, StatusCode::NOT_FOUND);
    assert_eq!(unset.content_type, PROBLEM);

    let runner = register_runner(&api, &ada, "r1", &["linux"]).await;
    let id = claim(&api, &runner).await.body["id"].as_u64().unwrap();
    let reported = api
        .post(
            &format!("/v1/runner/jobs/{id}/result"),
            &runner,
            json!({"status": "passed", "output_tail": "ok\n"}),
        )
        .await;
    assert_eq!(reported.status, StatusCode::NO_CONTENT);

    let set = api.get(uri, &ada).await;
    assert_eq!(set.status, StatusCode::OK, "{}", set.body);
    assert_eq!(
        set.body,
        json!({"name": "last-green", "changeset": changeset})
    );
    let unknown = api.get("/v1/orgs/acme/repos/app/refs/nothing", &ada).await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn serves_main_once_a_changeset_has_landed() {
    let api = Api::new();
    let ada = api.token("ada");
    let uri = "/v1/orgs/acme/repos/app/refs/main";
    let changeset = landed_with_a_check(&api, &ada).await;
    let set = api.get(uri, &ada).await;
    assert_eq!(set.status, StatusCode::OK, "{}", set.body);
    assert_eq!(set.body, json!({"name": "main", "changeset": changeset}));
}

#[tokio::test]
async fn reports_each_outcome_as_its_own_state() {
    let api = Api::new();
    let ada = api.token("ada");
    let changeset = landed_with_a_check(&api, &ada).await;
    let uri = format!("{CHECKS}/{changeset}/checks");
    let runner = register_runner(&api, &ada, "r1", &["linux"]).await;

    // Each status is its own outcome; no exit code stays null rather than 0.
    for (name, status, exit_code) in [
        ("lint", "cancelled", Value::Null),
        ("two", "timed_out", Value::Null),
        ("three", "failed", json!(7)),
    ] {
        if name != "lint" {
            api.queue_check("acme", &changeset, name, &["linux"]).await;
        }
        // The earlier jobs are finished, so the claim hands out this one.
        let claimed = claim(&api, &runner).await;
        assert_eq!(claimed.body["check"]["name"], name, "{}", claimed.body);
        let id = claimed.body["id"].as_u64().unwrap();
        let body = if exit_code.is_null() {
            json!({"status": status, "output_tail": ""})
        } else {
            json!({"status": status, "exit_code": exit_code, "output_tail": ""})
        };
        let reported = api
            .post(&format!("/v1/runner/jobs/{id}/result"), &runner, body)
            .await;
        assert_eq!(reported.status, StatusCode::NO_CONTENT, "{status}");
        let listed = api.get(&uri, &ada).await;
        let shown = listed.body["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|check| check["name"] == name)
            .unwrap();
        assert_eq!(shown["state"], status);
        assert_eq!(shown["exit_code"], exit_code, "{status}");
    }
}
