//! The span, the log line and the duration of every API request, and the scrape of `/metrics`
//! (ADR 0056). The probes and `/metrics` itself are outside it.

use std::sync::Arc;
use std::time::Instant;

use axum::extract::{MatchedPath, Request, State};
use axum::http::header;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use opentelemetry::trace::TraceContextExt;
use opentelemetry_http::HeaderExtractor;
use tracing::{Instrument, field};
use tracing_opentelemetry::OpenTelemetrySpanExt;

use crate::Scrape;

/// The histogram of API request durations, by method, route and status.
const REQUEST_DURATION: &str = "codehub_http_request_duration_seconds";

/// Wraps one request in the span `http.request`, continuing the trace its `traceparent` header
/// names, logs it once and records its duration. The log line carries the method, the route
/// template and the status, never the headers, the query or the body.
pub(crate) async fn observe(request: Request, next: Next) -> Response {
    let method = request.method().to_string();
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map_or_else(|| "unmatched".to_owned(), |path| path.as_str().to_owned());
    let parent = opentelemetry::global::get_text_map_propagator(|propagator| {
        propagator.extract(&HeaderExtractor(request.headers()))
    });
    let span = tracing::info_span!(
        "http.request",
        otel.name = format!("{method} {route}"),
        http.method = %method,
        http.route = %route,
        http.status_code = field::Empty,
        trace_id = field::Empty,
    );
    // Without a tracing-opentelemetry layer there is no parent to set and no trace to name.
    if span.set_parent(parent).is_ok() {
        let trace_id = span.context().span().span_context().trace_id();
        span.record("trace_id", trace_id.to_string());
    }
    let started = Instant::now();
    let response = next.run(request).instrument(span.clone()).await;
    let status = response.status().as_u16();
    let elapsed = started.elapsed();
    span.record("http.status_code", status);
    span.in_scope(|| {
        tracing::info!(
            target: "codehub::http",
            method = %method,
            route = %route,
            status,
            duration_ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
            "request"
        );
    });
    metrics::histogram!(
        REQUEST_DURATION,
        "method" => method,
        "route" => route,
        "status" => status.to_string()
    )
    .record(elapsed.as_secs_f64());
    response
}

/// Answers a scrape with the metrics in the Prometheus text format.
pub(crate) async fn scrape(State(scrape): State<Arc<dyn Scrape>>) -> Response {
    (
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        scrape.render(),
    )
        .into_response()
}
