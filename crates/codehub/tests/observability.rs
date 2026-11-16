//! The shipped `codehub` binary's metrics, traces and log (ADR 0056) against the services the
//! checks run (`dev/test.env`): the scrape after a landing and a queued check, the spans a request
//! exports to a collector, and the log line a request leaves.

#[cfg(test)]
pub mod common;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::routing::post;
use common::{Client, Server, TestDatabase, token};
use reqwest::StatusCode;
use serde_json::{Value, json};

const BOUND: Duration = Duration::from_secs(20);

const CHECKS: &str = "[[check]]\nname = \"unit\"\ncommand = \"true\"\ntimeout = 60\n";

/// The bodies of the trace exports a collector received.
type Exports = Arc<Mutex<Vec<Value>>>;

/// An OTLP collector that keeps the JSON each trace export carries; returns its URL.
#[cfg(test)]
async fn collector(exports: Exports) -> String {
    let app = Router::new()
        .route(
            "/v1/traces",
            post(|State(exports): State<Exports>, body: Bytes| async move {
                if let Ok(export) = serde_json::from_slice(&body) {
                    exports.lock().unwrap().push(export);
                }
                axum::Json(json!({}))
            }),
        )
        .with_state(exports);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await });
    url
}

/// The exported spans: `(name, trace id, span id, parent span id)`.
#[cfg(test)]
fn spans(exports: &Exports) -> Vec<(String, String, String, String)> {
    let mut found = Vec::new();
    for export in exports.lock().unwrap().iter() {
        for resource in export["resourceSpans"].as_array().into_iter().flatten() {
            for scope in resource["scopeSpans"].as_array().into_iter().flatten() {
                for span in scope["spans"].as_array().into_iter().flatten() {
                    let text = |key: &str| span[key].as_str().unwrap_or_default().to_owned();
                    found.push((
                        text("name"),
                        text("traceId"),
                        text("spanId"),
                        text("parentSpanId"),
                    ));
                }
            }
        }
    }
    found
}

/// What `server` serves at `/metrics`, with no token.
#[cfg(test)]
async fn scrape(server: &Server) -> String {
    let response = reqwest::get(format!("{}/metrics", server.base))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response.text().await.unwrap()
}

/// The text of the scrape once `done` holds of it, within the bound.
#[cfg(test)]
async fn scrape_until(server: &Server, done: impl Fn(&str) -> bool) -> String {
    let until = Instant::now() + BOUND;
    loop {
        let text = scrape(server).await;
        if done(&text) || Instant::now() > until {
            return text;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

#[tokio::test]
async fn the_scrape_reports_requests_the_landing_gate_and_the_queue() {
    let database = TestDatabase::new().await;
    let server = Server::start_with(&database, &[("CODEHUB_BACKLOG_INTERVAL", "1")]);
    let alice = Client::new(&server.base, Some(token("alice").await));
    alice.create_repository("acme", "app").await;
    // Landing the checks file queues a check that no runner claims.
    alice
        .land_files("acme", "app", "work", &[(".codehub/checks.toml", CHECKS)])
        .await;

    // A method no one registered, sent with no credential, makes no series of its own.
    reqwest::Client::new()
        .request(
            reqwest::Method::from_bytes(b"BREW").unwrap(),
            format!("{}/v1/orgs", server.base),
        )
        .send()
        .await
        .unwrap();

    let text = scrape_until(&server, |text| {
        text.lines()
            .any(|line| line.starts_with("codehub_queue_depth{") && line.ends_with(" 1"))
    })
    .await;

    let has = |prefix: &str| text.lines().any(|line| line.starts_with(prefix));
    assert!(
        has(
            "codehub_http_request_duration_seconds_count{method=\"POST\",route=\"/v1/orgs\",status=\"201\"}"
        ),
        "{text}"
    );
    assert!(has("codehub_landing_gate_wait_seconds_count"), "{text}");
    assert!(has("codehub_landing_gate_hold_seconds_count"), "{text}");
    assert!(has("codehub_landing_attempts_count"), "{text}");
    assert!(
        has("codehub_outbound_call_duration_seconds_count{dependency=\"identity\""),
        "{text}"
    );
    assert!(
        has("codehub_outbound_call_duration_seconds_count{dependency=\"object_store\""),
        "{text}"
    );
    assert!(has("codehub_db_pool_connections"), "{text}");
    assert!(
        text.lines()
            .any(|line| line.starts_with("codehub_queue_depth{") && line.ends_with(" 1")),
        "{text}"
    );
    assert!(has("codehub_outbox_pending"), "{text}");
    assert!(
        has("codehub_http_request_duration_seconds_count{method=\"other\""),
        "{text}"
    );
    assert!(!text.contains("BREW"), "{text}");
    // The scrape names no credential and no path parameter.
    assert!(!text.contains("Bearer"), "{text}");
    assert!(!text.contains("/v1/orgs/acme"), "{text}");
}

#[tokio::test]
async fn a_request_exports_one_trace_across_its_calls_and_names_it_in_the_log() {
    let database = TestDatabase::new().await;
    let exports = Exports::default();
    let collector = collector(Arc::clone(&exports)).await;
    let server = Server::start_with(
        &database,
        &[
            ("OTEL_EXPORTER_OTLP_ENDPOINT", &collector),
            ("OTEL_EXPORTER_OTLP_PROTOCOL", "http/json"),
            ("OTEL_BSP_SCHEDULE_DELAY", "200"),
        ],
    );
    let bearer = token("alice").await;
    let alice = Client::new(&server.base, Some(bearer.clone()));
    alice.create_repository("acme", "app").await;

    // A landing under a trace the caller names: the server continues it.
    let trace = "4bf92f3577b34da6a3ce929d0e0e4736";
    let parent = "00f067aa0ba902b7";
    let upload = reqwest::Client::new()
        .post(format!("{}/v1/orgs/acme/repos/app/uploads", server.base))
        .bearer_auth(&bearer)
        .header("traceparent", format!("00-{trace}-{parent}-01"))
        .json(&json!({"address": codehub_address(), "size": 1}))
        .send()
        .await
        .unwrap();
    assert_eq!(upload.status(), StatusCode::OK);
    alice
        .land_files("acme", "app", "work", &[(".codehub/checks.toml", CHECKS)])
        .await;

    let until = Instant::now() + BOUND;
    // The request span ends after the spans it holds, and the batches that export them are
    // separate: the landing is whole once its request's span has arrived.
    let landed = |spans: &[(String, String, String, String)]| {
        [
            "landing.swap",
            "POST /v1/orgs/{org}/repos/{repo}/drafts/{draft}/land",
        ]
        .iter()
        .all(|wanted| spans.iter().any(|(name, ..)| name == wanted))
    };
    while !landed(&spans(&exports)) && Instant::now() < until {
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let spans = spans(&exports);

    // The caller's trace: the request span is a child of the caller's span, and the identity
    // check is a child of the request.
    let request = spans
        .iter()
        .find(|(name, trace_id, ..)| {
            name == "POST /v1/orgs/{org}/repos/{repo}/uploads" && trace_id == trace
        })
        .unwrap_or_else(|| panic!("no request span in the caller's trace: {spans:?}"));
    assert_eq!(request.3, parent);
    assert!(
        spans
            .iter()
            .any(|(name, id, _, up)| name == "identity.verify" && id == trace && *up == request.2),
        "{spans:?}"
    );

    // The landing: its request, its gate and its swap are one trace.
    let swap = spans
        .iter()
        .find(|(name, ..)| name == "landing.swap")
        .unwrap_or_else(|| panic!("no landing.swap span: {spans:?}"));
    for name in [
        "POST /v1/orgs/{org}/repos/{repo}/drafts/{draft}/land",
        "landing.gate_wait",
        "decision_point.decide",
        "object_store.get",
        "landing.attempt",
    ] {
        assert!(
            spans
                .iter()
                .any(|(found, id, ..)| found == name && *id == swap.1),
            "no {name} span in the landing's trace: {spans:?}"
        );
    }

    // The log line of the request carries its trace id, and no line carries the token.
    let lines = server.stdout_lines();
    let logged = lines
        .iter()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|line| line["span"]["trace_id"] == trace)
        .unwrap_or_else(|| panic!("no log line names the trace: {lines:?}"));
    assert_eq!(logged["target"], "codehub::http");
    assert_eq!(logged["fields"]["status"], 200);
    assert_eq!(
        logged["fields"]["route"],
        "/v1/orgs/{org}/repos/{repo}/uploads"
    );
    assert!(
        lines.iter().all(|line| !line.contains(&bearer)),
        "a log line carries the bearer token"
    );
    assert!(
        lines.iter().all(|line| !line.contains("Authorization")),
        "a log line names the Authorization header"
    );
}

/// The address of one byte, for an upload grant request.
#[cfg(test)]
fn codehub_address() -> String {
    app::domain::address::Address::of(b"x").to_string()
}

#[tokio::test]
async fn a_server_with_no_collector_still_names_the_trace_in_its_log() {
    let database = TestDatabase::new().await;
    let server = Server::start(&database);
    let alice = Client::new(&server.base, Some(token("alice").await));
    alice.create_repository("acme", "app").await;

    let until = Instant::now() + BOUND;
    let named = loop {
        let named = server
            .stdout_lines()
            .iter()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .any(|line| {
                line["target"] == "codehub::http"
                    && line["span"]["trace_id"]
                        .as_str()
                        .is_some_and(|id| id.len() == 32 && id != "0".repeat(32))
            });
        if named || Instant::now() > until {
            break named;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert!(named, "{:?}", server.stdout_lines());
}
