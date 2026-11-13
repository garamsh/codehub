//! Tests of the single-job mode: it runs real commands and posts to a stub listener.

use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::routing::post as post_route;

use crate::job_source::JobId;

use super::*;

type Seen = Arc<Mutex<Vec<(String, Posted)>>>;

async fn listen() -> (String, Seen) {
    let seen = Seen::default();
    let app = Router::new()
        .route(
            "/results/{id}",
            post_route(
                |State(seen): State<Seen>, headers: HeaderMap, body: axum::body::Bytes| async move {
                    let bearer = headers
                        .get("authorization")
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_owned();
                    seen.lock()
                        .unwrap()
                        .push((bearer, serde_json::from_slice(&body).unwrap()));
                },
            ),
        )
        .with_state(seen.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{address}"), seen)
}

/// A job of its own `id`: the host driver names a job's directory by it, so tests that run at
/// once need ids that differ.
fn job(id: u64, command: &str) -> Job {
    Job {
        id: JobId::new(id),
        organisation: "acme".into(),
        repository: 42,
        changeset: Address::new([9; 32]),
        check: "unit".into(),
        command: command.into(),
        heartbeat_interval: Duration::from_secs(5),
        files: Vec::new(),
    }
}

fn write(name: &str, description: &Description) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("run-job-test-{}-{name}", std::process::id()));
    std::fs::write(&path, serde_json::to_vec(description).unwrap()).unwrap();
    path
}

#[test]
fn a_description_gives_back_the_job_it_was_made_from() {
    let mut original = job(11, "make test");
    original.files.push(JobFile {
        path: "a/b.sh".parse().unwrap(),
        address: Address::new([4; 32]),
        executable: true,
        download: Download {
            url: "http://objects/x".into(),
            headers: vec![("x-grant".into(), "ok".into())],
        },
    });
    let text = serde_json::to_vec(&Description::new(&original, "u".into(), "s".into())).unwrap();
    let back: Description = serde_json::from_slice(&text).unwrap();
    let job = back.job().unwrap();
    assert_eq!(job.id, original.id);
    assert_eq!(job.command, "make test");
    assert_eq!(job.changeset, original.changeset);
    let file = &job.files[0];
    assert_eq!(file.path.to_string(), "a/b.sh");
    assert_eq!(file.address, Address::new([4; 32]));
    assert!(file.executable);
    assert_eq!(
        file.download.headers,
        [("x-grant".to_owned(), "ok".to_owned())]
    );
}

#[tokio::test]
async fn the_end_and_output_of_the_command_are_posted_with_the_result_secret() {
    let (base, seen) = listen().await;
    let url = format!("{base}/results/12");
    let description = Description::new(&job(12, "echo hello; exit 3"), url, "the-secret".into());
    run_within(&write("posts", &description), Duration::from_secs(5))
        .await
        .unwrap();
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].0, "Bearer the-secret");
    assert_eq!(seen[0].1.end, PostedEnd::Exited(3), "{}", seen[0].1.output);
    assert_eq!(seen[0].1.output, "hello\n");
}

#[tokio::test]
async fn a_result_that_cannot_be_posted_is_a_failure() {
    let description = Description::new(&job(13, "true"), "http://127.0.0.1:1/x".into(), "s".into());
    let error = run_within(&write("unposted", &description), Duration::from_millis(100))
        .await
        .unwrap_err();
    assert!(error.contains("posting the result failed"), "{error}");
}

/// A listener that answers every post with `status`, and the URL to post to.
async fn answering(status: u16) -> String {
    let status = axum::http::StatusCode::from_u16(status).unwrap();
    let app = Router::new().route("/results/{id}", post_route(move || async move { status }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{address}/results/14")
}

#[tokio::test]
async fn a_conflict_from_the_listener_is_a_posted_result_and_an_unauthorised_is_not() {
    let window = Duration::from_millis(100);
    let conflict = Description::new(&job(14, "true"), answering(409).await, "s".into());
    run_within(&write("conflict", &conflict), window)
        .await
        .unwrap();
    let refused = Description::new(&job(15, "true"), answering(401).await, "s".into());
    let error = run_within(&write("refused", &refused), window)
        .await
        .unwrap_err();
    assert!(error.contains("401"), "{error}");
}

#[tokio::test]
async fn a_file_that_is_not_a_description_is_a_failure() {
    let path = std::env::temp_dir().join(format!("run-job-test-{}-garbage", std::process::id()));
    std::fs::write(&path, "not json").unwrap();
    let error = run_within(&path, Duration::from_millis(100))
        .await
        .unwrap_err();
    assert!(error.contains("not a job description"), "{error}");
}
