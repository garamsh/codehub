//! The host adapter of the executor driver port: runs a job's command as a process of this
//! machine, in a directory of its own.

use std::os::fd::OwnedFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use rustix::process::{Pid, Signal};
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::unix::pipe;
use tokio::process::Command;

use crate::executor::{End, Execution, Executor, ExecutorError, Stop};
use crate::job_source::{
    Address, Change, Job, JobFile, Produced, ProducedFile, RelativePath, RunOutcome, RunWork,
    Stream,
};
use crate::object_cache::{ObjectCache, ObjectCounts};

/// How much of a command's output a report carries.
pub const OUTPUT_LIMIT: usize = 64 * 1024;

/// How much of each of a run's output streams is kept whole; past it, the last of it is kept.
pub const STREAM_LIMIT: usize = 64 * 1024 * 1024;

/// How long a stopped command has to end after it is asked to, before it is killed.
pub const STOP_GRACE: Duration = Duration::from_secs(5);

/// The modification time of a file restored from a cache, in seconds after the epoch (1970-01-01T00:00:01Z): earlier than any
/// source, so that a build tool comparing times always sees a source as newer.
const CACHE_FILE_SECONDS: u64 = 1;

/// How long the runner waits for the command's output to end once its shell has exited.
const DRAIN_WAIT: Duration = Duration::from_secs(5);

/// The end of a byte stream, at most `limit` bytes of it kept.
struct Tail {
    bytes: Vec<u8>,
    limit: usize,
    cut: bool,
}

impl Default for Tail {
    fn default() -> Self {
        Self::with_limit(OUTPUT_LIMIT)
    }
}

impl Tail {
    const fn with_limit(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
            cut: false,
        }
    }

    fn push(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
        if self.bytes.len() > 2 * self.limit {
            self.bytes.drain(..self.bytes.len() - self.limit);
            self.cut = true;
        }
    }

    /// The kept bytes as text, cut so that it starts on a character boundary.
    fn text(&self) -> String {
        let start = self.bytes.len().saturating_sub(self.limit);
        let kept = self.bytes.get(start..).unwrap_or_default();
        let skip = kept
            .iter()
            .take(3)
            .take_while(|byte| (**byte & 0xC0) == 0x80)
            .count();
        let kept = kept.get(skip..).unwrap_or_default();
        let text = String::from_utf8_lossy(kept).into_owned();
        let mut from = text.len().saturating_sub(self.limit);
        while !text.is_char_boundary(from) {
            from += 1;
        }
        text.get(from..).unwrap_or_default().to_owned()
    }

    /// The last `limit` bytes, and whether anything before them was dropped.
    fn into_stream(mut self) -> Stream {
        let cut = self.cut || self.bytes.len() > self.limit;
        self.bytes
            .drain(..self.bytes.len().saturating_sub(self.limit));
        Stream::new(self.bytes, cut)
    }
}

/// Runs jobs as processes of this machine.
pub struct HostExecutor {
    work_dir: PathBuf,
    cache: ObjectCache,
    http: reqwest::Client,
    hidden: Option<String>,
    grace: Duration,
}

impl HostExecutor {
    /// An executor that makes each job's directory under `work_dir`, and keeps the objects its
    /// jobs take in `work_dir/objects`, at most `cache_limit` bytes of them between jobs. The
    /// environment variable named `hidden` is not passed on to commands.
    #[must_use]
    pub fn new(
        work_dir: PathBuf,
        cache_limit: u64,
        http: reqwest::Client,
        hidden: Option<String>,
    ) -> Self {
        Self {
            cache: ObjectCache::new(work_dir.join("objects"), cache_limit),
            work_dir,
            http,
            hidden,
            grace: STOP_GRACE,
        }
    }

    /// Removes the partial files an earlier runner left in the object cache. Call it once, before
    /// the first job.
    ///
    /// # Errors
    ///
    /// An I/O error while listing or removing.
    pub async fn clean(&self) -> std::io::Result<()> {
        self.cache.clean().await
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
        counts: &mut ObjectCounts,
    ) -> Result<Execution, ExecutorError> {
        let fetched = tokio::select! {
            reason = stop.requested() => {
                return Ok(Execution {
                    end: End::Stopped(reason),
                    output: String::new(),
                    produced: Produced::default(),
                    objects: None,
                    run: None,
                });
            }
            fetched = self.fetch_all(job, dir, counts) => fetched,
        };
        fetched?;
        let mut execution = self.command(job, dir, stop).await?;
        match (&job.run, execution.end) {
            (None, End::Exited(0)) => execution.produced = collect_produced(dir, job).await?,
            (Some(run), End::Exited(_)) => {
                execution.produced = collect_produced(dir, job).await?;
                let (changes, refused) = changes(dir, job, run).await?;
                if let Some(outcome) = &mut execution.run {
                    outcome.changes = changes;
                    outcome.refused = refused;
                }
            }
            _ => {}
        }
        self.remember(job, &execution).await;
        Ok(execution)
    }

    /// Puts the objects the runner is about to upload, which the next job may be sent, into the
    /// object cache. The streams are not put there: no job is sent one.
    async fn remember(&self, job: &Job, execution: &Execution) {
        let produced = execution
            .produced
            .outputs
            .iter()
            .chain(&execution.produced.caches);
        let changed = execution
            .run
            .iter()
            .flat_map(|outcome| &outcome.changes)
            .filter_map(|change| match change {
                Change::Written(file) => Some(file),
                Change::Deleted(_) => None,
            });
        for file in produced.chain(changed) {
            match self.cache.keep_bytes(file.address, &file.bytes).await {
                Ok(true) => {}
                Ok(false) => eprintln!(
                    "job {}: {} does not match its address, not kept in the object cache",
                    job.id, file.path
                ),
                Err(error) => eprintln!(
                    "job {}: keeping {} in the object cache failed: {error}",
                    job.id, file.path
                ),
            }
        }
    }

    async fn fetch_all(
        &self,
        job: &Job,
        dir: &Path,
        counts: &mut ObjectCounts,
    ) -> Result<(), ExecutorError> {
        for file in &job.files {
            self.fetch(file, dir, counts).await?;
        }
        for cache in job.run.iter().flat_map(|run| &run.caches) {
            let base = dir.join(cache.path.as_path());
            tokio::fs::create_dir_all(&base)
                .await
                .map_err(|error| ExecutorError::Download {
                    path: cache.path.to_string(),
                    reason: error.to_string(),
                })?;
            for file in &cache.files {
                self.fetch(file, &base, counts).await?;
                let path = base.join(file.path.as_path());
                set_modified(&path, UNIX_EPOCH + Duration::from_secs(CACHE_FILE_SECONDS))
                    .await
                    .map_err(|error| ExecutorError::Download {
                        path: file.path.to_string(),
                        reason: error.to_string(),
                    })?;
            }
        }
        Ok(())
    }

    /// Writes the file into `dir` from the object cache, or else from the server
    /// and then into the cache.
    async fn fetch(
        &self,
        file: &JobFile,
        dir: &Path,
        counts: &mut ObjectCounts,
    ) -> Result<(), ExecutorError> {
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
        let mode = if file.executable { 0o755 } else { 0o644 };
        let permissions = std::fs::Permissions::from_mode(mode);
        if self
            .cache
            .take(file.address, &target)
            .await
            .map_err(|error| download(format!("the object cache failed: {error}")))?
        {
            counts.cached += 1;
            return tokio::fs::set_permissions(&target, permissions)
                .await
                .map_err(|error| download(error.to_string()));
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
        self.cache
            .keep(file.address, &target)
            .await
            .map_err(|error| download(format!("the object cache failed: {error}")))?;
        counts.downloaded += 1;
        tokio::fs::set_permissions(&target, permissions)
            .await
            .map_err(|error| download(error.to_string()))
    }

    async fn command(
        &self,
        job: &Job,
        dir: &Path,
        stop: &mut Stop,
    ) -> Result<Execution, ExecutorError> {
        let is_run = job.run.is_some();
        let (out_reader, out_writer) = std::io::pipe().map_err(ExecutorError::Spawn)?;
        // A check's streams share one pipe; a run's are kept apart.
        let (err_reader, err_writer) = if is_run {
            let (reader, writer) = std::io::pipe().map_err(ExecutorError::Spawn)?;
            (Some(reader), writer)
        } else {
            (None, out_writer.try_clone().map_err(ExecutorError::Spawn)?)
        };
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg(&job.command)
            .current_dir(dir)
            .stdin(Stdio::null())
            .stdout(out_writer)
            .stderr(err_writer)
            .process_group(0)
            .kill_on_drop(true);
        match &job.run {
            Some(run) => {
                // The job's `env` is the whole of the command's CODEHUB_* variables.
                for (name, _) in std::env::vars_os() {
                    if name.to_string_lossy().starts_with("CODEHUB_") {
                        command.env_remove(name);
                    }
                }
                command.envs(run.env.iter().map(|(name, value)| (name, value)));
            }
            None => {
                command
                    .env("CODEHUB_ORGANISATION", &job.organisation)
                    .env("CODEHUB_REPOSITORY", job.repository.to_string())
                    .env("CODEHUB_CHANGESET", job.changeset.to_string())
                    .env("CODEHUB_CHECK", &job.check)
                    .env("CODEHUB_FOLDER", &job.folder);
            }
        }
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
        let limit = if is_run { STREAM_LIMIT } else { OUTPUT_LIMIT };
        let out_tail = Arc::new(Mutex::new(Tail::with_limit(limit)));
        let err_tail = Arc::new(Mutex::new(Tail::with_limit(limit)));
        let mut drains = vec![tokio::spawn(read_into(
            pipe::Receiver::from_owned_fd(OwnedFd::from(out_reader))
                .map_err(ExecutorError::Spawn)?,
            Arc::clone(&out_tail),
        ))];
        if let Some(reader) = err_reader {
            drains.push(tokio::spawn(read_into(
                pipe::Receiver::from_owned_fd(OwnedFd::from(reader))
                    .map_err(ExecutorError::Spawn)?,
                Arc::clone(&err_tail),
            )));
        }

        let end = tokio::select! {
            status = child.wait() => classify(status.map_err(ExecutorError::Spawn)?, is_run),
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
        let aborts: Vec<_> = drains
            .iter()
            .map(tokio::task::JoinHandle::abort_handle)
            .collect();
        let drained = async {
            for drain in drains {
                let _ = drain.await;
            }
        };
        if tokio::time::timeout(DRAIN_WAIT, drained).await.is_err() {
            for abort in aborts {
                abort.abort();
            }
        }
        let take = |tail: Arc<Mutex<Tail>>| {
            std::mem::replace(
                &mut *tail.lock().unwrap_or_else(PoisonError::into_inner),
                Tail::with_limit(limit),
            )
        };
        let (output, run) = if is_run {
            let outcome = RunOutcome {
                stdout: Some(take(out_tail).into_stream()),
                stderr: Some(take(err_tail).into_stream()),
                ..RunOutcome::default()
            };
            (String::new(), Some(outcome))
        } else {
            (take(out_tail).text(), None)
        };
        Ok(Execution {
            end,
            output,
            produced: Produced::default(),
            objects: None,
            run,
        })
    }
}

async fn set_modified(path: &Path, time: SystemTime) -> std::io::Result<()> {
    tokio::fs::File::options()
        .write(true)
        .open(path)
        .await?
        .into_std()
        .await
        .set_modified(time)
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

/// What lies at `path` under `dir`, looking at every component without following a link.
enum Place {
    /// The path leads only through directories; the last component's metadata, if it exists.
    Reached(Option<std::fs::Metadata>),
    /// A component before the last is a link.
    Linked,
}

async fn place(dir: &Path, path: &RelativePath) -> Result<Place, ExecutorError> {
    let mut at = dir.to_path_buf();
    let mut parts = path.as_path().components().peekable();
    while let Some(part) = parts.next() {
        at.push(part);
        let metadata = match tokio::fs::symlink_metadata(&at).await {
            Ok(metadata) => metadata,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) =>
            {
                return Ok(Place::Reached(None));
            }
            Err(error) => {
                return Err(ExecutorError::Collect {
                    path: path.to_string(),
                    reason: error.to_string(),
                });
            }
        };
        if parts.peek().is_none() {
            return Ok(Place::Reached(Some(metadata)));
        }
        if metadata.file_type().is_symlink() {
            return Ok(Place::Linked);
        }
        if !metadata.is_dir() {
            return Ok(Place::Reached(None));
        }
    }
    Ok(Place::Reached(None))
}

/// The files the job declares as outputs and caches, none of them reached through a link.
async fn collect_produced(dir: &Path, job: &Job) -> Result<Produced, ExecutorError> {
    let mut outputs = Vec::new();
    let mut caches = Vec::new();
    for (declared, kept) in [(&job.outputs, &mut outputs), (&job.caches, &mut caches)] {
        let mut clear = Vec::new();
        for path in declared {
            if matches!(place(dir, path).await?, Place::Reached(_)) {
                clear.push(path.clone());
            }
        }
        *kept = collect(dir, &clear).await?;
    }
    Ok(Produced { outputs, caches })
}

/// The run's changes to the workspace's files, and what it left that cannot be a change.
/// A path under a cache path is never a change.
async fn changes(
    dir: &Path,
    job: &Job,
    run: &RunWork,
) -> Result<(Vec<Change>, Vec<String>), ExecutorError> {
    let under_cache = |path: &Path| {
        job.caches
            .iter()
            .any(|cache| path.starts_with(cache.as_path()))
    };
    let mut changes = Vec::new();
    let mut refused = Vec::new();
    for declared in job.outputs.iter().chain(&job.caches) {
        if matches!(place(dir, declared).await?, Place::Linked) {
            refused.push(declared.to_string());
        }
    }
    let sent: std::collections::HashSet<String> =
        job.files.iter().map(|file| file.path.to_string()).collect();
    for file in &job.files {
        if under_cache(file.path.as_path()) {
            continue;
        }
        match place(dir, &file.path).await? {
            Place::Reached(None) => changes.push(Change::Deleted(file.path.clone())),
            Place::Linked => refused.push(file.path.to_string()),
            Place::Reached(Some(metadata)) if metadata.is_file() => {
                let executable = metadata.permissions().mode() & 0o111 != 0;
                let bytes = read_file(dir, &file.path).await?;
                let address = Address::new(Sha256::digest(&bytes).into());
                if address != file.address || executable != file.executable {
                    changes.push(Change::Written(ProducedFile {
                        path: file.path.clone(),
                        address,
                        executable,
                        bytes,
                    }));
                }
            }
            Place::Reached(Some(_)) => refused.push(file.path.to_string()),
        }
    }
    let mut written = std::collections::HashSet::new();
    for added in &run.add {
        let mut pending = vec![(dir.join(added.as_path()), added.to_string())];
        if matches!(place(dir, added).await?, Place::Linked) {
            refused.push(added.to_string());
            continue;
        }
        while let Some((at, name)) = pending.pop() {
            if under_cache(Path::new(&name)) {
                continue;
            }
            let fail = |error: std::io::Error| ExecutorError::Collect {
                path: name.clone(),
                reason: error.to_string(),
            };
            let metadata = match tokio::fs::symlink_metadata(&at).await {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(fail(error)),
            };
            if metadata.is_dir() {
                let mut entries = tokio::fs::read_dir(&at).await.map_err(fail)?;
                while let Some(entry) = entries.next_entry().await.map_err(fail)? {
                    let child = entry.file_name().to_string_lossy().into_owned();
                    pending.push((entry.path(), format!("{name}/{child}")));
                }
            } else if metadata.is_file() {
                if sent.contains(&name) || !written.insert(name.clone()) {
                    continue;
                }
                let path: RelativePath = name.parse().map_err(|error| ExecutorError::Collect {
                    path: name.clone(),
                    reason: format!("{error}"),
                })?;
                let bytes = tokio::fs::read(&at).await.map_err(fail)?;
                changes.push(Change::Written(ProducedFile {
                    address: Address::new(Sha256::digest(&bytes).into()),
                    executable: metadata.permissions().mode() & 0o111 != 0,
                    path,
                    bytes,
                }));
            } else {
                refused.push(name);
            }
        }
    }
    refused.sort();
    refused.dedup();
    Ok((changes, refused))
}

async fn read_file(dir: &Path, path: &RelativePath) -> Result<Vec<u8>, ExecutorError> {
    tokio::fs::read(dir.join(path.as_path()))
        .await
        .map_err(|error| ExecutorError::Collect {
            path: path.to_string(),
            reason: error.to_string(),
        })
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

/// A run reports a signal as the shell does, 128 plus its number; a check reports none.
fn classify(status: ExitStatus, is_run: bool) -> End {
    match (status.code(), status.signal()) {
        (Some(code), _) => End::Exited(code),
        (None, Some(number)) if is_run => End::Exited(128 + number),
        (None, _) => End::Signalled,
    }
}

/// Sends `signal` to every process of `group`, ignoring that none is left.
fn signal(group: Option<Pid>, signal: Signal) {
    if let Some(group) = group {
        let _ = rustix::process::kill_process_group(group, signal);
    }
}

impl HostExecutor {
    /// Runs the job and counts the objects it took, from the server and from the cache.
    async fn run_counted(
        &self,
        job: &Job,
        mut stop: Stop,
    ) -> (Result<Execution, ExecutorError>, ObjectCounts) {
        let mut counts = ObjectCounts::default();
        let result = self.run_in_directory(job, &mut stop, &mut counts).await;
        if let Err(error) = self.cache.evict().await {
            eprintln!(
                "job {}: evicting from the object cache failed: {error}",
                job.id
            );
        }
        (result, counts)
    }

    async fn run_in_directory(
        &self,
        job: &Job,
        stop: &mut Stop,
        counts: &mut ObjectCounts,
    ) -> Result<Execution, ExecutorError> {
        tokio::fs::create_dir_all(&self.work_dir)
            .await
            .map_err(ExecutorError::Directory)?;
        let dir = self
            .work_dir
            .join(format!("codehub-job-{}-{}", std::process::id(), job.id));
        tokio::fs::create_dir(&dir)
            .await
            .map_err(ExecutorError::Directory)?;
        let result = self.execute(job, &dir, stop, counts).await;
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

#[async_trait]
impl Executor for HostExecutor {
    async fn run(&self, job: &Job, stop: Stop) -> Result<Execution, ExecutorError> {
        let (result, counts) = self.run_counted(job, stop).await;
        eprintln!(
            "job {}: {} objects downloaded, {} taken from the object cache",
            job.id, counts.downloaded, counts.cached
        );
        result.map(|execution| Execution {
            objects: Some(counts),
            ..execution
        })
    }
}

#[cfg(test)]
mod tests;
