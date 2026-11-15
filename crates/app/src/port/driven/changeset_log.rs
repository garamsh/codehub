//! The changeset log: a repository's changesets and the refs naming them (ADR 0012).

use std::fmt;

use crate::domain::address::Address;
use crate::domain::changeset::Changeset;
use crate::domain::encoding::EncodeError;
use crate::domain::repository::RepositoryId;

/// A named ref of a repository.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Ref {
    /// What agents base their work on, and what landing advances.
    Main,
    /// The newest changeset on `main` whose blocking checks have all passed; it only moves
    /// forward along `main`.
    LastGreen,
}

/// Holds the changesets of each repository and moves its refs. Each operation reads or writes the
/// log of the repository it is given.
#[async_trait::async_trait]
pub trait ChangesetLog: Send + Sync {
    /// Appends `changeset` and returns its identifier, its address. Appending moves no ref.
    ///
    /// # Errors
    ///
    /// [`ChangesetLogError::Encode`] where the changeset has no encoding.
    async fn append(
        &self,
        repository: RepositoryId,
        changeset: Changeset,
    ) -> Result<Address, ChangesetLogError>;

    /// The changeset identified by `id`.
    ///
    /// # Errors
    ///
    /// [`ChangesetLogError::NotFound`] where the log holds no such changeset.
    async fn get(
        &self,
        repository: RepositoryId,
        id: Address,
    ) -> Result<Changeset, ChangesetLogError>;

    /// The changeset `name` points at; `None` before it is first set.
    ///
    /// # Errors
    ///
    /// The log's [`ChangesetLogError`].
    async fn read_ref(
        &self,
        repository: RepositoryId,
        name: Ref,
    ) -> Result<Option<Address>, ChangesetLogError>;

    /// Points `name` at `new` where it still points at `expected`, and leaves it unchanged
    /// otherwise.
    ///
    /// # Errors
    ///
    /// [`ChangesetLogError::RefMoved`] where `name` no longer points at `expected`.
    async fn compare_and_swap(
        &self,
        repository: RepositoryId,
        name: Ref,
        expected: Option<Address>,
        new: Address,
    ) -> Result<(), ChangesetLogError>;
}

/// Why the changeset log refused or did not answer.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ChangesetLogError {
    /// The log holds no changeset with this identifier.
    NotFound(Address),
    /// A compare-and-swap found the ref pointing elsewhere; the ref was not moved.
    RefMoved {
        /// Where the ref points.
        current: Option<Address>,
    },
    /// A changeset has no encoding, so no identifier.
    Encode(EncodeError),
    /// The log's backend failed; carries a description.
    Backend(String),
}

impl fmt::Display for ChangesetLogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(id) => write!(f, "no changeset {id}"),
            Self::RefMoved {
                current: Some(current),
            } => write!(f, "the ref moved to {current}"),
            Self::RefMoved { current: None } => write!(f, "the ref was unset"),
            Self::Encode(error) => write!(f, "{error}"),
            Self::Backend(description) => write!(f, "changeset log failed: {description}"),
        }
    }
}

impl std::error::Error for ChangesetLogError {}
