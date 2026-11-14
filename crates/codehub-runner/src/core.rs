//! The runner's loop: claim a job, run it while heartbeating, report how it ended.

use std::convert::Infallible;
use std::time::Duration;

use tokio::time::{MissedTickBehavior, interval_at};

use crate::executor::{End, Execution, Executor, ExecutorError, Stop, StopSignal};
use crate::job_source::{
    Heartbeat, Job, JobSource, Produced, Report, SourceError, Status, StopReason,
};

/// How long the runner waits after a failed claim before it claims again.
pub const RETRY_PAUSE: Duration = Duration::from_secs(5);

/// The shortest time between two heartbeats, whatever the server asks.
const MIN_HEARTBEAT: Duration = Duration::from_secs(1);

/// How many jobs the runner takes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// One claim: run the job it returns, if any, and stop.
    Once,
    /// Claim for as long as the process lives.
    Forever,
}

/// Claims and runs jobs as `mode` says.
///
/// # Errors
///
/// A [`SourceError`] where a claim failed in [`Mode::Once`]. In [`Mode::Forever`] a failed claim
/// is logged and tried again after [`RETRY_PAUSE`], and the function does not return.
pub async fn run<S: JobSource, E: Executor>(
    source: &S,
    executor: &E,
    mode: Mode,
) -> Result<(), SourceError> {
    loop {
        match source.claim().await {
            Ok(Some(job)) => run_job(source, executor, &job).await,
            Ok(None) => {}
            Err(error) if mode == Mode::Once => return Err(error),
            Err(error) => {
                eprintln!("claim failed: {error}");
                tokio::time::sleep(RETRY_PAUSE).await;
            }
        }
        if mode == Mode::Once {
            return Ok(());
        }
    }
}

async fn run_job<S: JobSource, E: Executor>(source: &S, executor: &E, job: &Job) {
    eprintln!(
        "job {} started: {} check {} of {} in repository {}",
        job.id, job.check, job.changeset, job.organisation, job.repository
    );
    let (signal, stop) = Stop::channel();
    let execution = executor.run(job, stop);
    tokio::pin!(execution);
    let result = tokio::select! {
        result = &mut execution => result,
        never = keep_alive(source, job, &signal) => match never {},
    };
    let report = report_of(result);
    eprintln!("job {} ended: {:?}", job.id, report.status);
    if let Err(error) = source.report(job.id, &report).await {
        eprintln!("job {}: reporting failed: {error}", job.id);
    }
}

/// Heartbeats for as long as it is polled, and passes on a stop the server asks for.
async fn keep_alive<S: JobSource>(source: &S, job: &Job, signal: &StopSignal) -> Infallible {
    let every = job.heartbeat_interval.max(MIN_HEARTBEAT);
    let mut ticks = interval_at(tokio::time::Instant::now() + every, every);
    ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        ticks.tick().await;
        match source.heartbeat(job.id).await {
            Ok(Heartbeat::Continue) => {}
            Ok(Heartbeat::Stop(reason)) => signal.send(reason),
            Err(SourceError::NotHeld) => {
                eprintln!("job {}: the server no longer holds it; stopping", job.id);
                signal.send(StopReason::Cancelled);
                std::future::pending::<Infallible>().await;
            }
            Err(error) => eprintln!("job {}: heartbeat failed: {error}", job.id),
        }
    }
}

fn report_of(result: Result<Execution, ExecutorError>) -> Report {
    match result {
        Ok(Execution {
            end,
            output,
            produced,
        }) => {
            let (status, exit_code) = match end {
                End::Exited(0) => (Status::Passed, Some(0)),
                End::Exited(code) => (Status::Failed, Some(code)),
                End::Signalled => (Status::Failed, None),
                End::Stopped(StopReason::TimedOut) => (Status::TimedOut, None),
                End::Stopped(StopReason::Cancelled) => (Status::Cancelled, None),
            };
            Report {
                status,
                exit_code,
                output,
                produced: if status == Status::Passed {
                    produced
                } else {
                    Produced::default()
                },
            }
        }
        Err(error) => Report {
            status: Status::Failed,
            exit_code: None,
            output: error.to_string(),
            produced: Produced::default(),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use crate::job_source::{Address, JobId};

    use super::*;

    fn job() -> Job {
        Job {
            id: JobId::new(7),
            organisation: "acme".into(),
            repository: 3,
            changeset: Address::new([1; 32]),
            check: "unit".into(),
            folder: String::new(),
            outputs: Vec::new(),
            caches: Vec::new(),
            command: "true".into(),
            heartbeat_interval: Duration::from_secs(5),
            files: Vec::new(),
        }
    }

    #[derive(Default)]
    struct Source {
        claims: Mutex<VecDeque<Result<Option<Job>, SourceError>>>,
        answers: Mutex<VecDeque<Result<Heartbeat, SourceError>>>,
        heartbeats: Mutex<u32>,
        reports: Mutex<Vec<(JobId, Report)>>,
    }

    impl Source {
        fn with_job() -> Self {
            let source = Self::default();
            source.claims.lock().unwrap().push_back(Ok(Some(job())));
            source
        }

        fn reported(&self) -> Vec<Report> {
            self.reports
                .lock()
                .unwrap()
                .iter()
                .map(|(_, report)| report.clone())
                .collect()
        }
    }

    impl JobSource for Source {
        async fn claim(&self) -> Result<Option<Job>, SourceError> {
            let next = self.claims.lock().unwrap().pop_front();
            match next {
                Some(answer) => answer,
                // A server holds a claim it has no job for.
                None => std::future::pending().await,
            }
        }

        async fn heartbeat(&self, _: JobId) -> Result<Heartbeat, SourceError> {
            *self.heartbeats.lock().unwrap() += 1;
            self.answers
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Ok(Heartbeat::Continue))
        }

        async fn report(&self, job: JobId, report: &Report) -> Result<(), SourceError> {
            self.reports.lock().unwrap().push((job, report.clone()));
            Ok(())
        }
    }

    /// Ends after `after` with `end`, or when asked to stop.
    struct Executes {
        after: Duration,
        end: Result<End, String>,
    }

    impl Executor for Executes {
        async fn run(&self, _: &Job, mut stop: Stop) -> Result<Execution, ExecutorError> {
            tokio::select! {
                reason = stop.requested() => Ok(Execution {
                    end: End::Stopped(reason),
                    output: "stopped".into(),
                    produced: Produced::default(),
                }),
                () = tokio::time::sleep(self.after) => match &self.end {
                    Ok(end) => Ok(Execution {
                        end: *end,
                        output: "out".into(),
                        produced: Produced::default(),
                    }),
                    Err(text) => Err(ExecutorError::Mismatch { path: text.clone() }),
                },
            }
        }
    }

    fn quick(end: End) -> Executes {
        Executes {
            after: Duration::from_secs(1),
            end: Ok(end),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_command_that_exits_zero_is_reported_passed_with_its_output() {
        let source = Source::with_job();
        run(&source, &quick(End::Exited(0)), Mode::Once)
            .await
            .unwrap();
        assert_eq!(
            source.reported(),
            [Report {
                status: Status::Passed,
                exit_code: Some(0),
                output: "out".into(),
                produced: Produced::default(),
            }]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_command_that_exits_otherwise_is_reported_failed_with_its_code() {
        let source = Source::with_job();
        run(&source, &quick(End::Exited(3)), Mode::Once)
            .await
            .unwrap();
        let [report] = source.reported().try_into().unwrap();
        assert_eq!((report.status, report.exit_code), (Status::Failed, Some(3)));
    }

    #[tokio::test(start_paused = true)]
    async fn a_command_ended_by_a_signal_is_reported_failed_without_a_code() {
        let source = Source::with_job();
        run(&source, &quick(End::Signalled), Mode::Once)
            .await
            .unwrap();
        let [report] = source.reported().try_into().unwrap();
        assert_eq!((report.status, report.exit_code), (Status::Failed, None));
    }

    #[tokio::test(start_paused = true)]
    async fn a_driver_error_is_reported_failed_with_its_text() {
        let source = Source::with_job();
        let executor = Executes {
            after: Duration::from_secs(1),
            end: Err("bad".into()),
        };
        run(&source, &executor, Mode::Once).await.unwrap();
        let [report] = source.reported().try_into().unwrap();
        assert_eq!(report.status, Status::Failed);
        assert!(report.output.contains("bad"), "{}", report.output);
    }

    #[tokio::test(start_paused = true)]
    async fn heartbeats_are_no_closer_than_the_minimum_whatever_the_server_asks() {
        let source = Source::default();
        let mut asked = job();
        asked.heartbeat_interval = Duration::from_millis(10);
        source.claims.lock().unwrap().push_back(Ok(Some(asked)));
        let executor = Executes {
            after: Duration::from_millis(2500),
            end: Ok(End::Exited(0)),
        };
        run(&source, &executor, Mode::Once).await.unwrap();
        assert_eq!(*source.heartbeats.lock().unwrap(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn the_job_is_heartbeated_while_it_runs() {
        let source = Source::with_job();
        let executor = Executes {
            after: Duration::from_secs(12),
            end: Ok(End::Exited(0)),
        };
        run(&source, &executor, Mode::Once).await.unwrap();
        assert_eq!(*source.heartbeats.lock().unwrap(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn a_stop_from_the_server_stops_the_command_and_is_reported_by_reason() {
        for (reason, status) in [
            (StopReason::TimedOut, Status::TimedOut),
            (StopReason::Cancelled, Status::Cancelled),
        ] {
            let source = Source::with_job();
            source
                .answers
                .lock()
                .unwrap()
                .push_back(Ok(Heartbeat::Stop(reason)));
            let executor = Executes {
                after: Duration::from_secs(3600),
                end: Ok(End::Exited(0)),
            };
            run(&source, &executor, Mode::Once).await.unwrap();
            let [report] = source.reported().try_into().unwrap();
            assert_eq!((report.status, report.exit_code), (status, None));
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_job_the_server_no_longer_holds_is_stopped() {
        let source = Source::with_job();
        source
            .answers
            .lock()
            .unwrap()
            .push_back(Err(SourceError::NotHeld));
        let executor = Executes {
            after: Duration::from_secs(3600),
            end: Ok(End::Exited(0)),
        };
        run(&source, &executor, Mode::Once).await.unwrap();
        let [report] = source.reported().try_into().unwrap();
        assert_eq!(report.status, Status::Cancelled);
    }

    #[tokio::test(start_paused = true)]
    async fn once_ends_after_a_claim_that_returned_nothing() {
        let source = Source::default();
        source.claims.lock().unwrap().push_back(Ok(None));
        run(&source, &quick(End::Exited(0)), Mode::Once)
            .await
            .unwrap();
        assert!(source.reported().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn once_returns_a_failed_claim() {
        let source = Source::default();
        source
            .claims
            .lock()
            .unwrap()
            .push_back(Err(SourceError::Status(500)));
        let result = run(&source, &quick(End::Exited(0)), Mode::Once).await;
        assert_eq!(result, Err(SourceError::Status(500)));
    }

    #[tokio::test(start_paused = true)]
    async fn forever_pauses_after_a_failed_claim_and_claims_again() {
        let source = Source::with_job();
        source
            .claims
            .lock()
            .unwrap()
            .push_front(Err(SourceError::Token("none".into())));
        let started = tokio::time::Instant::now();
        let executor = quick(End::Exited(0));
        let forever = run(&source, &executor, Mode::Forever);
        let _ = tokio::time::timeout(Duration::from_secs(30), forever).await;
        assert_eq!(source.reported().len(), 1);
        assert!(started.elapsed() >= RETRY_PAUSE);
    }
}
