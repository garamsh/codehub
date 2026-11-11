//! The changeset log: a workspace's changesets and the refs naming them (ADR 0012).

use std::fmt;

use crate::domain::address::Address;
use crate::domain::changeset::Changeset;
use crate::domain::encoding::EncodeError;

/// A named ref of the workspace.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Ref {
    /// What agents base their work on, and what landing advances.
    Main,
}

/// Holds the changesets of a workspace and moves its refs.
pub trait ChangesetLog {
    /// Appends `changeset` and returns its identifier, its address. Appending moves no ref.
    ///
    /// # Errors
    ///
    /// [`ChangesetLogError::Encode`] where the changeset has no encoding.
    fn append(
        &self,
        changeset: Changeset,
    ) -> impl Future<Output = Result<Address, ChangesetLogError>> + Send;

    /// The changeset identified by `id`.
    ///
    /// # Errors
    ///
    /// [`ChangesetLogError::NotFound`] where the log holds no such changeset.
    fn get(&self, id: Address)
    -> impl Future<Output = Result<Changeset, ChangesetLogError>> + Send;

    /// The changeset `name` points at; `None` before it is first set.
    ///
    /// # Errors
    ///
    /// The log's [`ChangesetLogError`].
    fn read_ref(
        &self,
        name: Ref,
    ) -> impl Future<Output = Result<Option<Address>, ChangesetLogError>> + Send;

    /// Points `name` at `new` where it still points at `expected`, and leaves it unchanged
    /// otherwise.
    ///
    /// # Errors
    ///
    /// [`ChangesetLogError::RefMoved`] where `name` no longer points at `expected`.
    fn compare_and_swap(
        &self,
        name: Ref,
        expected: Option<Address>,
        new: Address,
    ) -> impl Future<Output = Result<(), ChangesetLogError>> + Send;
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
        }
    }
}

impl std::error::Error for ChangesetLogError {}
