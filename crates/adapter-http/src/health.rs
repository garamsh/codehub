//! The liveness and readiness probes: unauthenticated and outside `/v1`.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use crate::ApiState;
use crate::problem::Problem;

/// Answers 200 while the process serves requests; it depends on nothing.
pub(crate) async fn live() -> StatusCode {
    StatusCode::OK
}

/// Answers 200 where every dependency answers, and 503 with the reason where one does not.
pub(crate) async fn ready(State(ports): State<Arc<ApiState>>) -> Response {
    match ports.readiness.check().await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(not_ready) => Problem::unavailable(not_ready.0).into_response(),
    }
}
