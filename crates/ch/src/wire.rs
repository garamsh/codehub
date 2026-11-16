//! The request and response bodies of the HTTP API (ADR 0024), declared here and checked against
//! `crates/adapter-http/openapi.json` by `tests/wire.rs`. Each type takes the name of its schema.

use serde::{Deserialize, Serialize};

/// The name of an organisation or repository to create.
#[derive(Debug, Serialize, Deserialize)]
pub struct NewName {
    /// The name.
    pub name: String,
}

/// An organisation the server created.
#[derive(Debug, Serialize, Deserialize)]
pub struct Created {
    /// The name.
    pub name: String,
}

/// A repository the server created.
#[derive(Debug, Serialize, Deserialize)]
pub struct CreatedRepository {
    /// The name.
    pub name: String,
    /// The repository's identity.
    pub id: u64,
}

/// An issuer and a subject, which together name a principal.
#[derive(Debug, Serialize, Deserialize)]
pub struct Principal {
    /// The issuer URL.
    pub issuer: String,
    /// The subject at that issuer.
    pub subject: String,
}

/// A member's role in an organisation.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// May manage members.
    Admin,
    /// May use what its grants cover.
    Member,
}

/// What a grant allows.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    /// Read the prefix.
    Read,
    /// Write, and so read, the prefix.
    Write,
    /// Run commands against the caller's workspace; the whole repository only.
    Run,
    /// Pull the repository's images; the whole repository only.
    ImagePull,
    /// Push the repository's images; the whole repository only.
    ImagePush,
}

/// A principal and its role in an organisation.
#[derive(Debug, Serialize, Deserialize)]
pub struct Member {
    /// The issuer URL.
    pub issuer: String,
    /// The subject at that issuer.
    pub subject: String,
    /// The role.
    pub role: Role,
}

/// Access of a principal to the files under a prefix of a repository.
#[derive(Debug, Serialize, Deserialize)]
pub struct Grant {
    /// The issuer URL.
    pub issuer: String,
    /// The subject at that issuer.
    pub subject: String,
    /// The directory covered; empty for the whole repository.
    pub prefix: String,
    /// What the grant allows.
    pub access: Access,
}

/// A request for a grant to upload an object.
#[derive(Debug, Serialize, Deserialize)]
pub struct Upload {
    /// The object's address, `sha256:` and 64 hex digits.
    pub address: String,
    /// The object's size in bytes.
    pub size: u64,
}

/// The method of a transfer grant.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum TransferMethod {
    /// Download.
    Get,
    /// Upload.
    Put,
}

/// One header a transfer must carry.
#[derive(Debug, Serialize, Deserialize)]
pub struct Header {
    /// The header name.
    pub name: String,
    /// The header value.
    pub value: String,
}

/// Permission to move object bytes to or from the object store.
#[derive(Debug, Serialize, Deserialize)]
pub struct TransferGrant {
    /// Where to send the request.
    pub url: String,
    /// The method to send it with.
    pub method: TransferMethod,
    /// The headers to send with it, and no others.
    pub headers: Vec<Header>,
    /// Seconds since 1970-01-01T00:00:00Z.
    pub expires_at: i64,
}

/// The kind of a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// An ordinary file.
    Regular,
    /// A file with the executable bit.
    Executable,
}

impl Kind {
    /// The kind as the API and `ch` write it.
    #[must_use]
    pub const fn text(self) -> &'static str {
        match self {
            Self::Regular => "regular",
            Self::Executable => "executable",
        }
    }
}

/// A file staged in a draft.
#[derive(Debug, Serialize, Deserialize)]
pub struct StagedFile {
    /// The kind of the file.
    pub kind: Kind,
    /// The address of its bytes.
    pub content: String,
}

/// The message a draft lands with.
#[derive(Debug, Serialize, Deserialize)]
pub struct Landing {
    /// The message.
    pub message: String,
}

/// The changeset a landing made.
#[derive(Debug, Serialize, Deserialize)]
pub struct Landed {
    /// The changeset's address.
    pub changeset: String,
}

/// A file at the head of the repository.
#[derive(Debug, Serialize, Deserialize)]
pub struct File {
    /// The kind of the file.
    pub kind: Kind,
    /// Where its bytes can be downloaded.
    pub download: TransferGrant,
}

/// One version of a file.
#[derive(Debug, Serialize, Deserialize)]
pub struct Version {
    /// The changeset that made it.
    pub changeset: String,
    /// The file's path.
    pub path: String,
    /// The kind of the file.
    pub kind: Kind,
    /// The address of its bytes.
    pub content: String,
    /// The changesets this version follows.
    pub previous: Vec<String>,
    /// Why the changeset landed.
    pub message: String,
}

/// The versions of a file, newest first.
#[derive(Debug, Serialize, Deserialize)]
pub struct History {
    /// The versions.
    pub versions: Vec<Version>,
}

/// An RFC 9457 problem document.
#[derive(Debug, Serialize, Deserialize)]
pub struct Problem {
    /// Always `about:blank`.
    #[serde(rename = "type")]
    pub kind: String,
    /// The status text.
    pub title: String,
    /// The HTTP status code.
    pub status: u16,
    /// What went wrong.
    pub detail: String,
    /// The paths a 403 or a 409 is about.
    #[serde(default)]
    pub paths: Vec<String>,
}

/// A principal to register as a runner.
#[derive(Debug, Serialize, Deserialize)]
pub struct NewRunner {
    /// The issuer URL.
    pub issuer: String,
    /// The subject at that issuer.
    pub subject: String,
    /// What the runner offers; a job's labels must all be among them.
    pub labels: Vec<String>,
    /// The repository's name, where the runner takes only its jobs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
}

/// A registered runner.
#[derive(Debug, Serialize, Deserialize)]
pub struct Runner {
    /// The registration's identifier.
    pub id: u64,
    /// The organisation the runner is registered in.
    pub organisation: String,
    /// The repository it takes jobs of; absent for any repository of the organisation.
    pub repository_id: Option<u64>,
    /// The issuer URL.
    pub issuer: String,
    /// The subject at that issuer.
    pub subject: String,
    /// What the runner offers.
    pub labels: Vec<String>,
}

/// The registrations of an organisation.
#[derive(Debug, Serialize, Deserialize)]
pub struct Runners {
    /// The registrations, oldest first.
    pub runners: Vec<Runner>,
}

/// Where a check's job is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckState {
    /// Waiting for the checks it needs to pass.
    Waiting,
    /// Waiting for a runner.
    Queued,
    /// Claimed by a runner.
    Running,
    /// Ended, passed.
    Passed,
    /// Ended, failed.
    Failed,
    /// Ended past its timeout.
    TimedOut,
    /// Ended by cancellation.
    Cancelled,
    /// Never run: a check it needs did not pass. It has not passed.
    Skipped,
    /// Not run: its key had already passed, and it takes that result.
    Cached,
}

impl CheckState {
    /// Whether the job has ended.
    #[must_use]
    pub const fn ended(self) -> bool {
        match self {
            Self::Waiting | Self::Queued | Self::Running => false,
            Self::Passed
            | Self::Failed
            | Self::TimedOut
            | Self::Cancelled
            | Self::Skipped
            | Self::Cached => true,
        }
    }

    /// Whether the check passed, by a run or from the cache.
    #[must_use]
    pub const fn passed(self) -> bool {
        matches!(self, Self::Passed | Self::Cached)
    }

    /// The state as the API and `ch` write it.
    #[must_use]
    pub const fn text(self) -> &'static str {
        match self {
            Self::Waiting => "waiting",
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::TimedOut => "timed_out",
            Self::Cancelled => "cancelled",
            Self::Skipped => "skipped",
            Self::Cached => "cached",
        }
    }

    /// The state as `ch checks` prints it: as [`CheckState::text`], but for a cached result,
    /// which reads as a pass.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Cached => "passed (cached)",
            Self::Waiting
            | Self::Queued
            | Self::Running
            | Self::Passed
            | Self::Failed
            | Self::TimedOut
            | Self::Cancelled
            | Self::Skipped => self.text(),
        }
    }
}

/// What came of reverting a changeset because a blocking check failed.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Revert {
    /// The revert landed as this changeset.
    Landed {
        /// The revert changeset's address.
        changeset: String,
    },
    /// The revert conflicts with later changes and landed nothing.
    Conflicted {
        /// The paths it conflicts on.
        paths: Vec<String>,
    },
    /// The revert failed after it was reserved and landed nothing.
    Failed {
        /// Why it failed.
        reason: String,
    },
}

/// One check of a changeset.
#[derive(Debug, Serialize, Deserialize)]
pub struct Check {
    /// The job's identifier.
    pub id: u64,
    /// The check's name.
    pub name: String,
    /// Whether a failure reverts the changeset.
    pub blocking: bool,
    /// The labels the job is routed by.
    pub labels: Vec<String>,
    /// Where the job is.
    pub state: CheckState,
    /// The command's exit code, where it has one.
    pub exit_code: Option<i32>,
    /// The end of the command's output.
    pub output_tail: String,
    /// While running: when the lease lapses, in seconds since the epoch.
    pub lease_expires_at: Option<i64>,
    /// While running: when the job times out, in seconds since the epoch.
    pub deadline: Option<i64>,
    /// What came of a revert, where one was attempted.
    pub revert: Option<Revert>,
}

/// The checks of a changeset.
#[derive(Debug, Serialize, Deserialize)]
pub struct Checks {
    /// The checks.
    pub checks: Vec<Check>,
}

/// What landing a draft would do about each check, in `needs` order.
#[derive(Debug, Serialize, Deserialize)]
pub struct Plan {
    /// The checks; a check comes after every check it needs.
    pub checks: Vec<PlannedCheck>,
}

/// One check of a plan.
#[derive(Debug, Serialize, Deserialize)]
pub struct PlannedCheck {
    /// The check's name, with its folder where it is not at the root.
    pub name: String,
    /// Whether a failure would revert the landing.
    pub blocking: bool,
    /// The patterns that select the files the check reads.
    pub inputs: Vec<String>,
    /// Whether the check would run or take a passed result.
    pub outcome: PlanOutcome,
    /// Why the check would run; empty where it is cached.
    pub reasons: Vec<PlanReason>,
}

/// What would become of a check.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanOutcome {
    /// The check would run.
    Run,
    /// A run of the check's key has passed, and it takes that result.
    Cached,
}

/// Why a check would run.
#[derive(Debug, Serialize, Deserialize)]
pub struct PlanReason {
    /// The kind of reason.
    pub reason: ReasonKind,
    /// The needed check that runs, for [`ReasonKind::NeedRuns`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<String>,
}

/// The kinds of reason a check would run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasonKind {
    /// The draft changes a file the check reads.
    InputsChanged,
    /// A check it needs would run.
    #[serde(rename = "needs_run")]
    NeedRuns,
    /// No run of its key has passed.
    NoPassedRun,
}

/// A ref and the changeset it points at.
#[derive(Debug, Serialize, Deserialize)]
pub struct Ref {
    /// `main` or `last-green`.
    pub name: String,
    /// The changeset's address.
    pub changeset: String,
}

/// What a workspace does to a path against its base.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// The base lacks the path.
    Added,
    /// The base holds the path with other content or kind.
    Modified,
    /// The base holds the path and the workspace removes it.
    Deleted,
}

impl Status {
    /// The status as the API and `ch` write it.
    #[must_use]
    pub const fn text(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Modified => "modified",
            Self::Deleted => "deleted",
        }
    }
}

/// A path a workspace changes.
#[derive(Debug, Serialize, Deserialize)]
pub struct ChangedPath {
    /// The path.
    pub path: String,
    /// What the workspace does to it.
    pub status: Status,
    /// The file mode; a deleted path's is the base's.
    pub kind: Kind,
    /// Where an added or modified path's bytes can be downloaded; absent for a deleted one.
    pub download: Option<TransferGrant>,
}

/// A workspace: a draft with its base and description.
#[derive(Debug, Serialize, Deserialize)]
pub struct Workspace {
    /// Which opening of the workspace this is; landing closes it.
    pub instance: u64,
    /// 0 when the workspace opens, and one more each time its changes are recorded.
    pub version: u64,
    /// The changeset it is based on; absent where `main` was unset.
    pub base: Option<String>,
    /// The description.
    pub description: String,
    /// The changed paths, in path order.
    pub changes: Vec<ChangedPath>,
}

/// A workspace's description.
#[derive(Debug, Serialize, Deserialize)]
pub struct Description {
    /// The description.
    pub description: String,
}

/// One path's change in a snapshot.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "lowercase")]
pub enum SnapshotChange {
    /// Record a file.
    File {
        /// The path.
        path: String,
        /// The kind of the file.
        kind: Kind,
        /// The address of its bytes.
        content: String,
    },
    /// Record a deletion.
    Delete {
        /// The path.
        path: String,
    },
    /// Return the path to its base.
    Restore {
        /// The path.
        path: String,
    },
}

/// Changes recorded together, or not at all.
#[derive(Debug, Serialize, Deserialize)]
pub struct Snapshot {
    /// The changes, at most one to a path.
    pub changes: Vec<SnapshotChange>,
}

/// How a path's content differs.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum DiffContent {
    /// A line diff.
    Lines {
        /// The unified diff, as the server prints it.
        unified: String,
    },
    /// A side is not text.
    Binary,
    /// Only the file kind changed.
    Kind,
}

/// A file kind that changed.
#[derive(Debug, Serialize, Deserialize)]
pub struct KindChange {
    /// The base's kind.
    pub before: Kind,
    /// The workspace's kind.
    pub after: Kind,
}

/// The difference of one path.
#[derive(Debug, Serialize, Deserialize)]
pub struct PathDiff {
    /// The path.
    pub path: String,
    /// What the workspace does to it.
    pub status: Status,
    /// How its content differs.
    pub diff: DiffContent,
    /// Present where the file kind differs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind_change: Option<KindChange>,
}

/// The differences of a workspace against its base.
#[derive(Debug, Serialize, Deserialize)]
pub struct Diff {
    /// One entry for each changed path.
    pub diffs: Vec<PathDiff>,
}

/// A file of a changeset's tree.
#[derive(Debug, Serialize, Deserialize)]
pub struct ListedFile {
    /// The path.
    pub path: String,
    /// The kind of the file.
    pub kind: Kind,
    /// The address of its bytes.
    pub content: String,
    /// Where its bytes can be downloaded.
    pub download: TransferGrant,
}

/// The files of a changeset's tree.
#[derive(Debug, Serialize, Deserialize)]
pub struct Files {
    /// The files, in path order.
    pub files: Vec<ListedFile>,
}

/// What to run, in the caller's workspace.
#[derive(Debug, Serialize, Deserialize)]
pub struct StartRun {
    /// What the shell runs.
    pub command: String,
    /// The directory the files sent are limited to; the whole workspace where absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    /// Paths where the run's changes may add files.
    pub add: Vec<String>,
    /// Paths whose files make the run's output tree.
    pub outputs: Vec<String>,
}

/// How far a run has come.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    /// Waiting for a runner.
    Queued,
    /// A runner holds it.
    Running,
    /// It has ended.
    Ended,
}

/// How the command ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunOutcome {
    /// The command exited with a code.
    Exited,
    /// The run passed its deadline.
    TimedOut,
    /// The run was cancelled.
    Cancelled,
    /// The executor failed.
    ExecutorFailed,
}

/// A stream a runner reported, with permission to download it.
#[derive(Debug, Serialize, Deserialize)]
pub struct RunStreamRead {
    /// The address of the stream's bytes.
    pub address: String,
    /// Whether the stream was cut at its bound.
    pub cut: bool,
    /// Where its bytes can be downloaded.
    pub download: TransferGrant,
}

/// How many objects a runner fetched and how many it held.
#[derive(Debug, Serialize, Deserialize)]
pub struct ObjectCounts {
    /// Objects the runner fetched.
    pub downloaded: u64,
    /// Objects the runner already held.
    pub from_cache: u64,
}

/// What a run changed at a path.
#[derive(Debug, Serialize, Deserialize)]
pub struct RunChange {
    /// The path.
    pub path: String,
    /// The file's kind, or that it was deleted.
    pub kind: RunChangeKind,
    /// The address of the file's bytes; null where it was deleted.
    pub address: Option<String>,
}

/// What a run did to a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunChangeKind {
    /// A regular file.
    Regular,
    /// A file with the executable bit.
    Executable,
    /// The file is gone.
    Deleted,
}

/// Whether a run's changes reached the workspace.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteBackOutcome {
    /// They were written.
    Written,
    /// They were not.
    NotWritten,
}

/// Why a run's changes were not written.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotWrittenReason {
    /// The workspace's instance closed.
    InstanceClosed,
    /// The workspace changed while the run ran.
    WorkspaceMoved,
    /// The caller may not write a changed path.
    WriteDenied,
    /// The run timed out.
    TimedOut,
    /// The run was cancelled.
    Cancelled,
    /// The executor failed.
    ExecutorFailed,
}

impl NotWrittenReason {
    /// The reason as the API writes it.
    #[must_use]
    pub const fn text(self) -> &'static str {
        match self {
            Self::InstanceClosed => "instance_closed",
            Self::WorkspaceMoved => "workspace_moved",
            Self::WriteDenied => "write_denied",
            Self::TimedOut => "timed_out",
            Self::Cancelled => "cancelled",
            Self::ExecutorFailed => "executor_failed",
        }
    }
}

/// What became of a run's changes.
#[derive(Debug, Serialize, Deserialize)]
pub struct RunWriteBack {
    /// Whether they were written.
    pub outcome: WriteBackOutcome,
    /// Why not, where they were not.
    pub reason: Option<NotWrittenReason>,
    /// The paths written, or those a refusal names.
    pub paths: Vec<String>,
}

/// How a run ended.
#[derive(Debug, Serialize, Deserialize)]
pub struct RunResult {
    /// How the command ended.
    pub outcome: RunOutcome,
    /// The command's exit code, where it exited.
    pub exit_code: Option<i32>,
    /// The command's standard output, where it started.
    pub stdout: Option<RunStreamRead>,
    /// The command's standard error, where it started.
    pub stderr: Option<RunStreamRead>,
    /// The object counts.
    pub objects: ObjectCounts,
    /// The address of the output tree.
    pub output_tree: Option<String>,
    /// What the command changed.
    pub changes: Vec<RunChange>,
    /// Sent or added paths that hold anything but a regular file.
    pub refused: Vec<String>,
    /// What became of the changes.
    pub write_back: RunWriteBack,
}

/// A run of a workspace.
#[derive(Debug, Serialize, Deserialize)]
pub struct Run {
    /// The run's identifier.
    pub id: u64,
    /// The workspace's name.
    pub draft: String,
    /// The workspace instance.
    pub instance: u64,
    /// The workspace's version when the run started.
    pub version: u64,
    /// What the shell ran.
    pub command: String,
    /// The directory the files sent were limited to.
    pub prefix: String,
    /// How far the run has come.
    pub state: RunState,
    /// When the runner's lease lapses.
    pub lease_expires_at: Option<i64>,
    /// When the run times out.
    pub deadline: Option<i64>,
    /// How it ended; null until it has.
    pub result: Option<RunResult>,
}

/// A file of a run's output tree.
#[derive(Debug, Serialize, Deserialize)]
pub struct RunFile {
    /// The path.
    pub path: String,
    /// The address of its bytes.
    pub address: String,
    /// Whether it has the executable bit.
    pub executable: bool,
    /// Where its bytes can be downloaded.
    pub download: TransferGrant,
}

/// The files of a run's output tree.
#[derive(Debug, Serialize, Deserialize)]
pub struct RunFiles {
    /// The files, in path order.
    pub files: Vec<RunFile>,
}
