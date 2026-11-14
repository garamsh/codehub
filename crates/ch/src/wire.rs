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
#[serde(rename_all = "lowercase")]
pub enum Access {
    /// Read the prefix.
    Read,
    /// Write, and so read, the prefix.
    Write,
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
