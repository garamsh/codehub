//! The directory changes: what the application reports of the organisations and repositories it
//! creates, so an outside layer can learn of them (ADR 0030, ADR 0044).
//!
//! A repository change is delivered at least once, from the directory outbox by the relay. An
//! organisation change is reported once by the use case, with no retry.

use crate::domain::name::{OrganisationName, RepositoryName};
use crate::domain::repository::RepositoryId;

/// What happened to a repository.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ChangeKind {
    /// The repository was created.
    Created,
    /// The repository was deleted; nothing writes it until codehub has a deletion operation.
    Deleted,
}

/// One change to a repository, as the outbox holds it until it is delivered.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RepositoryChange {
    /// The organisation holding the repository.
    pub organisation: OrganisationName,
    /// The repository's name.
    pub name: RepositoryName,
    /// The repository's identifier.
    pub repository: RepositoryId,
    /// What happened.
    pub kind: ChangeKind,
    /// Rises with every change to the repository, from 1 at its creation; never reused. A receiver
    /// applies a change only where its sequence is above the last it applied.
    pub sequence: u64,
}

/// A change to the organisations the directory holds.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum OrganisationChange {
    /// An organisation was created.
    Created(OrganisationName),
    /// An organisation was deleted; nothing reports it until codehub has a deletion operation.
    Deleted(OrganisationName),
}

/// Receives the changes.
pub trait DirectoryChanges {
    /// Delivers `change`. Answers whether the receiver accepted it; an adapter that cannot deliver
    /// it, or whose receiver refuses it, answers `false`, and the relay delivers it again. A
    /// change can arrive more than once.
    fn repository_changed(&self, change: &RepositoryChange) -> impl Future<Output = bool> + Send;

    /// Reports `change`, after it succeeded and never on failure. Delivery is at most once: an
    /// adapter that fails to deliver keeps that to itself, and the caller neither waits on nor
    /// retries it.
    fn organisation_changed(&self, change: &OrganisationChange) -> impl Future<Output = ()> + Send;
}

/// Delivers nowhere: every repository change is accepted at once.
impl DirectoryChanges for () {
    async fn repository_changed(&self, _change: &RepositoryChange) -> bool {
        true
    }

    async fn organisation_changed(&self, _change: &OrganisationChange) {}
}
