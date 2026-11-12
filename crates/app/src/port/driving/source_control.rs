//! Source control: objects uploaded, drafts staged and landed on `main`, and files and history
//! read back, each within one repository.

use std::collections::BTreeMap;
use std::fmt;

use crate::domain::address::Address;
use crate::domain::changeset::Change;
use crate::domain::derivation::DerivationError;
use crate::domain::encoding::EncodeError;
use crate::domain::file::FileKind;
use crate::domain::file_version::{FileVersionError, FileVersionNode};
use crate::domain::merge::FileConflict;
use crate::domain::name::DraftName;
use crate::domain::path::Path;
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

    /// Lands the caller's draft `draft` on `main`, removes it, and returns the landed changeset's
    /// identifier. The caller is the changeset's author.
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
