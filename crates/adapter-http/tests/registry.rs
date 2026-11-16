//! The registry token endpoint over the in-memory adapters, through requests to the router.

#[cfg(test)]
pub mod common;

use std::sync::Arc;

use adapter_http::ApiState;
use adapter_memory::MemoryRegistryTokenIssuance;
use app::domain::image::ImageAction;
use app::port::driven::registry_token_issuance::GrantedEntry;
use app::use_case::administration::Administration;
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use common::{Api, NOW, REGISTRY_SERVICE, state};
use serde_json::Value;
use test_support::contract::Deployment;
use tower::ServiceExt;

const PROBLEM: &str = "application/problem+json";

/// What the router answered, headers included.
#[cfg(test)]
struct Answer {
    status: StatusCode,
    headers: axum::http::HeaderMap,
    body: Value,
}

#[cfg(test)]
async fn send(router: &Router, method: Method, uri: &str, authorization: Option<String>) -> Answer {
    let mut request = Request::builder().method(method).uri(uri);
    if let Some(authorization) = authorization {
        request = request.header(header::AUTHORIZATION, authorization);
    }
    let response = router
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let (parts, body) = response.into_parts();
    let bytes = to_bytes(body, usize::MAX).await.unwrap();
    Answer {
        status: parts.status,
        headers: parts.headers,
        body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    }
}

#[cfg(test)]
fn basic(user: &str, password: &str) -> String {
    format!("Basic {}", STANDARD.encode(format!("{user}:{password}")))
}

/// A router serving the registry, an organisation `acme` of `ada`, and the issuance behind it.
#[cfg(test)]
async fn registry() -> (Api, Router, Arc<MemoryRegistryTokenIssuance>) {
    let api = Api::new();
    let ada = api.token("ada");
    api.repository(&ada, "acme").await;
    let _ = api.token("bob");
    let issuance = Arc::new(MemoryRegistryTokenIssuance::default());
    let registry =
        api.stores
            .assembly()
            .registry_token(issuance.clone(), REGISTRY_SERVICE.to_owned(), || NOW);
    let router = adapter_http::router(ApiState {
        registry_token: Some(Arc::new(registry)),
        ..state(&api.stores, Administration::BuiltIn)
    });
    (api, router, issuance)
}

#[cfg(test)]
const SCOPES: &str = "service=registry.test\
    &scope=repository%3Aacme%2Fapp%2Fweb%3Apull%2Cpush\
    &scope=repository%3Aacme%2Fnothing%2Fweb%3Apull";

#[tokio::test]
async fn gives_a_token_for_basic_credentials_and_for_a_bearer_token() {
    let (_api, router, issuance) = registry().await;
    let uri = format!("/v1/registry/token?{SCOPES}");

    let by_basic = send(
        &router,
        Method::GET,
        &uri,
        Some(basic("ignored", "token-ada")),
    )
    .await;
    let by_bearer = send(
        &router,
        Method::GET,
        &uri,
        Some("Bearer token-ada".to_owned()),
    )
    .await;

    for answer in [&by_basic, &by_bearer] {
        assert_eq!(answer.status, StatusCode::OK);
        assert_eq!(answer.body["token"], answer.body["access_token"]);
        assert_eq!(answer.body["expires_in"], 300);
        assert_eq!(answer.body["issued_at"], "2023-11-14T22:13:20Z");
        let record = issuance
            .carried_by(answer.body["token"].as_str().unwrap())
            .unwrap();
        assert_eq!(record.subject, "ada");
        assert_eq!(record.service, REGISTRY_SERVICE);
        assert_eq!(
            record.entries,
            [
                GrantedEntry {
                    kind: "repository".to_owned(),
                    name: "acme/app/web".to_owned(),
                    actions: vec![ImageAction::Pull, ImageAction::Push],
                },
                GrantedEntry {
                    kind: "repository".to_owned(),
                    name: "acme/nothing/web".to_owned(),
                    actions: Vec::new(),
                },
            ]
        );
    }
    // Control: a caller who is no member is granted nothing on the same scopes.
    let outsider = send(&router, Method::GET, &uri, Some(basic("bob", "token-bob"))).await;
    let record = issuance
        .carried_by(outsider.body["token"].as_str().unwrap())
        .unwrap();
    assert!(record.entries.iter().all(|entry| entry.actions.is_empty()));
}

#[tokio::test]
async fn refuses_a_request_without_a_valid_credential_and_challenges_with_basic() {
    let (_api, router, _issuance) = registry().await;
    let uri = format!("/v1/registry/token?{SCOPES}");
    let garbled = format!("Basic {}", STANDARD.encode("no-colon"));

    for authorization in [
        None,
        Some("Bearer unknown".to_owned()),
        Some(basic("ada", "unknown")),
        Some(basic("ada", "")),
        Some("Basic ***".to_owned()),
        Some(garbled),
        Some("Digest abc".to_owned()),
    ] {
        let answer = send(&router, Method::GET, &uri, authorization.clone()).await;
        assert_eq!(answer.status, StatusCode::UNAUTHORIZED, "{authorization:?}");
        assert_eq!(answer.headers[header::CONTENT_TYPE], PROBLEM);
        assert_eq!(
            answer.headers[header::WWW_AUTHENTICATE],
            r#"Basic realm="codehub""#
        );
        assert_eq!(answer.body["status"], 401);
    }
    // Control: the same request with a known token is served.
    let served = send(&router, Method::GET, &uri, Some(basic("ada", "token-ada"))).await;
    assert_eq!(served.status, StatusCode::OK);
}

#[tokio::test]
async fn refuses_another_service_and_too_many_scopes() {
    let (_api, router, _issuance) = registry().await;
    let ask = |query: String| {
        let router = router.clone();
        async move {
            send(
                &router,
                Method::GET,
                &format!("/v1/registry/token?{query}"),
                Some(basic("ada", "token-ada")),
            )
            .await
        }
    };
    let scopes = |count: usize| "&scope=repository%3Aacme%2Fapp%2Fweb%3Apull".repeat(count);

    for query in [
        "service=other".to_owned(),
        String::new(),
        format!("service=registry.test{}", scopes(17)),
    ] {
        let answer = ask(query.clone()).await;
        assert_eq!(answer.status, StatusCode::BAD_REQUEST, "{query}");
        assert_eq!(answer.headers[header::CONTENT_TYPE], PROBLEM);
    }
    // Control: no scope at all, and sixteen, are served.
    assert_eq!(
        ask("service=registry.test".to_owned()).await.status,
        StatusCode::OK
    );
    assert_eq!(
        ask(format!("service=registry.test{}", scopes(16)))
            .await
            .status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn answers_a_post_with_405_and_allow_get() {
    let (_api, router, _issuance) = registry().await;

    let answer = send(
        &router,
        Method::POST,
        "/v1/registry/token",
        Some(basic("ada", "token-ada")),
    )
    .await;

    assert_eq!(answer.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(answer.headers[header::ALLOW], "GET");
    assert_eq!(answer.headers[header::CONTENT_TYPE], PROBLEM);
    // Control: the GET of the same path and credentials is served.
    let get = send(
        &router,
        Method::GET,
        "/v1/registry/token?service=registry.test",
        Some(basic("ada", "token-ada")),
    )
    .await;
    assert_eq!(get.status, StatusCode::OK);
}

#[tokio::test]
async fn answers_404_where_the_server_issues_no_registry_tokens() {
    let api = Api::new();
    let _ = api.token("ada");
    let router = adapter_http::router(ApiState {
        registry_token: None,
        ..state(&api.stores, Administration::BuiltIn)
    });

    for method in [Method::GET, Method::POST] {
        let answer = send(
            &router,
            method.clone(),
            "/v1/registry/token?service=registry.test",
            Some(basic("ada", "token-ada")),
        )
        .await;
        assert_eq!(answer.status, StatusCode::NOT_FOUND, "{method}");
        assert_eq!(answer.headers[header::CONTENT_TYPE], PROBLEM);
    }
    // Control: the configured fixture serves the same request.
    let (_api, configured, _issuance) = registry().await;
    let served = send(
        &configured,
        Method::GET,
        "/v1/registry/token?service=registry.test",
        Some(basic("ada", "token-ada")),
    )
    .await;
    assert_eq!(served.status, StatusCode::OK);
}
