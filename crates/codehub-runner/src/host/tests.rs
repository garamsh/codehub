//! Tests of the host executor: they start real processes and a stub file server, and need no
//! codehub server.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use axum::Router;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::get;

use crate::job_source::{Download, JobId, StopReason};

use super::*;

static NEXT: AtomicU64 = AtomicU64::new(1);

/// A new empty directory to work in.
fn scratch() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "runner-host-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn address_of(bytes: &[u8]) -> Address {
    Address::new(Sha256::digest(bytes).into())
}

fn job(command: &str) -> Job {
    Job {
        id: JobId::new(1),
        organisation: "acme".into(),
        repository: 42,
        changeset: Address::new([9; 32]),
        check: "unit".into(),
        command: command.into(),
        heartbeat_interval: Duration::from_secs(5),
        files: Vec::new(),
    }
}

fn executor(work: &Path) -> HostExecutor {
    HostExecutor::new(work.to_owned(), reqwest::Client::new(), None)
        .with_grace(Duration::from_millis(300))
}

async fn run(executor: &HostExecutor, job: &Job) -> Result<Execution, ExecutorError> {
    let (_signal, stop) = Stop::channel();
    executor.run(job, stop).await
}

/// Serves `/file` with `bytes` to a request that carries the header `x-grant: ok`.
async fn serve(bytes: &'static [u8]) -> String {
    let app = Router::new().route(
        "/file",
        get(move |headers: HeaderMap| async move {
            if headers.get("x-grant").map(|value| value.as_bytes()) == Some(b"ok") {
                (StatusCode::OK, bytes.to_vec())
            } else {
                (StatusCode::FORBIDDEN, Vec::new())
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await });
    format!("http://{address}/file")
}

fn file(path: &str, address_bytes: &[u8], url: &str, executable: bool) -> JobFile {
    JobFile {
        path: path.into(),
        address: address_of(address_bytes),
        executable,
        download: Download {
            url: url.into(),
            headers: vec![("x-grant".into(), "ok".into())],
        },
    }
}

/// Whether the process is not running: absent, or a zombie waiting to be reaped.
fn is_gone(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).map_or(true, |stat| {
        stat.rsplit(')')
            .next()
            .is_some_and(|rest| rest.trim_start().starts_with('Z'))
    })
}

async fn wait_gone(pid: u32) -> bool {
    let until = Instant::now() + Duration::from_secs(5);
    while Instant::now() < until {
        if is_gone(pid) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    false
}

/// Waits for `pidfile` to hold a pid and returns it.
async fn pid_in(pidfile: &Path) -> u32 {
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(text) = std::fs::read_to_string(pidfile)
            && let Ok(pid) = text.trim().parse()
        {
            return pid;
        }
        assert!(Instant::now() < until, "no pid in {}", pidfile.display());
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn the_command_runs_in_a_fresh_directory_with_the_job_in_its_environment() {
    let work = scratch();
    let job = job(
        "pwd; echo $CODEHUB_ORGANISATION $CODEHUB_REPOSITORY $CODEHUB_CHANGESET $CODEHUB_CHECK; \
         echo oops >&2",
    );
    let execution = run(&executor(&work), &job).await.unwrap();
    assert_eq!(execution.end, End::Exited(0));
    let lines: Vec<&str> = execution.output.lines().collect();
    assert!(
        lines
            .first()
            .is_some_and(|dir| dir.contains("codehub-job-")),
        "{lines:?}"
    );
    let expected = format!("acme 42 {} unit", job.changeset);
    assert_eq!(lines.get(1).copied(), Some(expected.as_str()));
    assert_eq!(lines.get(2).copied(), Some("oops"));
    assert_eq!(
        std::fs::read_dir(&work).unwrap().count(),
        0,
        "the directory is removed"
    );
    std::fs::remove_dir_all(&work).unwrap();
}

#[tokio::test]
async fn the_exit_code_is_returned() {
    let work = scratch();
    let execution = run(&executor(&work), &job("echo no; exit 3"))
        .await
        .unwrap();
    assert_eq!(execution.end, End::Exited(3));
    assert_eq!(execution.output, "no\n");
    std::fs::remove_dir_all(&work).unwrap();
}

#[tokio::test]
async fn a_command_killed_by_a_signal_has_no_exit_code() {
    let work = scratch();
    let execution = run(&executor(&work), &job("kill -9 $$")).await.unwrap();
    assert_eq!(execution.end, End::Signalled);
    std::fs::remove_dir_all(&work).unwrap();
}

#[tokio::test]
async fn files_are_downloaded_verified_and_given_their_modes() {
    let work = scratch();
    let bytes: &'static [u8] = b"#!/bin/sh\necho ran\n";
    let url = serve(bytes).await;
    let mut job = job("./bin/tool; stat -c %a bin/tool data.txt");
    job.files = vec![
        file("bin/tool", bytes, &url, true),
        file("data.txt", bytes, &url, false),
    ];
    let execution = run(&executor(&work), &job).await.unwrap();
    assert_eq!(execution.end, End::Exited(0));
    assert_eq!(execution.output, "ran\n755\n644\n");
    std::fs::remove_dir_all(&work).unwrap();
}

#[tokio::test]
async fn a_file_whose_bytes_do_not_match_its_address_fails_the_job_and_runs_no_command() {
    let work = scratch();
    let marker = work.join("ran");
    let url = serve(b"the real bytes").await;
    let mut job = job(&format!("touch {}", marker.display()));
    job.files = vec![file("lib/x.sh", b"other bytes", &url, false)];
    let error = run(&executor(&work), &job).await.unwrap_err();
    assert!(
        matches!(&error, ExecutorError::Mismatch { path } if path == "lib/x.sh"),
        "{error}"
    );
    assert!(error.to_string().contains("lib/x.sh"));
    assert!(!marker.exists(), "the command ran");
    assert_eq!(std::fs::read_dir(&work).unwrap().count(), 0);
    std::fs::remove_dir_all(&work).unwrap();
}

#[tokio::test]
async fn a_file_path_outside_the_directory_is_refused() {
    let work = scratch();
    let url = serve(b"x").await;
    for path in ["../escape", "/abs", "", "a/../../b"] {
        let mut job = job("true");
        job.files = vec![file(path, b"x", &url, false)];
        let error = run(&executor(&work), &job).await.unwrap_err();
        assert!(matches!(error, ExecutorError::Path(_)), "{path:?}: {error}");
    }
    std::fs::remove_dir_all(&work).unwrap();
}

#[tokio::test]
async fn a_download_the_server_refuses_fails_the_job() {
    let work = scratch();
    let url = serve(b"x").await;
    let mut job = job("true");
    let mut refused = file("x", b"x", &url, false);
    refused.download.headers.clear();
    job.files = vec![refused];
    let error = run(&executor(&work), &job).await.unwrap_err();
    assert!(matches!(error, ExecutorError::Download { .. }), "{error}");
    std::fs::remove_dir_all(&work).unwrap();
}

#[tokio::test]
async fn a_stop_ends_the_command_and_the_process_group() {
    let work = scratch();
    let pidfile = work.join("child.pid");
    let job = job(&format!(
        "sleep 300 & echo $! > {}; wait",
        pidfile.display()
    ));
    let executor = executor(&work);
    let (signal, stop) = Stop::channel();
    let running = tokio::spawn({
        let job = job.clone();
        async move { executor.run(&job, stop).await }
    });
    let pid = pid_in(&pidfile).await;
    assert!(!is_gone(pid), "the child is running before the stop");
    signal.send(StopReason::TimedOut);
    let execution = running.await.unwrap().unwrap();
    assert_eq!(execution.end, End::Stopped(StopReason::TimedOut));
    assert!(
        wait_gone(pid).await,
        "the child sleep {pid} outlived the stop"
    );
    std::fs::remove_dir_all(&work).unwrap();
}

#[tokio::test]
async fn a_command_that_ignores_the_request_is_killed_after_the_grace() {
    let work = scratch();
    let started = work.join("started");
    let job = job(&format!(
        "trap '' TERM; touch {}; while true; do sleep 1; done",
        started.display()
    ));
    let executor = executor(&work);
    let (signal, stop) = Stop::channel();
    let running = tokio::spawn({
        let job = job.clone();
        async move { executor.run(&job, stop).await }
    });
    let until = Instant::now() + Duration::from_secs(5);
    while !started.exists() {
        assert!(Instant::now() < until, "the command never started");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let asked = Instant::now();
    signal.send(StopReason::Cancelled);
    let execution = running.await.unwrap().unwrap();
    assert_eq!(execution.end, End::Stopped(StopReason::Cancelled));
    assert!(
        asked.elapsed() >= Duration::from_millis(250),
        "{:?}",
        asked.elapsed()
    );
    std::fs::remove_dir_all(&work).unwrap();
}

#[tokio::test]
async fn a_process_left_running_does_not_outlive_the_job() {
    let work = scratch();
    let pidfile = work.join("left.pid");
    let job = job(&format!("sleep 300 & echo $! > {}", pidfile.display()));
    let execution = run(&executor(&work), &job).await.unwrap();
    assert_eq!(execution.end, End::Exited(0));
    let pid = pid_in(&pidfile).await;
    assert!(wait_gone(pid).await, "sleep {pid} outlived its job");
    std::fs::remove_dir_all(&work).unwrap();
}

#[tokio::test]
async fn only_the_end_of_the_output_is_kept() {
    let work = scratch();
    let execution = run(
        &executor(&work),
        &job("head -c 200000 /dev/zero | tr '\\0' a; echo; echo the-end"),
    )
    .await
    .unwrap();
    assert!(execution.output.len() <= OUTPUT_LIMIT);
    assert!(execution.output.ends_with("aaa\nthe-end\n"));
    std::fs::remove_dir_all(&work).unwrap();
}

#[test]
fn the_kept_output_is_cut_at_a_character_boundary() {
    let mut tail = Tail::default();
    tail.push(b"a");
    tail.push("é".repeat(OUTPUT_LIMIT).as_bytes());
    let kept = tail.text();
    assert!(kept.len() <= OUTPUT_LIMIT);
    assert!(kept.len() >= OUTPUT_LIMIT - 2);
    assert!(kept.chars().all(|c| c == 'é'), "a character was cut");
}

#[tokio::test]
async fn the_hidden_variable_is_not_passed_to_the_command() {
    let work = scratch();
    let shown = run(&executor(&work), &job("echo \"[$HOME]\""))
        .await
        .unwrap();
    assert_ne!(shown.output, "[]\n", "the control variable is set");
    let hidden = HostExecutor::new(work.clone(), reqwest::Client::new(), Some("HOME".into()));
    let shown = run(&hidden, &job("echo \"[$HOME]\"")).await.unwrap();
    assert_eq!(shown.output, "[]\n");
    std::fs::remove_dir_all(&work).unwrap();
}
