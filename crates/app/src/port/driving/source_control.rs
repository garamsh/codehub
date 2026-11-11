//! Source control: drafts landed on `main`, and a path's history read back.

use std::fmt;

use crate::domain::address::Address;
use crate::domain::file_version::FileVersionNode;
use crate::domain::path::Path;
use crate::use_case::history::HistoryError;
use crate::use_case::land::LandError;

/// What the outside asks of the application's source control.
pub trait SourceControl {
    /// Lands the draft stored under `draft` on `main` and returns the landed changeset's
    /// identifier.
    ///
    /// # Errors
    ///
    /// [`SourceControlError::Land`], naming why the draft did not land; nothing moves `main` on
    /// an error.
    fn land(
        &self,
        draft: Address,
    ) -> impl Future<Output = Result<Address, SourceControlError>> + Send;

    /// The versions of `path` as of the changeset `at`, newest first.
    ///
    /// # Errors
    ///
    /// [`SourceControlError::History`], naming why the history could not be read.
    fn history(
        &self,
        at: Address,
        path: &Path,
    ) -> impl Future<Output = Result<Vec<FileVersionNode>, SourceControlError>> + Send;
}

/// Why source control did not do what it was asked.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SourceControlError {
    /// The draft did not land.
    Land(LandError),
    /// The history could not be read.
    History(HistoryError),
}

impl fmt::Display for SourceControlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Land(error) => write!(f, "landing failed: {error}"),
            Self::History(error) => write!(f, "history failed: {error}"),
        }
    }
}

impl std::error::Error for SourceControlError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Land(error) => Some(error),
            Self::History(error) => Some(error),
        }
    }
}

impl From<LandError> for SourceControlError {
    fn from(error: LandError) -> Self {
        Self::Land(error)
    }
}

impl From<HistoryError> for SourceControlError {
    fn from(error: HistoryError) -> Self {
        Self::History(error)
    }
}
