//! The single-job mode: runs the job a file describes as the host driver does, and posts how it
//! ended to the runner that wrote the file. The gaon driver runs it inside a MicroVM.

use std::fmt;
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Serialize};
use sha2::Digest as _;
use tokio::time::Instant;

use crate::executor::{End, Execution, Executor, Stop};
use crate::host::HostExecutor;
use crate::job_source::{
    Address, Download, Job, JobFile, JobId, Produced, ProducedFile, RelativePath,
};

/// How long a result that is not accepted is posted again.
pub const POST_WINDOW: Duration = Duration::from_secs(30);

/// How long to wait before posting again.
const POST_PAUSE: Duration = Duration::from_secs(1);

/// How long one post may take.
const POST_TIMEOUT: Duration = Duration::from_secs(10);

/// How a job's command ended, as `run-job` posts it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, Debug)]
#[serde(rename_all = "snake_case")]
pub enum PostedEnd {
    /// The command exited with this code.
    Exited(i32),
    /// A signal ended the command, or it could not run.
    Signalled,
}

/// What `run-job` posts to the result URL.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, Debug)]
pub struct Posted {
    /// How the command ended.
    pub end: PostedEnd,
    /// The end of its combined output, or what stopped it from running.
    pub output: String,
    /// The files it left under the check's outputs and caches, which the runner uploads.
    #[serde(default)]
    pub produced: PostedProduced,
}

/// The files a command left, as `run-job` posts them.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize, Debug)]
pub struct PostedProduced {
    outputs: Vec<PostedFile>,
    caches: Vec<PostedFile>,
}

/// A file a command left, with its bytes in base64.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, Debug)]
struct PostedFile {
    path: String,
    executable: bool,
    bytes: String,
}

impl PostedProduced {
    fn of(produced: &Produced) -> Self {
        let posted = |files: &[ProducedFile]| {
            files
                .iter()
                .map(|file| PostedFile {
                    path: file.path.to_string(),
                    executable: file.executable,
                    bytes: STANDARD.encode(&file.bytes),
                })
                .collect()
        };
        Self {
            outputs: posted(&produced.outputs),
            caches: posted(&produced.caches),
        }
    }

    /// The files posted, addressed by their bytes; `None` where a path or the base64 is bad.
    fn produced(&self) -> Option<Produced> {
        let read = |files: &[PostedFile]| {
            files
                .iter()
                .map(|file| {
                    let bytes = STANDARD.decode(&file.bytes).ok()?;
                    Some(ProducedFile {
                        path: file.path.parse::<RelativePath>().ok()?,
                        address: Address::new(sha2::Sha256::digest(&bytes).into()),
                        executable: file.executable,
                        bytes,
                    })
                })
                .collect::<Option<Vec<_>>>()
        };
        Some(Produced {
            outputs: read(&self.outputs)?,
            caches: read(&self.caches)?,
        })
    }
}

impl From<Posted> for Execution {
    fn from(posted: Posted) -> Self {
        let produced = posted.produced.produced();
        let (end, output) = match (posted.end, produced.is_some()) {
            (PostedEnd::Exited(code), true) => (End::Exited(code), posted.output),
            (PostedEnd::Exited(_), false) => (
                End::Signalled,
                format!(
                    "{}\ncodehub: the files the job left could not be read\n",
                    posted.output
                ),
            ),
            (PostedEnd::Signalled, _) => (End::Signalled, posted.output),
        };
        Self {
            end,
            output,
            produced: produced.unwrap_or_default(),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct DescribedFile {
    path: String,
    address: String,
    executable: bool,
    url: String,
    headers: Vec<(String, String)>,
}

/// The job a MicroVM runs, with where to post its end and the secret that authorises the post. It
/// is the content of the file the gaon driver hands the MicroVM, so it holds a secret and has no
/// `Debug`.
#[derive(Serialize, Deserialize)]
pub struct Description {
    id: u64,
    organisation: String,
    repository: u64,
    changeset: String,
    check: String,
    folder: String,
    outputs: Vec<String>,
    caches: Vec<String>,
    command: String,
    files: Vec<DescribedFile>,
    result_url: String,
    result_secret: String,
}

/// Why a description is not a job.
#[derive(Debug)]
pub struct DescriptionError(String);

impl fmt::Display for DescriptionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for DescriptionError {}

impl Description {
    /// The description of `job` that posts to `result_url` with `result_secret`.
    #[must_use]
    pub fn new(job: &Job, result_url: String, result_secret: String) -> Self {
        Self {
            id: job.id.value(),
            organisation: job.organisation.clone(),
            repository: job.repository,
            changeset: job.changeset.to_string(),
            check: job.check.clone(),
            folder: job.folder.clone(),
            outputs: job.outputs.iter().map(ToString::to_string).collect(),
            caches: job.caches.iter().map(ToString::to_string).collect(),
            command: job.command.clone(),
            files: job
                .files
                .iter()
                .map(|file| DescribedFile {
                    path: file.path.to_string(),
                    address: file.address.to_string(),
                    executable: file.executable,
                    url: file.download.url.clone(),
                    headers: file.download.headers.clone(),
                })
                .collect(),
            result_url,
            result_secret,
        }
    }

    /// The job described.
    ///
    /// # Errors
    ///
    /// A [`DescriptionError`] where an address or a path is malformed.
    pub fn job(&self) -> Result<Job, DescriptionError> {
        let bad = |what: &str, error: &dyn fmt::Display| {
            DescriptionError(format!("the description's {what} is malformed: {error}"))
        };
        let files = self
            .files
            .iter()
            .map(|file| {
                Ok(JobFile {
                    path: file.path.parse().map_err(|e| bad("file path", &e))?,
                    address: file.address.parse().map_err(|e| bad("file address", &e))?,
                    executable: file.executable,
                    download: Download {
                        url: file.url.clone(),
                        headers: file.headers.clone(),
                    },
                })
            })
            .collect::<Result<_, DescriptionError>>()?;
        Ok(Job {
            id: JobId::new(self.id),
            organisation: self.organisation.clone(),
            repository: self.repository,
            changeset: self
                .changeset
                .parse::<Address>()
                .map_err(|e| bad("changeset", &e))?,
            check: self.check.clone(),
            folder: self.folder.clone(),
            outputs: self
                .outputs
                .iter()
                .map(|path| path.parse().map_err(|e| bad("output path", &e)))
                .collect::<Result<_, DescriptionError>>()?,
            caches: self
                .caches
                .iter()
                .map(|path| path.parse().map_err(|e| bad("cache path", &e)))
                .collect::<Result<_, DescriptionError>>()?,
            command: self.command.clone(),
            heartbeat_interval: Duration::ZERO,
            files,
        })
    }
}

/// Runs the job `path` describes and posts its end. Returns the command's exit code where the
/// post was accepted, and failure otherwise.
pub async fn run(path: &Path) -> ExitCode {
    match run_within(path, POST_WINDOW).await {
        Ok(code) => code,
        Err(reason) => {
            eprintln!("run-job: {reason}");
            ExitCode::FAILURE
        }
    }
}

async fn run_within(path: &Path, window: Duration) -> Result<ExitCode, String> {
    let text = tokio::fs::read(path)
        .await
        .map_err(|error| format!("reading {} failed: {error}", path.display()))?;
    let description: Description = serde_json::from_slice(&text)
        .map_err(|error| format!("{} is not a job description: {error}", path.display()))?;
    let job = description.job().map_err(|error| error.to_string())?;
    let http = reqwest::Client::new();
    let executor = HostExecutor::new(std::env::temp_dir(), http.clone(), None);
    // The sender stays alive so that nothing stops the job.
    let (_stop, signal) = Stop::channel();
    let posted = match executor.run(&job, signal).await {
        Ok(Execution {
            end: End::Exited(code),
            output,
            produced,
        }) => Posted {
            end: PostedEnd::Exited(code),
            output,
            produced: PostedProduced::of(&produced),
        },
        Ok(Execution {
            end: End::Signalled | End::Stopped(_),
            output,
            ..
        }) => Posted {
            end: PostedEnd::Signalled,
            output,
            produced: PostedProduced::default(),
        },
        Err(error) => Posted {
            end: PostedEnd::Signalled,
            output: error.to_string(),
            produced: PostedProduced::default(),
        },
    };
    post(
        &http,
        &description.result_url,
        &description.result_secret,
        &posted,
        window,
    )
    .await?;
    Ok(match posted.end {
        PostedEnd::Exited(code) => u8::try_from(code).map_or(ExitCode::FAILURE, ExitCode::from),
        PostedEnd::Signalled => ExitCode::FAILURE,
    })
}

/// Posts `posted` to `url` until it is accepted or `window` has passed.
async fn post(
    http: &reqwest::Client,
    url: &str,
    secret: &str,
    posted: &Posted,
    window: Duration,
) -> Result<(), String> {
    let deadline = Instant::now() + window;
    loop {
        let answer = http
            .post(url)
            .bearer_auth(secret)
            .json(posted)
            .timeout(POST_TIMEOUT)
            .send()
            .await
            .map_err(|error| error.without_url().to_string())
            .and_then(|response| {
                // 409: the listener already holds this job's result.
                if response.status().is_success()
                    || response.status() == reqwest::StatusCode::CONFLICT
                {
                    Ok(())
                } else {
                    Err(format!("the runner answered {}", response.status()))
                }
            });
        match answer {
            Ok(()) => return Ok(()),
            Err(reason) if Instant::now() + POST_PAUSE >= deadline => {
                return Err(format!("posting the result failed: {reason}"));
            }
            Err(_) => tokio::time::sleep(POST_PAUSE).await,
        }
    }
}

#[cfg(test)]
mod tests;
