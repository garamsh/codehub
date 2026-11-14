//! Tests of the executor check: the host driver runs real processes, and a recording executor
//! shows the job the check builds.

use std::path::PathBuf;
use std::sync::Mutex;

use crate::host::HostExecutor;
use crate::job_source::Produced;
use crate::run_job::Description;

use super::*;

fn host() -> HostExecutor {
    HostExecutor::new(std::env::temp_dir(), reqwest::Client::new(), None)
}

const LONG: Duration = Duration::from_secs(60);

/// Records every job it is given and answers with `end`.
struct Recording {
    jobs: Mutex<Vec<Job>>,
    end: End,
}

impl Recording {
    fn new(end: End) -> Self {
        Self {
            jobs: Mutex::new(Vec::new()),
            end,
        }
    }

    fn jobs(&self) -> Vec<Job> {
        self.jobs.lock().unwrap().clone()
    }
}

impl Executor for Recording {
    async fn run(&self, job: &Job, _stop: Stop) -> Result<Execution, ExecutorError> {
        self.jobs.lock().unwrap().push(job.clone());
        Ok(Execution {
            end: self.end,
            output: String::new(),
            produced: Produced::default(),
        })
    }
}

#[tokio::test]
async fn a_command_that_exits_0_gives_code_0_and_its_output_after_the_line_naming_the_end() {
    let outcome = run(&host(), "echo x", LONG).await;
    assert_eq!(outcome.code, 0);
    assert_eq!(outcome.stdout, "exited with code 0\nx\n");
    assert_eq!(outcome.stderr, "");
}

#[tokio::test]
async fn a_command_that_exits_with_a_code_gives_that_code() {
    let outcome = run(&host(), "echo no; exit 7", LONG).await;
    assert_eq!(outcome.code, 7);
    assert_eq!(outcome.stdout, "exited with code 7\nno\n");
}

#[tokio::test]
async fn a_timeout_stops_the_command_and_names_the_stop_with_code_1() {
    let started = std::time::Instant::now();
    let outcome = run(&host(), "sleep 60", Duration::from_millis(200)).await;
    assert!(
        started.elapsed() < Duration::from_secs(30),
        "it was stopped"
    );
    assert_eq!(outcome.code, 1);
    assert_eq!(outcome.stdout, "stopped: timed out\n");
}

#[tokio::test]
async fn a_command_that_ends_before_the_timeout_is_not_stopped() {
    let outcome = run(&host(), "true", Duration::from_secs(30)).await;
    assert_eq!(outcome.stdout, "exited with code 0\n");
}

#[test]
fn an_end_that_is_not_a_code_from_0_to_255_gives_code_1_and_the_line_names_it() {
    let of = |end| {
        Outcome::of(Ok(Execution {
            end,
            output: String::new(),
            produced: Produced::default(),
        }))
    };
    let signalled = of(End::Signalled);
    assert_eq!(
        (signalled.code, signalled.stdout.as_str()),
        (1, "ended by a signal\n")
    );
    let cancelled = of(End::Stopped(StopReason::Cancelled));
    assert_eq!(
        (cancelled.code, cancelled.stdout.as_str()),
        (1, "stopped: cancelled\n")
    );
    assert_eq!(of(End::Exited(-1)).code, 1);
    assert_eq!(of(End::Exited(256)).code, 1);
    assert_eq!(of(End::Exited(255)).code, 255);
}

#[test]
fn an_executor_error_goes_to_standard_error_with_code_125() {
    let error = ExecutorError::Orchestrator("down".to_owned());
    let outcome = Outcome::of(Err(error));
    assert_eq!(outcome.code, 125);
    assert_eq!(outcome.stdout, "");
    assert_eq!(
        outcome.stderr,
        "check-executor: the orchestrator failed: down\n"
    );
}

#[tokio::test]
async fn a_host_directory_that_cannot_be_made_is_an_executor_error() {
    let blocker: PathBuf =
        std::env::temp_dir().join(format!("check-blocker-{}", std::process::id()));
    std::fs::write(&blocker, "a file, not a directory").unwrap();
    let executor = HostExecutor::new(blocker.join("under"), reqwest::Client::new(), None);
    let outcome = run(&executor, "true", LONG).await;
    assert_eq!(outcome.code, 125);
    assert!(outcome.stderr.contains("directory"), "{}", outcome.stderr);
}

#[tokio::test]
async fn the_job_has_no_files_the_given_command_and_a_changeset_run_job_accepts() {
    let executor = Recording::new(End::Exited(0));
    run(&executor, "make test", LONG).await;
    let jobs = executor.jobs();
    let [job] = jobs.as_slice() else {
        panic!("one job runs, got {}", jobs.len());
    };
    assert_eq!(job.command, "make test");
    assert!(job.files.is_empty());
    let description = Description::new(job, "http://runner/results/1".into(), "s".into());
    let accepted = description.job().unwrap();
    assert_eq!(accepted.changeset, job.changeset);
    assert_eq!(accepted.check, "check-executor");
}

#[tokio::test]
async fn each_check_draws_its_own_job_identifier() {
    let executor = Recording::new(End::Exited(0));
    for _ in 0..4 {
        run(&executor, "true", LONG).await;
    }
    let mut ids: Vec<u64> = executor.jobs().iter().map(|job| job.id.value()).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), 4, "four checks, four identifiers");
}
