//! Source control: objects uploaded, drafts staged and landed on `main`, and files and history
//! read back, each within one repository.

use std::fmt;

use crate::domain::address::Address;
use crate::domain::changeset::Change;
use crate::domain::draft::DraftId;
use crate::domain::file_version::FileVersionNode;
use crate::domain::path::Path;
use crate::domain::repository::RepositoryId;
use crate::port::driven::object_store::Grant;
use crate::use_case::history::HistoryError;
use crate::use_case::land::{LandError, Landing};
use crate::use_case::read_file::{FileContent, ReadFileError};
use crate::use_case::stage::StageError;
use crate::use_case::upload::UploadError;

/// What the outside asks of the application's source control.
pub trait SourceControl {
    /// A grant to upload the `size` bytes of the object `address` to `repository`.
    ///
    /// # Errors
    ///
    /// [`SourceControlError::Upload`], naming why no grant was issued; a size above the server's
    /// maximum is one.
    fn request_upload(
        &self,
        repository: RepositoryId,
        address: Address,
        size: u64,
    ) -> impl Future<Output = Result<Grant, SourceControlError>> + Send;

    /// Records `change` at `path` in the draft `draft`, creating the draft on `main` as it stands
    /// now where it does not exist. A file's content must already be uploaded.
    ///
    /// # Errors
    ///
    /// [`SourceControlError::Stage`], naming why the change was not staged.
    fn stage(
        &self,
        repository: RepositoryId,
        draft: &DraftId,
        path: Path,
        change: Change,
    ) -> impl Future<Output = Result<(), SourceControlError>> + Send;

    /// Lands the draft `draft` on `main`, removes it, and returns the landed changeset's
    /// identifier.
    ///
    /// # Errors
    ///
    /// [`SourceControlError::Land`], naming why the draft did not land; nothing moves `main` on an
    /// error.
    fn land(
        &self,
        repository: RepositoryId,
        draft: &DraftId,
        landing: Landing,
    ) -> impl Future<Output = Result<Address, SourceControlError>> + Send;

    /// The file at `path` in `main`: its kind and a grant to download its content.
    ///
    /// # Errors
    ///
    /// [`SourceControlError::ReadFile`], naming why the file could not be read.
    fn read_file(
        &self,
        repository: RepositoryId,
        path: &Path,
    ) -> impl Future<Output = Result<FileContent, SourceControlError>> + Send;

    /// The versions of `path` as of the changeset `at`, newest first.
    ///
    /// # Errors
    ///
    /// [`SourceControlError::History`], naming why the history could not be read.
    fn history(
        &self,
        repository: RepositoryId,
        at: Address,
        path: &Path,
    ) -> impl Future<Output = Result<Vec<FileVersionNode>, SourceControlError>> + Send;
}

/// Why source control did not do what it was asked.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SourceControlError {
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
            Self::Upload(error) => Some(error),
            Self::Stage(error) => Some(error),
            Self::Land(error) => Some(error),
            Self::ReadFile(error) => Some(error),
            Self::History(error) => Some(error),
        }
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
