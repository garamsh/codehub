use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{Value, json};
use sha2::Digest as _;

use super::*;

static NEXT: AtomicU64 = AtomicU64::new(1);

type Seen = Arc<Mutex<Vec<(u64, Value)>>>;

/// The address and bytes of each file PUT to a grant.
type Puts = Arc<Mutex<Vec<(String, Vec<u8>)>>>;

/// A server that never answers the heartbeat of job 7, answers a claim with `claim` (no job
/// where `None`), and answers the heartbeat of job 1 with a stop for time, of job 2 with a stop
/// for cancellation, of job 3 with a continue and of any other job with a 404; that has no
/// job to claim; and that keeps the bodies of the results it is sent. Returns its URL.
async fn server(seen: Seen, claim: Option<Value>) -> String {
    let app = Router::new()
        .route(
            "/v1/runner/jobs/claim",
            post(move || async move {
                claim.map_or_else(
                    || StatusCode::NO_CONTENT.into_response(),
                    |job| Json(job).into_response(),
                )
            }),
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
                    7 => std::future::pending().await,
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
    source_claiming(seen, None).await
}

async fn source_claiming(seen: Seen, claim: Option<Value>) -> (HttpJobSource, PathBuf) {
    let url = server(seen, claim).await;
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
        produced: Produced::default(),
    };
    source.report(JobId::new(5), &failed).await.unwrap();
    let timed_out = Report {
        status: Status::TimedOut,
        exit_code: None,
        output: String::new(),
        produced: Produced::default(),
    };
    source.report(JobId::new(6), &timed_out).await.unwrap();
    let seen = seen.lock().unwrap().clone();
    assert_eq!(
        seen,
        vec![
            (
                5,
                json!({"status": "failed", "exit_code": 7, "output_tail": "broken",
                       "outputs": [], "caches": []})
            ),
            (
                6,
                json!({"status": "timed_out", "output_tail": "", "outputs": [], "caches": []})
            ),
        ]
    );
    std::fs::remove_file(file).unwrap();
}

#[tokio::test]
async fn a_heartbeat_the_server_does_not_answer_ends_at_the_call_timeout() {
    let (source, file) = source(Seen::default()).await;
    let source = source.with_call_timeout(Duration::from_millis(200));
    let answer = tokio::time::timeout(Duration::from_secs(10), source.heartbeat(JobId::new(7)))
        .await
        .expect("the heartbeat outlived the call timeout");
    assert!(matches!(answer, Err(SourceError::Transport(_))));
    std::fs::remove_file(file).unwrap();
}

fn claimed(path: &str) -> Value {
    json!({
        "id": 1,
        "organisation": "acme",
        "repository_id": 3,
        "changeset": format!("sha256:{}", "1".repeat(64)),
        "check": {
            "id": "web:unit", "folder": "web", "command": "true", "timeout": 60,
            "outputs": ["web/dist"], "caches": ["web/.cache"]
        },
        "heartbeat_interval": 5,
        "files": [{
            "path": path,
            "address": format!("sha256:{}", "2".repeat(64)),
            "executable": false,
            "download": {"url": "http://files", "headers": []}
        }]
    })
}

#[tokio::test]
async fn a_claimed_file_path_is_parsed_where_it_enters() {
    let (source, file) = source_claiming(Seen::default(), Some(claimed("a/b"))).await;
    let job = source.claim().await.unwrap().unwrap();
    assert_eq!(job.files[0].path.to_string(), "a/b");
    assert_eq!(
        (job.check.as_str(), job.folder.as_str()),
        ("web:unit", "web")
    );
    assert_eq!(job.outputs[0].to_string(), "web/dist");
    assert_eq!(job.caches[0].to_string(), "web/.cache");
    std::fs::remove_file(file).unwrap();
    for bad in ["../escape", "/abs", "", "a/../../b"] {
        let (source, file) = source_claiming(Seen::default(), Some(claimed(bad))).await;
        let answer = source.claim().await;
        assert!(matches!(answer, Err(SourceError::Malformed(_))), "{bad:?}");
        std::fs::remove_file(file).unwrap();
    }
}

/// A server that grants an upload of every address it is asked about except `held`, and keeps
/// the bodies PUT to the grants and the results reported. Returns its URL.
async fn storing(held: Address, puts: Puts, seen: Seen) -> String {
    let url = Arc::new(Mutex::new(String::new()));
    let grants_url = Arc::clone(&url);
    let app = Router::new()
        .route(
            "/v1/runner/jobs/{id}/uploads",
            post(move |Json(body): Json<Value>| {
                let url = grants_url.lock().unwrap().clone();
                async move {
                    let grants: Vec<Value> = body["files"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter(|file| file["address"] != json!(held.to_string()))
                        .map(|file| {
                            json!({
                                "address": file["address"],
                                "grant": {
                                    "url": format!("{url}/store/{}", file["address"].as_str().unwrap()),
                                    "headers": [{"name": "x-grant", "value": "yes"}]
                                }
                            })
                        })
                        .collect();
                    Json(json!({ "grants": grants }))
                }
            }),
        )
        .route(
            "/store/{address}",
            axum::routing::put(
                |State(puts): State<Puts>,
                 Path(address): Path<String>,
                 headers: axum::http::HeaderMap,
                 body: axum::body::Bytes| async move {
                    assert_eq!(headers["x-grant"], "yes");
                    puts.lock().unwrap().push((address, body.to_vec()));
                    StatusCode::OK
                },
            ),
        )
        .with_state(puts)
        .route(
            "/v1/runner/jobs/{id}/result",
            post(
                |State(seen): State<Seen>, Path(id): Path<u64>, Json(body): Json<Value>| async move {
                    seen.lock().unwrap().push((id, body));
                    StatusCode::NO_CONTENT
                },
            )
            .with_state(seen),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let at = format!("http://{}", listener.local_addr().unwrap());
    *url.lock().unwrap() = at.clone();
    tokio::spawn(async move { axum::serve(listener, app).await });
    at
}

fn produced_file(path: &str, bytes: &[u8]) -> ProducedFile {
    ProducedFile {
        path: path.parse().unwrap(),
        address: Address::new(sha2::Sha256::digest(bytes).into()),
        executable: false,
        bytes: bytes.to_vec(),
    }
}

#[tokio::test]
async fn a_report_uploads_the_files_the_server_lacks_and_lists_all_of_them() {
    let kept = produced_file("web/dist/a", b"alpha");
    let new = produced_file("web/.cache/b", b"beta");
    let puts = Arc::default();
    let seen = Seen::default();
    let url = storing(kept.address, Arc::clone(&puts), seen.clone()).await;
    let file = std::env::temp_dir().join(format!(
        "codehub-runner-http-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::write(&file, "token\n").unwrap();
    let source = HttpJobSource::new(
        reqwest::Client::new(),
        &url,
        TokenSource::File(file.clone()),
    );
    let report = Report {
        status: Status::Passed,
        exit_code: Some(0),
        output: String::new(),
        produced: Produced {
            outputs: vec![kept.clone()],
            caches: vec![new.clone()],
        },
    };
    source.report(JobId::new(4), &report).await.unwrap();
    assert_eq!(
        *puts.lock().unwrap(),
        [(new.address.to_string(), b"beta".to_vec())],
        "only the file the server lacks is uploaded"
    );
    let seen = seen.lock().unwrap().clone();
    let [(4, body)] = seen.as_slice() else {
        panic!("{seen:?}")
    };
    assert_eq!(body["outputs"][0]["path"], "web/dist/a");
    assert_eq!(body["caches"][0]["address"], json!(new.address.to_string()));
    std::fs::remove_file(file).unwrap();
}
