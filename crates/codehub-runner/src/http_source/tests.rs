use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{Value, json};

use super::*;

static NEXT: AtomicU64 = AtomicU64::new(1);

type Seen = Arc<Mutex<Vec<(u64, Value)>>>;

/// A server that answers the heartbeat of job 1 with a stop for time, of job 2 with a stop
/// for cancellation, of job 3 with a continue and of any other job with a 404; that has no
/// job to claim; and that keeps the bodies of the results it is sent. Returns its URL.
async fn server(seen: Seen) -> String {
    let app = Router::new()
        .route(
            "/v1/runner/jobs/claim",
            post(|| async { StatusCode::NO_CONTENT }),
        )
        .route(
            "/v1/runner/jobs/{id}/heartbeat",
            post(|Path(id): Path<u64>| async move {
                match id {
                    1 => (
                        StatusCode::OK,
                        Json(json!({"action": "stop", "reason": "timed_out"})),
                    ),
                    2 => (
                        StatusCode::OK,
                        Json(json!({"action": "stop", "reason": "cancelled"})),
                    ),
                    3 => (StatusCode::OK, Json(json!({"action": "continue"}))),
                    _ => (StatusCode::NOT_FOUND, Json(Value::Null)),
                }
            }),
        )
        .route(
            "/v1/runner/jobs/{id}/result",
            post(
                |State(seen): State<Seen>, Path(id): Path<u64>, Json(body): Json<Value>| async move {
                    seen.lock().unwrap().push((id, body));
                    StatusCode::NO_CONTENT
                },
            ),
        )
        .with_state(seen);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await });
    url
}

/// A source of the server, and the token file it reads, which the caller removes.
async fn source(seen: Seen) -> (HttpJobSource, PathBuf) {
    let url = server(seen).await;
    let file = std::env::temp_dir().join(format!(
        "codehub-runner-http-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::write(&file, "token\n").unwrap();
    (
        HttpJobSource::new(
            reqwest::Client::new(),
            &url,
            TokenSource::File(file.clone()),
        ),
        file,
    )
}

#[tokio::test]
async fn a_heartbeat_answer_is_read_by_its_reason() {
    let (source, file) = source(Seen::default()).await;
    assert!(matches!(
        source.heartbeat(JobId::new(1)).await,
        Ok(Heartbeat::Stop(StopReason::TimedOut))
    ));
    assert!(matches!(
        source.heartbeat(JobId::new(2)).await,
        Ok(Heartbeat::Stop(StopReason::Cancelled))
    ));
    assert!(matches!(
        source.heartbeat(JobId::new(3)).await,
        Ok(Heartbeat::Continue)
    ));
    std::fs::remove_file(file).unwrap();
}

#[tokio::test]
async fn a_job_the_server_does_not_hold_is_not_held() {
    let (source, file) = source(Seen::default()).await;
    assert!(matches!(
        source.heartbeat(JobId::new(9)).await,
        Err(SourceError::NotHeld)
    ));
    std::fs::remove_file(file).unwrap();
}

#[tokio::test]
async fn no_content_is_no_job() {
    let (source, file) = source(Seen::default()).await;
    assert!(matches!(source.claim().await, Ok(None)));
    std::fs::remove_file(file).unwrap();
}

#[tokio::test]
async fn a_report_carries_its_status_exit_code_and_output() {
    let seen = Seen::default();
    let (source, file) = source(seen.clone()).await;
    let failed = Report {
        status: Status::Failed,
        exit_code: Some(7),
        output: "broken".into(),
    };
    source.report(JobId::new(5), &failed).await.unwrap();
    let timed_out = Report {
        status: Status::TimedOut,
        exit_code: None,
        output: String::new(),
    };
    source.report(JobId::new(6), &timed_out).await.unwrap();
    let seen = seen.lock().unwrap().clone();
    assert_eq!(
        seen,
        vec![
            (
                5,
                json!({"status": "failed", "exit_code": 7, "output_tail": "broken"})
            ),
            (6, json!({"status": "timed_out", "output_tail": ""})),
        ]
    );
    std::fs::remove_file(file).unwrap();
}
