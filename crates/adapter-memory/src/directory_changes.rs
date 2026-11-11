use std::sync::Mutex;

use app::port::driven::directory_changes::{DirectoryChange, DirectoryChanges};

use crate::lock;

/// Directory changes recorded in memory, in the order they were reported.
#[derive(Default)]
pub struct MemoryDirectoryChanges {
    changes: Mutex<Vec<DirectoryChange>>,
}

impl MemoryDirectoryChanges {
    /// Every change reported so far, in order.
    #[must_use]
    pub fn reported(&self) -> Vec<DirectoryChange> {
        lock(&self.changes).clone()
    }
}

impl DirectoryChanges for MemoryDirectoryChanges {
    async fn report(&self, change: &DirectoryChange) {
        lock(&self.changes).push(change.clone());
    }
}
