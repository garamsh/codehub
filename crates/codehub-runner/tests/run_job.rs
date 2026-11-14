//! `codehub-runner run-job` end to end: the built binary runs a job file against a local result
//! listener, and exits with the command's code.

use std::path::PathBuf;
use std::process::{Command, ExitStatus};
use std::time::Duration;

use codehub_runner::gaon::results::Results;
use codehub_runner::job_source::{Address, Job, JobId};
use codehub_runner::run_job::{Description, Posted, PostedEnd, PostedProduced};

/// A job of `id` that runs `command`.
#[cfg(test)]
fn job(id: u64, command: &str) -> Job {
    Job {
        id: JobId::new(id),
        organisation: "acme".into(),
        repository: 42,
        changeset: Address::new([9; 32]),
        check: "unit".into(),
        folder: String::new(),
        outputs: Vec::new(),
        caches: Vec::new(),
        command: command.into(),
        heartbeat_interval: Duration::from_secs(5),
        files: Vec::new(),
    }
}

/// Serves `results` on a local port and returns its base URL.
#[cfg(test)]
async fn listen(results: &Results) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = results.router();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{address}")
}

/// Writes `description` to a file of this test process and runs the binary on it, off the test's
/// runtime so that the listener keeps serving.
#[cfg(test)]
async fn run_job(name: &str, description: &Description) -> ExitStatus {
    let path: PathBuf =
        std::env::temp_dir().join(format!("run-job-e2e-{}-{name}", std::process::id()));
    std::fs::write(&path, serde_json::to_vec(description).unwrap()).unwrap();
    tokio::task::spawn_blocking(move || {
        Command::new(env!("CARGO_BIN_EXE_codehub-runner"))
            .arg("run-job")
            .arg("--job")
            .arg(path)
            .env_clear()
            .status()
            .unwrap()
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn the_command_runs_its_end_is_posted_and_its_code_is_the_exit_code() {
    let results = Results::default();
    let base = listen(&results).await;
    let mut expectation = results.expect(9101, "secret-9101");
    let description = Description::new(
        &job(9101, "echo from the job; exit 4"),
        format!("{base}/results/9101"),
        "secret-9101".into(),
    );
    let status = run_job("ends", &description).await;
    assert_eq!(status.code(), Some(4));
    assert_eq!(
        expectation.take(),
        Some(Posted {
            end: PostedEnd::Exited(4),
            output: "from the job\n".into(),
            produced: PostedProduced::default(),
        })
    );
}
