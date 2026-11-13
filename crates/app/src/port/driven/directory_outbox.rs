//! The directory outbox: the repository changes written in the transaction that made them, held
//! until they are delivered (ADR 0044).
//!
//! The repository directory writes the entries. This port is how the relay reads them: it claims
//! one repository's pending entries under a lease, so that no row lock is held while it delivers,
//! and deletes each entry once the receiver has accepted it. Every operation that depends on time
//! takes `now` in seconds since 1970-01-01T00:00:00Z; the outbox never reads a clock.

use std::fmt;

use crate::domain::repository::RepositoryId;
use crate::port::driven::directory_changes::RepositoryChange;

/// Holds the entries.
pub trait DirectoryOutbox {
    /// Claims every pending entry of one repository for `lease` seconds from `now`, and returns
    /// them in sequence order. The repository is `repository` where given, and otherwise the one
    /// whose oldest entry was written first. A repository is claimable where none of its entries is
    /// under an unexpired lease, so an expired lease lets another caller claim the entries again.
    /// Empty where no repository is claimable.
    ///
    /// # Errors
    ///
    /// [`DirectoryOutboxError::Backend`] where the outbox fails; nothing is then claimed.
    fn claim(
        &self,
        repository: Option<RepositoryId>,
        now: i64,
        lease: i64,
    ) -> impl Future<Output = Result<Vec<RepositoryChange>, DirectoryOutboxError>> + Send;

    /// Ends the claim on `repository`'s entries, which can be claimed again from `until`: a time
    /// not after `now` makes them claimable at once, and a later one holds them back.
    ///
    /// # Errors
    ///
    /// [`DirectoryOutboxError::Backend`] where the outbox fails.
    fn release(
        &self,
        repository: RepositoryId,
        until: i64,
    ) -> impl Future<Output = Result<(), DirectoryOutboxError>> + Send;

    /// Deletes the entry of `repository` with `sequence`, which was delivered. Deleting an entry
    /// that is not there is not an error.
    ///
    /// # Errors
    ///
    /// [`DirectoryOutboxError::Backend`] where the outbox fails.
    fn delete(
        &self,
        repository: RepositoryId,
        sequence: u64,
    ) -> impl Future<Output = Result<(), DirectoryOutboxError>> + Send;
}

/// Why the outbox did not answer.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum DirectoryOutboxError {
    /// The outbox's backend failed; carries a description.
    Backend(String),
}

impl fmt::Display for DirectoryOutboxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Backend(description) => write!(f, "directory outbox failed: {description}"),
        }
    }
}

impl std::error::Error for DirectoryOutboxError {}
