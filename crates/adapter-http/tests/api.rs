//! The HTTP API over the in-memory adapters, through requests to the router.

#[cfg(test)]
pub mod common;

use std::collections::BTreeSet;

use app::domain::address::Address;
use app::domain::name::{OrganisationName, RepositoryName};
use app::domain::repository::RepositoryId;
use app::port::driven::repository_directory::DirectoryError;
use app::port::driving::repositories::{Repositories, RepositoriesError};
use axum::http::{Method, StatusCode};
use common::{Api, Reply, router_with, send};
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
    use app::use_case::repositories::RepositoriesService;
    let stores = api.stores;
    router_with(
        stores,
        RepositoriesService::new(
            &stores.directory,
            &stores.authorisation,
            &stores.membership,
            &(),
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
    use app::use_case::members::MembersService;
    use app::use_case::repositories::RepositoriesService;
    use app::use_case::source_control::SourceControlService;
    let stores = api.stores;
    adapter_http::router(
        AuthenticationService::new(&stores.identity),
        RepositoriesService::new(
            &stores.directory,
            &stores.authorisation,
            &stores.membership,
            &(),
            Administration::External,
        ),
        MembersService::new(
            &stores.directory,
            &stores.authorisation,
            &stores.membership,
            Administration::External,
        ),
        SourceControlService::new(
            &stores.objects,
            &stores.log,
            &stores.derived,
            &stores.drafts,
            &stores.directory,
            &stores.authorisation,
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
    assert_eq!(deletes, 3);
    assert!(document["paths"]["/v1/orgs/{org}/members"]["put"]["requestBody"].is_object());
}

#[tokio::test]
async fn documents_every_route_and_routes_every_documented_operation() {
    let routed: BTreeSet<(String, String)> = adapter_http::ROUTES
        .iter()
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
        let reply = api.send(method.clone(), &uri, None, None).await;
        let expected = if uri == "/v1/openapi.json" {
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
