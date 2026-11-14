//! The executor check: runs one job with no files and a given command through an executor, with
//! no server. ADR 0047 decides it.

use std::process::ExitCode;
use std::time::Duration;

use rustix::rand::{GetRandomFlags, getrandom};

use crate::executor::{End, Execution, Executor, ExecutorError, Stop};
use crate::job_source::{Address, Job, JobId, StopReason};

/// How long the job runs before it is stopped as timed out, in seconds, where none is given.
pub const DEFAULT_TIMEOUT_SECONDS: u64 = 600;

/// The exit code of an executor error, which no end of the command gives.
const EXECUTOR_ERROR_CODE: u8 = 125;

/// The changeset address the job names: 32 bytes of 0x01, a valid `sha256:` address.
const CHANGESET: Address = Address::new([1; 32]);

/// What the check prints and exits with.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Outcome {
    /// What goes to standard output: the line naming the end, then the output tail.
    pub stdout: String,
    /// What goes to standard error.
    pub stderr: String,
    /// The process's exit code.
    pub code: u8,
}

impl Outcome {
    /// The outcome of `result`: the command's code where it exited with one from 0 to 255, 1 for
    /// any other end, and 125 for an executor error.
    #[must_use]
    pub fn of(result: Result<Execution, ExecutorError>) -> Self {
        match result {
            Ok(Execution { end, output, .. }) => {
                let (line, code) = match end {
                    End::Exited(code) => (
                        format!("exited with code {code}"),
                        u8::try_from(code).unwrap_or(1),
                    ),
                    End::Signalled => ("ended by a signal".to_owned(), 1),
                    End::Stopped(StopReason::TimedOut) => ("stopped: timed out".to_owned(), 1),
                    End::Stopped(StopReason::Cancelled) => ("stopped: cancelled".to_owned(), 1),
                };
                Self {
                    stdout: format!("{line}\n{output}"),
                    stderr: String::new(),
                    code,
                }
            }
            Err(error) => Self {
                stdout: String::new(),
                stderr: format!("check-executor: {error}\n"),
                code: EXECUTOR_ERROR_CODE,
            },
        }
    }

    /// Prints the outcome and returns the exit code.
    #[must_use]
    pub fn finish(self) -> ExitCode {
        print!("{}", self.stdout);
        eprint!("{}", self.stderr);
        ExitCode::from(self.code)
    }
}

/// Runs one job with no files and `command` through `executor`, and stops it as timed out after
/// `timeout`.
pub async fn run<E: Executor>(executor: &E, command: &str, timeout: Duration) -> Outcome {
    Outcome::of(execute(executor, command, timeout).await)
}

async fn execute<E: Executor>(
    executor: &E,
    command: &str,
    timeout: Duration,
) -> Result<Execution, ExecutorError> {
    let job = Job {
        id: random_id()?,
        organisation: "check-executor".to_owned(),
        repository: 0,
        changeset: CHANGESET,
        check: "check-executor".to_owned(),
        folder: String::new(),
        outputs: Vec::new(),
        caches: Vec::new(),
        command: command.to_owned(),
        heartbeat_interval: Duration::ZERO,
        files: Vec::new(),
    };
    let (signal, stop) = Stop::channel();
    let running = executor.run(&job, stop);
    tokio::pin!(running);
    tokio::select! {
        result = &mut running => result,
        () = tokio::time::sleep(timeout) => {
            signal.send(StopReason::TimedOut);
            running.await
        }
    }
}

/// A job identifier of 8 random bytes, so that two checks share no object name.
fn random_id() -> Result<JobId, ExecutorError> {
    let mut bytes = [0_u8; 8];
    let mut filled = 0;
    while filled < bytes.len() {
        let rest = bytes.get_mut(filled..).unwrap_or_default();
        filled += getrandom(rest, GetRandomFlags::empty())
            .map_err(|error| ExecutorError::Orchestrator(format!("no random bytes: {error}")))?;
    }
    Ok(JobId::new(u64::from_le_bytes(bytes)))
}

#[cfg(test)]
mod tests;
