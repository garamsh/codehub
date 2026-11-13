use std::sync::Mutex;

use app::port::driven::directory_changes::{
    DirectoryChanges, OrganisationChange, RepositoryChange,
};

use crate::lock;

/// A change the receiver was given.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ReceivedChange {
    /// An organisation change.
    Organisation(OrganisationChange),
    /// A repository change, as delivered; one delivered again is received again.
    Repository(RepositoryChange),
}

/// Directory changes recorded in memory, in the order they were received. It accepts every
/// repository change.
#[derive(Default)]
pub struct MemoryDirectoryChanges {
    changes: Mutex<Vec<ReceivedChange>>,
}

impl MemoryDirectoryChanges {
    /// Every change received so far, in order.
    #[must_use]
    pub fn reported(&self) -> Vec<ReceivedChange> {
        lock(&self.changes).clone()
    }
}

impl DirectoryChanges for MemoryDirectoryChanges {
    async fn repository_changed(&self, change: &RepositoryChange) -> bool {
        lock(&self.changes).push(ReceivedChange::Repository(change.clone()));
        true
    }

    async fn organisation_changed(&self, change: &OrganisationChange) {
        lock(&self.changes).push(ReceivedChange::Organisation(change.clone()));
    }
}
