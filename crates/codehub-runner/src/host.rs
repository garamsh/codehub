//! The host adapter of the executor driver port: runs a job's command as a process of this
//! machine, in a directory of its own.

use std::os::fd::OwnedFd;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use rustix::process::{Pid, Signal};
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::unix::pipe;
use tokio::process::Command;

use crate::executor::{End, Execution, Executor, ExecutorError, Stop};
use crate::job_source::{Address, Job, JobFile, Produced, ProducedFile, RelativePath};

/// How much of a command's output a report carries.
pub const OUTPUT_LIMIT: usize = 64 * 1024;

/// How long a stopped command has to end after it is asked to, before it is killed.
pub const STOP_GRACE: Duration = Duration::from_secs(5);

/// How long the runner waits for the command's output to end once its shell has exited.
const DRAIN_WAIT: Duration = Duration::from_secs(5);

/// The end of a byte stream, at most [`OUTPUT_LIMIT`] bytes of it kept.
#[derive(Default)]
struct Tail(Vec<u8>);

impl Tail {
    fn push(&mut self, bytes: &[u8]) {
        self.0.extend_from_slice(bytes);
        if self.0.len() > 2 * OUTPUT_LIMIT {
            self.0.drain(..self.0.len() - OUTPUT_LIMIT);
        }
    }

    /// The kept bytes as text, cut so that it starts on a character boundary.
    fn text(&self) -> String {
        let start = self.0.len().saturating_sub(OUTPUT_LIMIT);
        let kept = self.0.get(start..).unwrap_or_default();
        let skip = kept
            .iter()
            .take(3)
            .take_while(|byte| (**byte & 0xC0) == 0x80)
            .count();
        let kept = kept.get(skip..).unwrap_or_default();
        let text = String::from_utf8_lossy(kept).into_owned();
        let mut from = text.len().saturating_sub(OUTPUT_LIMIT);
        while !text.is_char_boundary(from) {
            from += 1;
        }
        text.get(from..).unwrap_or_default().to_owned()
    }
}

/// Runs jobs as processes of this machine.
pub struct HostExecutor {
    work_dir: PathBuf,
    http: reqwest::Client,
    hidden: Option<String>,
    grace: Duration,
}

impl HostExecutor {
    /// An executor that makes each job's directory under `work_dir`. The environment variable
    /// named `hidden` is not passed on to commands.
    #[must_use]
    pub fn new(work_dir: PathBuf, http: reqwest::Client, hidden: Option<String>) -> Self {
        Self {
            work_dir,
            http,
            hidden,
            grace: STOP_GRACE,
        }
    }

    /// The same executor, giving a stopped command `grace` to end before it is killed.
    #[must_use]
    pub const fn with_grace(mut self, grace: Duration) -> Self {
        self.grace = grace;
        self
    }

    async fn execute(
        &self,
        job: &Job,
        dir: &Path,
        stop: &mut Stop,
    ) -> Result<Execution, ExecutorError> {
        let fetched = tokio::select! {
            reason = stop.requested() => {
                return Ok(Execution {
                    end: End::Stopped(reason),
                    output: String::new(),
                    produced: Produced::default(),
                });
            }
            fetched = self.fetch_all(job, dir) => fetched,
        };
        fetched?;
        let mut execution = self.command(job, dir, stop).await?;
        if execution.end == End::Exited(0) {
            execution.produced = Produced {
                outputs: collect(dir, &job.outputs).await?,
                caches: collect(dir, &job.caches).await?,
            };
        }
        Ok(execution)
    }

    async fn fetch_all(&self, job: &Job, dir: &Path) -> Result<(), ExecutorError> {
        for file in &job.files {
            self.fetch(file, dir).await?;
        }
        Ok(())
    }

    async fn fetch(&self, file: &JobFile, dir: &Path) -> Result<(), ExecutorError> {
        let target = dir.join(file.path.as_path());
        let download = |reason: String| ExecutorError::Download {
            path: file.path.to_string(),
            reason,
        };
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|error| download(error.to_string()))?;
        }
        let mut request = self.http.get(&file.download.url);
        for (name, value) in &file.download.headers {
            request = request.header(name, value);
        }
        let mut response = request
            .send()
            .await
            .map_err(|error| download(error.to_string()))?;
        if !response.status().is_success() {
            return Err(download(format!(
                "the server answered {}",
                response.status()
            )));
        }
        let mut out = tokio::fs::File::create(&target)
            .await
            .map_err(|error| download(error.to_string()))?;
        let mut hash = Sha256::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| download(error.to_string()))?
        {
            hash.update(&chunk);
            out.write_all(&chunk)
                .await
                .map_err(|error| download(error.to_string()))?;
        }
        out.flush()
            .await
            .map_err(|error| download(error.to_string()))?;
        drop(out);
        if Address::new(hash.finalize().into()) != file.address {
            return Err(ExecutorError::Mismatch {
                path: file.path.to_string(),
            });
        }
        let mode = if file.executable { 0o755 } else { 0o644 };
        tokio::fs::set_permissions(&target, std::fs::Permissions::from_mode(mode))
            .await
            .map_err(|error| download(error.to_string()))
    }

    async fn command(
        &self,
        job: &Job,
        dir: &Path,
        stop: &mut Stop,
    ) -> Result<Execution, ExecutorError> {
        let (reader, writer) = std::io::pipe().map_err(ExecutorError::Spawn)?;
        let writer_again = writer.try_clone().map_err(ExecutorError::Spawn)?;
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg(&job.command)
            .current_dir(dir)
            .env("CODEHUB_ORGANISATION", &job.organisation)
            .env("CODEHUB_REPOSITORY", job.repository.to_string())
            .env("CODEHUB_CHANGESET", job.changeset.to_string())
            .env("CODEHUB_CHECK", &job.check)
            .env("CODEHUB_FOLDER", &job.folder)
            .stdin(Stdio::null())
            .stdout(writer)
            .stderr(writer_again)
            .process_group(0)
            .kill_on_drop(true);
        if let Some(hidden) = &self.hidden {
            command.env_remove(hidden);
        }
        let mut child = command.spawn().map_err(ExecutorError::Spawn)?;
        // The command object holds the write ends; they must close when the processes do.
        drop(command);

        let group = child
            .id()
            .and_then(|id| i32::try_from(id).ok())
            .and_then(Pid::from_raw);
        let receiver =
            pipe::Receiver::from_owned_fd(OwnedFd::from(reader)).map_err(ExecutorError::Spawn)?;
        let tail = Arc::new(Mutex::new(Tail::default()));
        let drain = tokio::spawn(read_into(receiver, Arc::clone(&tail)));

        let end = tokio::select! {
            status = child.wait() => classify(status.map_err(ExecutorError::Spawn)?),
            reason = stop.requested() => {
                signal(group, Signal::TERM);
                if tokio::time::timeout(self.grace, child.wait()).await.is_err() {
                    signal(group, Signal::KILL);
                    child.wait().await.map_err(ExecutorError::Spawn)?;
                }
                End::Stopped(reason)
            }
        };
        // Whatever the command left running in its group goes with it.
        signal(group, Signal::KILL);
        let drain_abort = drain.abort_handle();
        if tokio::time::timeout(DRAIN_WAIT, drain).await.is_err() {
            drain_abort.abort();
        }
        let output = tail.lock().unwrap_or_else(PoisonError::into_inner).text();
        Ok(Execution {
            end,
            output,
            produced: Produced::default(),
        })
    }
}

/// The regular files under each of `declared`, in the job's directory `dir`. A path the command
/// left no directory at has none; a link or another kind of file is not collected.
async fn collect(
    dir: &Path,
    declared: &[RelativePath],
) -> Result<Vec<ProducedFile>, ExecutorError> {
    let mut found = Vec::new();
    for path in declared {
        let mut pending = vec![(dir.join(path.as_path()), path.to_string())];
        while let Some((at, name)) = pending.pop() {
            let fail = |error: std::io::Error| ExecutorError::Collect {
                path: name.clone(),
                reason: error.to_string(),
            };
            let kind = match tokio::fs::symlink_metadata(&at).await {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(fail(error)),
            };
            if kind.is_dir() {
                let mut entries = tokio::fs::read_dir(&at).await.map_err(fail)?;
                while let Some(entry) = entries.next_entry().await.map_err(fail)? {
                    let child = entry.file_name().to_string_lossy().into_owned();
                    pending.push((entry.path(), format!("{name}/{child}")));
                }
            } else if kind.is_file() {
                let bytes = tokio::fs::read(&at).await.map_err(fail)?;
                found.push(ProducedFile {
                    path: name.parse().map_err(|error| ExecutorError::Collect {
                        path: name.clone(),
                        reason: format!("{error}"),
                    })?,
                    address: Address::new(Sha256::digest(&bytes).into()),
                    executable: kind.permissions().mode() & 0o111 != 0,
                    bytes,
                });
            }
        }
    }
    found.sort_by_key(|file| file.path.to_string());
    found.dedup_by(|a, b| a.path == b.path);
    Ok(found)
}

async fn read_into(mut receiver: pipe::Receiver, tail: Arc<Mutex<Tail>>) {
    let mut buffer = vec![0_u8; 16 * 1024];
    loop {
        match receiver.read(&mut buffer).await {
            Ok(0) | Err(_) => return,
            Ok(count) => tail
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(buffer.get(..count).unwrap_or_default()),
        }
    }
}

fn classify(status: ExitStatus) -> End {
    status.code().map_or(End::Signalled, End::Exited)
}

/// Sends `signal` to every process of `group`, ignoring that none is left.
fn signal(group: Option<Pid>, signal: Signal) {
    if let Some(group) = group {
        let _ = rustix::process::kill_process_group(group, signal);
    }
}

impl Executor for HostExecutor {
    async fn run(&self, job: &Job, mut stop: Stop) -> Result<Execution, ExecutorError> {
        tokio::fs::create_dir_all(&self.work_dir)
            .await
            .map_err(ExecutorError::Directory)?;
        let dir = self
            .work_dir
            .join(format!("codehub-job-{}-{}", std::process::id(), job.id));
        tokio::fs::create_dir(&dir)
            .await
            .map_err(ExecutorError::Directory)?;
        let result = self.execute(job, &dir, &mut stop).await;
        let removed = tokio::fs::remove_dir_all(&dir).await;
        match (result, removed) {
            (Ok(execution), Err(error)) => {
                eprintln!("job {}: removing {} failed: {error}", job.id, dir.display());
                Ok(execution)
            }
            (result, _) => result,
        }
    }
}

#[cfg(test)]
mod tests;
