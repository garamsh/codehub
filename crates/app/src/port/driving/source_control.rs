//! Source control: objects uploaded, drafts staged and landed on `main`, and files and history
//! read back, each within one repository. A draft is also the caller's workspace (ADR 0048): opened,
//! described, snapshotted, restored and diffed.

use std::collections::BTreeMap;
use std::fmt;

use crate::domain::address::Address;
use crate::domain::changeset::Change;
use crate::domain::derivation::DerivationError;
use crate::domain::diff::{ContentDiff, KindChange};
use crate::domain::draft::{ChangeStatus, Edit};
use crate::domain::encoding::EncodeError;
use crate::domain::file::FileKind;
use crate::domain::file_version::{FileVersionError, FileVersionNode};
use crate::domain::merge::FileConflict;
use crate::domain::name::DraftName;
use crate::domain::path::{Path, Prefix};
use crate::domain::principal::Caller;
use crate::domain::repository::RepositoryId;
use crate::domain::tree::DeriveError;
use crate::port::driven::changeset_log::ChangesetLogError;
use crate::port::driven::derived_data::DerivedDataError;
use crate::port::driven::draft_store::DraftStoreError;
use crate::port::driven::object_store::{Grant, ObjectStoreError};
use crate::port::driving::access::AccessError;

/// What the outside asks of the application's source control, for a caller. A caller with no read
/// on a repository is told it does not exist ([`AccessError::RepositoryNotFound`]), as for one
/// that does not.
pub trait SourceControl {
    /// A grant to upload the `size` bytes of the object `address` to `repository`.
    ///
    /// # Errors
    ///
    /// [`SourceControlError::Access`] where `caller` may not write to `repository`, and
    /// [`SourceControlError::Upload`], naming why no grant was issued; a size above the server's
    /// maximum is one.
    fn request_upload(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        address: Address,
        size: u64,
    ) -> impl Future<Output = Result<Grant, SourceControlError>> + Send;

    /// Records `change` at `path` in the caller's draft `draft`, creating the draft on `main` as it
    /// stands now where it does not exist. A file's content must already be uploaded.
    ///
    /// # Errors
    ///
    /// [`SourceControlError::Access`] where `caller` may not write `path`, and
    /// [`SourceControlError::Stage`], naming why the change was not staged.
    fn stage(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        draft: &DraftName,
        path: Path,
        change: Change,
    ) -> impl Future<Output = Result<(), SourceControlError>> + Send;

    /// The caller's workspace `draft`, created with its base at `main` as it stands now, or with
    /// no base where `main` is unset, when the caller has none of that name; a workspace the caller
    /// has keeps its own base.
    ///
    /// # Errors
    ///
    /// [`SourceControlError::Access`] where `caller` may not write to `repository` or may not read
    /// a path the workspace changes, and [`SourceControlError::Workspace`], naming why it could not
    /// be read.
    fn open_workspace(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        draft: &DraftName,
    ) -> impl Future<Output = Result<Workspace, SourceControlError>> + Send;

    /// The caller's workspace `draft`: its base, its description and each path it changes.
    ///
    /// # Errors
    ///
    /// [`SourceControlError::Access`] where `caller` may not read a path the workspace changes, and
    /// [`SourceControlError::Workspace`], naming why it could not be read; a missing workspace is
    /// [`DraftStoreError::NotFound`].
    fn workspace(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        draft: &DraftName,
    ) -> impl Future<Output = Result<Workspace, SourceControlError>> + Send;

    /// Sets the description of the caller's workspace `draft`.
    ///
    /// # Errors
    ///
    /// [`SourceControlError::Access`] where `caller` may not write to `repository`, and
    /// [`SourceControlError::Workspace`] with [`DraftStoreError::NotFound`] where the caller has no
    /// such workspace.
    fn describe(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        draft: &DraftName,
        description: String,
    ) -> impl Future<Output = Result<(), SourceControlError>> + Send;

    /// Applies every edit to the caller's workspace `draft`, or none of them. A file's content
    /// must already be uploaded.
    ///
    /// # Errors
    ///
    /// [`SourceControlError::Access`] naming every path `caller` may not write, and
    /// [`SourceControlError::Workspace`], naming why the snapshot was not applied; a file whose
    /// content was not uploaded is [`WorkspaceError::ObjectMissing`].
    fn snapshot(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        draft: &DraftName,
        edits: BTreeMap<Path, Edit>,
    ) -> impl Future<Output = Result<(), SourceControlError>> + Send;

    /// Returns `path` of the caller's workspace `draft` to the base's file, or to none where the
    /// base holds none. A path the workspace does not change is left as it is.
    ///
    /// # Errors
    ///
    /// [`SourceControlError::Access`] where `caller` may not write `path`, and
    /// [`SourceControlError::Workspace`] with [`DraftStoreError::NotFound`] where the caller has no
    /// such workspace.
    fn restore(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        draft: &DraftName,
        path: &Path,
    ) -> impl Future<Output = Result<(), SourceControlError>> + Send;

    /// The difference of each path the caller's workspace `draft` changes, or of `path` alone where
    /// it is given, from the base to the workspace.
    ///
    /// # Errors
    ///
    /// [`SourceControlError::Access`] where `caller` may not read a path the workspace changes, and
    /// [`SourceControlError::Workspace`], naming why it could not be computed.
    fn diff(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        draft: &DraftName,
        path: Option<&Path>,
    ) -> impl Future<Output = Result<Vec<PathDiff>, SourceControlError>> + Send;

    /// Every file under `prefix` in the changeset `changeset`, in path order.
    ///
    /// # Errors
    ///
    /// [`SourceControlError::Access`] where `caller` may not read one of the files, which refuses
    /// the whole listing, and [`SourceControlError::ListFiles`], naming why it could not be made;
    /// an unknown changeset is [`DerivedDataError::NotFound`].
    fn list_files(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        changeset: Address,
        prefix: &Prefix,
    ) -> impl Future<Output = Result<Vec<ListedFile>, SourceControlError>> + Send;

    /// Lands the caller's draft `draft` on `main`, removes it, and returns the landed changeset's
    /// identifier. The caller is the changeset's author; a landing with an empty message takes the
    /// draft's description as its message.
    ///
    /// # Errors
    ///
    /// [`SourceControlError::Land`], naming why the draft did not land, with
    /// [`AccessError::PathsDenied`] naming every path the draft changes that `caller` may not
    /// write; nothing moves `main` on an error, except that a failure to remove the draft is
    /// reported after `main` advanced.
    fn land(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        draft: &DraftName,
        landing: Landing,
    ) -> impl Future<Output = Result<Address, SourceControlError>> + Send;

    /// The file at `path` in `main`: its kind and a grant to download its content.
    ///
    /// # Errors
    ///
    /// [`SourceControlError::Access`] where `caller` may not read `path`, and
    /// [`SourceControlError::ReadFile`], naming why the file could not be read.
    fn read_file(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        path: &Path,
    ) -> impl Future<Output = Result<FileContent, SourceControlError>> + Send;

    /// The versions of `path` as of the changeset `at`, or as of `main` where `at` is `None`,
    /// newest first.
    ///
    /// # Errors
    ///
    /// [`SourceControlError::Access`] where `caller` may not read `path`, and
    /// [`SourceControlError::History`], naming why the history could not be read.
    fn history(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        at: Option<Address>,
        path: &Path,
    ) -> impl Future<Output = Result<Vec<FileVersionNode>, SourceControlError>> + Send;
}

/// Why source control did not do what it was asked.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SourceControlError {
    /// The caller may not do it.
    Access(AccessError),
    /// No upload grant was issued.
    Upload(UploadError),
    /// The change was not staged.
    Stage(StageError),
    /// The draft did not land.
    Land(LandError),
    /// The file could not be read.
    ReadFile(ReadFileError),
    /// The history could not be read.
    History(HistoryError),
    /// The workspace could not be read or changed.
    Workspace(WorkspaceError),
    /// The files could not be listed.
    ListFiles(ListFilesError),
}

impl fmt::Display for SourceControlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Access(error) => write!(f, "{error}"),
            Self::Upload(error) => write!(f, "upload failed: {error}"),
            Self::Stage(error) => write!(f, "staging failed: {error}"),
            Self::Land(error) => write!(f, "landing failed: {error}"),
            Self::ReadFile(error) => write!(f, "reading failed: {error}"),
            Self::History(error) => write!(f, "history failed: {error}"),
            Self::Workspace(error) => write!(f, "workspace failed: {error}"),
            Self::ListFiles(error) => write!(f, "listing failed: {error}"),
        }
    }
}

impl std::error::Error for SourceControlError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Access(error) => Some(error),
            Self::Upload(error) => Some(error),
            Self::Stage(error) => Some(error),
            Self::Land(error) => Some(error),
            Self::ReadFile(error) => Some(error),
            Self::History(error) => Some(error),
            Self::Workspace(error) => Some(error),
            Self::ListFiles(error) => Some(error),
        }
    }
}

impl From<AccessError> for SourceControlError {
    fn from(error: AccessError) -> Self {
        Self::Access(error)
    }
}

impl From<UploadError> for SourceControlError {
    fn from(error: UploadError) -> Self {
        Self::Upload(error)
    }
}

impl From<StageError> for SourceControlError {
    fn from(error: StageError) -> Self {
        Self::Stage(error)
    }
}

impl From<LandError> for SourceControlError {
    fn from(error: LandError) -> Self {
        Self::Land(error)
    }
}

impl From<ReadFileError> for SourceControlError {
    fn from(error: ReadFileError) -> Self {
        Self::ReadFile(error)
    }
}

impl From<HistoryError> for SourceControlError {
    fn from(error: HistoryError) -> Self {
        Self::History(error)
    }
}

impl From<WorkspaceError> for SourceControlError {
    fn from(error: WorkspaceError) -> Self {
        Self::Workspace(error)
    }
}

impl From<ListFilesError> for SourceControlError {
    fn from(error: ListFilesError) -> Self {
        Self::ListFiles(error)
    }
}

/// What a landing records beyond the draft's changes.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Landing {
    /// Why.
    pub message: String,
    /// When, in seconds since 1970-01-01T00:00:00Z.
    pub timestamp: i64,
}

/// A file as `main` holds it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FileContent {
    /// The file's mode.
    pub kind: FileKind,
    /// A grant to download the file's bytes.
    pub download: Grant,
}

/// Why a path conflicts.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PathConflict {
    /// The two sides' versions of the file do not merge.
    File(FileConflict),
    /// The path holds a file where the other side has a directory, or lies under one.
    FileAgainstDirectory,
}

/// Why a draft did not land.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum LandError {
    /// Paths conflict with what `main` changed since the draft's base; each with its reason.
    Conflicts(BTreeMap<Path, PathConflict>),
    /// `main` moved between the landing's read and its compare-and-swap; the landing was not
    /// applied.
    MainMoved {
        /// Where `main` points.
        current: Option<Address>,
    },
    /// The draft's base is not `main` or a first-parent ancestor of it.
    BaseNotOnMain,
    /// The draft's tree could not be derived.
    Tree(DerivationError<DeriveError>),
    /// The object store failed.
    Objects(ObjectStoreError),
    /// The changeset log failed.
    Log(ChangesetLogError),
    /// Derived data failed.
    Derived(DerivedDataError),
    /// The draft store failed.
    Drafts(DraftStoreError),
    /// The landed changeset has no encoding.
    Encode(EncodeError),
    /// The caller may not land the draft.
    Access(AccessError),
    /// The draft landed and `main` advanced to `landed`, but its checks were not queued; carries
    /// why. The landing is not undone.
    ChecksNotQueued {
        /// The changeset that landed.
        landed: Address,
        /// Why queuing its checks failed.
        cause: String,
    },
}

impl fmt::Display for LandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Conflicts(paths) => write!(f, "{} conflicting paths", paths.len()),
            Self::MainMoved {
                current: Some(current),
            } => write!(f, "main moved to {current}"),
            Self::MainMoved { current: None } => write!(f, "main was unset"),
            Self::BaseNotOnMain => write!(f, "the draft's base is not on main"),
            Self::Tree(error) => write!(f, "{error}"),
            Self::Objects(error) => write!(f, "{error}"),
            Self::Log(error) => write!(f, "{error}"),
            Self::Derived(error) => write!(f, "{error}"),
            Self::Drafts(error) => write!(f, "{error}"),
            Self::Encode(error) => write!(f, "{error}"),
            Self::Access(error) => write!(f, "{error}"),
            Self::ChecksNotQueued { landed, cause } => write!(
                f,
                "changeset {landed} landed and its checks were not queued: {cause}"
            ),
        }
    }
}

impl std::error::Error for LandError {}

impl From<ObjectStoreError> for LandError {
    fn from(error: ObjectStoreError) -> Self {
        Self::Objects(error)
    }
}

impl From<ChangesetLogError> for LandError {
    fn from(error: ChangesetLogError) -> Self {
        Self::Log(error)
    }
}

impl From<DerivedDataError> for LandError {
    fn from(error: DerivedDataError) -> Self {
        Self::Derived(error)
    }
}

impl From<DraftStoreError> for LandError {
    fn from(error: DraftStoreError) -> Self {
        Self::Drafts(error)
    }
}

impl From<AccessError> for LandError {
    fn from(error: AccessError) -> Self {
        Self::Access(error)
    }
}

impl From<EncodeError> for LandError {
    fn from(error: EncodeError) -> Self {
        Self::Encode(error)
    }
}

/// Why a file could not be read.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ReadFileError {
    /// `main` holds no file at the path.
    NotFound(Path),
    /// The object store failed.
    Objects(ObjectStoreError),
    /// The changeset log failed.
    Log(ChangesetLogError),
    /// Derived data failed.
    Derived(DerivedDataError),
}

impl fmt::Display for ReadFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(path) => write!(f, "main holds no file {path}"),
            Self::Objects(error) => write!(f, "{error}"),
            Self::Log(error) => write!(f, "{error}"),
            Self::Derived(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for ReadFileError {}

impl From<ObjectStoreError> for ReadFileError {
    fn from(error: ObjectStoreError) -> Self {
        Self::Objects(error)
    }
}

impl From<ChangesetLogError> for ReadFileError {
    fn from(error: ChangesetLogError) -> Self {
        Self::Log(error)
    }
}

impl From<DerivedDataError> for ReadFileError {
    fn from(error: DerivedDataError) -> Self {
        Self::Derived(error)
    }
}

/// Why a history could not be read.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum HistoryError {
    /// A changeset's file-version nodes could not be derived.
    FileVersions(DerivationError<FileVersionError>),
    /// The changeset log failed.
    Log(ChangesetLogError),
    /// Derived data failed.
    Derived(DerivedDataError),
}

impl fmt::Display for HistoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FileVersions(error) => write!(f, "{error}"),
            Self::Log(error) => write!(f, "{error}"),
            Self::Derived(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for HistoryError {}

impl From<DerivationError<FileVersionError>> for HistoryError {
    fn from(error: DerivationError<FileVersionError>) -> Self {
        Self::FileVersions(error)
    }
}

impl From<ChangesetLogError> for HistoryError {
    fn from(error: ChangesetLogError) -> Self {
        Self::Log(error)
    }
}

impl From<DerivedDataError> for HistoryError {
    fn from(error: DerivedDataError) -> Self {
        Self::Derived(error)
    }
}

/// Why a change was not staged.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum StageError {
    /// The object store holds no object with this address; it was never uploaded.
    ObjectMissing(Address),
    /// The object store failed.
    Objects(ObjectStoreError),
    /// The changeset log failed.
    Log(ChangesetLogError),
    /// The draft store failed.
    Drafts(DraftStoreError),
}

impl fmt::Display for StageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ObjectMissing(address) => write!(f, "object {address} was not uploaded"),
            Self::Objects(error) => write!(f, "{error}"),
            Self::Log(error) => write!(f, "{error}"),
            Self::Drafts(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for StageError {}

impl From<ObjectStoreError> for StageError {
    fn from(error: ObjectStoreError) -> Self {
        Self::Objects(error)
    }
}

impl From<ChangesetLogError> for StageError {
    fn from(error: ChangesetLogError) -> Self {
        Self::Log(error)
    }
}

impl From<DraftStoreError> for StageError {
    fn from(error: DraftStoreError) -> Self {
        Self::Drafts(error)
    }
}

/// Why no upload grant was issued.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum UploadError {
    /// The object is larger than the maximum the server accepts.
    TooLarge {
        /// The object's size, in bytes.
        size: u64,
        /// The largest size accepted, in bytes.
        max_size: u64,
    },
    /// The object store failed.
    Objects(ObjectStoreError),
}

impl fmt::Display for UploadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge { size, max_size } => {
                write!(f, "{size} bytes exceed the {max_size} byte maximum")
            }
            Self::Objects(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for UploadError {}

impl From<ObjectStoreError> for UploadError {
    fn from(error: ObjectStoreError) -> Self {
        Self::Objects(error)
    }
}

/// A workspace as the caller sees it: a draft with its base, its description and its changes
/// against the base.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Workspace {
    /// The changeset the workspace was opened on; `None` where `main` was unset.
    pub base: Option<Address>,
    /// What the work is for; empty until set.
    pub description: String,
    /// The paths the workspace changes, in path order. A path whose change leaves the base's file
    /// as it is is not listed.
    pub changes: Vec<ChangedPath>,
}

/// One path a workspace changes.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ChangedPath {
    /// The path.
    pub path: Path,
    /// What the workspace does to the path against the base.
    pub status: ChangeStatus,
    /// The file's mode: the workspace's for an added or modified path, the base's for a deleted
    /// one.
    pub kind: FileKind,
    /// A grant to download the workspace's content; none for a deleted path.
    pub download: Option<Grant>,
}

/// The difference of one changed path from the base to the workspace.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PathDiff {
    /// The path.
    pub path: Path,
    /// What the workspace does to the path against the base.
    pub status: ChangeStatus,
    /// How the content differs.
    pub diff: ContentDiff,
    /// The change of file kind, where the path is on both sides and its kind differs, whatever
    /// the content does.
    pub kind_change: Option<KindChange>,
}

/// A file of a changeset's tree.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ListedFile {
    /// The file's path.
    pub path: Path,
    /// The file's mode.
    pub kind: FileKind,
    /// The content's address.
    pub content: Address,
    /// A grant to download the content.
    pub download: Grant,
}

/// Why a workspace could not be read or changed.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum WorkspaceError {
    /// The object store holds no object with this address; it was never uploaded.
    ObjectMissing(Address),
    /// The object store failed.
    Objects(ObjectStoreError),
    /// The changeset log failed.
    Log(ChangesetLogError),
    /// Derived data failed.
    Derived(DerivedDataError),
    /// The draft store failed, or holds no such workspace.
    Drafts(DraftStoreError),
}

impl fmt::Display for WorkspaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ObjectMissing(address) => write!(f, "object {address} was not uploaded"),
            Self::Objects(error) => write!(f, "{error}"),
            Self::Log(error) => write!(f, "{error}"),
            Self::Derived(error) => write!(f, "{error}"),
            Self::Drafts(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for WorkspaceError {}

impl From<ObjectStoreError> for WorkspaceError {
    fn from(error: ObjectStoreError) -> Self {
        Self::Objects(error)
    }
}

impl From<ChangesetLogError> for WorkspaceError {
    fn from(error: ChangesetLogError) -> Self {
        Self::Log(error)
    }
}

impl From<DerivedDataError> for WorkspaceError {
    fn from(error: DerivedDataError) -> Self {
        Self::Derived(error)
    }
}

impl From<DraftStoreError> for WorkspaceError {
    fn from(error: DraftStoreError) -> Self {
        Self::Drafts(error)
    }
}

/// Why a changeset's files could not be listed.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ListFilesError {
    /// The object store failed.
    Objects(ObjectStoreError),
    /// Derived data failed, or holds no tree for the changeset.
    Derived(DerivedDataError),
}

impl fmt::Display for ListFilesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Objects(error) => write!(f, "{error}"),
            Self::Derived(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for ListFilesError {}

impl From<ObjectStoreError> for ListFilesError {
    fn from(error: ObjectStoreError) -> Self {
        Self::Objects(error)
    }
}

impl From<DerivedDataError> for ListFilesError {
    fn from(error: DerivedDataError) -> Self {
        Self::Derived(error)
    }
}
