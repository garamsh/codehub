//! The directory changes: what the application reports of the organisations and repositories it
//! creates, so an outside layer can learn of them (ADR 0030).

use crate::domain::name::{OrganisationName, RepositoryName};

/// A change to the organisations and repositories the directory holds.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum DirectoryChange {
    /// An organisation was created.
    OrganisationCreated(OrganisationName),
    /// An organisation was deleted; nothing reports it until codehub has a deletion operation.
    OrganisationDeleted(OrganisationName),
    /// A repository was created.
    RepositoryCreated {
        /// The organisation holding it.
        organisation: OrganisationName,
        /// The repository.
        repository: RepositoryName,
    },
    /// A repository was deleted; nothing reports it until codehub has a deletion operation.
    RepositoryDeleted {
        /// The organisation that held it.
        organisation: OrganisationName,
        /// The repository.
        repository: RepositoryName,
    },
}

/// Receives each change once, after it succeeded and never on failure.
pub trait DirectoryChanges {
    /// Reports `change`. Delivery is at most once: an adapter that fails to deliver keeps that to
    /// itself, and the caller neither waits on nor retries it.
    fn report(&self, change: &DirectoryChange) -> impl Future<Output = ()> + Send;
}

/// Reports nowhere.
impl DirectoryChanges for () {
    async fn report(&self, _change: &DirectoryChange) {}
}
