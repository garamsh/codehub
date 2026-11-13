//! The executor driver port: runs one job in an environment the driver builds and removes.

use std::fmt;
use std::future::Future;

use tokio::sync::watch;

use crate::job_source::{Job, StopReason};

/// How the job's command ended.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum End {
    /// The command exited with this code.
    Exited(i32),
    /// A signal ended the command.
    Signalled,
    /// The driver stopped the command because it was told to.
    Stopped(StopReason),
}

/// What running a job produced.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Execution {
    /// How the command ended.
    pub end: End,
    /// The end of the command's combined output.
    pub output: String,
}

/// Why a driver could not run the job's command.
#[derive(Debug)]
pub enum ExecutorError {
    /// The job's directory could not be made or removed.
    Directory(std::io::Error),
    /// A file could not be fetched or written.
    Download {
        /// The file's path.
        path: String,
        /// What went wrong.
        reason: String,
    },
    /// A file's bytes do not hash to its address.
    Mismatch {
        /// The file's path.
        path: String,
    },
    /// The command could not be started.
    Spawn(std::io::Error),
    /// The description of the job is larger than the orchestrator holds.
    DescriptionTooLarge {
        /// The description's size, with its key, in bytes.
        size: usize,
        /// The most the orchestrator holds, in bytes.
        limit: usize,
    },
    /// The orchestrator could not be reached or refused a request.
    Orchestrator(String),
    /// The job ended on the orchestrator without posting a result.
    NoResult {
        /// The phase it ended in.
        phase: String,
        /// The reason the orchestrator gave, where it gave one.
        reason: Option<String>,
    },
}

impl fmt::Display for ExecutorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Directory(error) => write!(f, "the job's directory failed: {error}"),
            Self::Download { path, reason } => write!(f, "fetching {path} failed: {reason}"),
            Self::Mismatch { path } => {
                write!(f, "the bytes of {path} do not match its address")
            }
            Self::Spawn(error) => write!(f, "starting the command failed: {error}"),
            Self::DescriptionTooLarge { size, limit } => write!(
                f,
                "the job's description is {size} bytes, over the {limit} the orchestrator holds"
            ),
            Self::Orchestrator(reason) => write!(f, "the orchestrator failed: {reason}"),
            Self::NoResult { phase, reason } => {
                write!(f, "the job ended {phase} without a result")?;
                match reason {
                    Some(reason) => write!(f, ": {reason}"),
                    None => Ok(()),
                }
            }
        }
    }
}

impl std::error::Error for ExecutorError {}

/// The sending half of a stop request.
pub struct StopSignal(watch::Sender<Option<StopReason>>);

impl StopSignal {
    /// Asks the executor to stop the job, for `reason`.
    pub fn send(&self, reason: StopReason) {
        self.0.send_replace(Some(reason));
    }
}

/// The receiving half of a stop request, which an executor watches while it runs a job.
pub struct Stop(watch::Receiver<Option<StopReason>>);

impl Stop {
    /// A signal and the request it sends.
    #[must_use]
    pub fn channel() -> (StopSignal, Self) {
        let (sender, receiver) = watch::channel(None);
        (StopSignal(sender), Self(receiver))
    }

    /// Waits until a stop is requested, and returns why. Where the signal is dropped without
    /// one, never returns.
    pub async fn requested(&mut self) -> StopReason {
        loop {
            if let Some(reason) = *self.0.borrow_and_update() {
                return reason;
            }
            if self.0.changed().await.is_err() {
                std::future::pending::<()>().await;
            }
        }
    }
}

/// Runs a job's command with the changeset's files present.
pub trait Executor {
    /// Builds the environment for `job`, runs its command, and removes the environment. Where
    /// `stop` requests one, ends the command and returns [`End::Stopped`].
    ///
    /// # Errors
    ///
    /// An [`ExecutorError`] where the environment could not be built or the command not started.
    fn run(
        &self,
        job: &Job,
        stop: Stop,
    ) -> impl Future<Output = Result<Execution, ExecutorError>> + Send;
}
