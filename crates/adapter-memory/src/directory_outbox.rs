use std::sync::{Arc, Mutex};

use app::domain::repository::RepositoryId;
use app::port::driven::directory_changes::RepositoryChange;
use app::port::driven::directory_outbox::{DirectoryOutbox, DirectoryOutboxError};

use crate::lock;
use crate::repository_directory::Directory;

/// The directory outbox in memory, over the entries a [`crate::MemoryRepositoryDirectory`] writes.
pub struct MemoryDirectoryOutbox {
    directory: Arc<Mutex<Directory>>,
}

impl MemoryDirectoryOutbox {
    pub(crate) const fn new(directory: Arc<Mutex<Directory>>) -> Self {
        Self { directory }
    }
}

/// Whether one of `repository`'s entries is under a lease that has not lapsed at `now`.
fn held(directory: &Directory, repository: RepositoryId, now: i64) -> bool {
    directory
        .outbox
        .iter()
        .any(|entry| entry.change.repository == repository && entry.lease_expires_at > now)
}

impl DirectoryOutbox for MemoryDirectoryOutbox {
    async fn claim(
        &self,
        repository: Option<RepositoryId>,
        now: i64,
        lease: i64,
    ) -> Result<Vec<RepositoryChange>, DirectoryOutboxError> {
        let mut directory = lock(&self.directory);
        let claimable = directory.outbox.iter().find_map(|entry| {
            let wanted = repository.is_none_or(|wanted| wanted == entry.change.repository);
            (wanted && !held(&directory, entry.change.repository, now))
                .then_some(entry.change.repository)
        });
        let Some(claimable) = claimable else {
            return Ok(Vec::new());
        };
        let mut claimed = Vec::new();
        for entry in &mut directory.outbox {
            if entry.change.repository == claimable {
                entry.lease_expires_at = now.saturating_add(lease);
                claimed.push(entry.change.clone());
            }
        }
        claimed.sort_by_key(|change| change.sequence);
        Ok(claimed)
    }

    async fn release(
        &self,
        repository: RepositoryId,
        until: i64,
    ) -> Result<(), DirectoryOutboxError> {
        for entry in &mut lock(&self.directory).outbox {
            if entry.change.repository == repository {
                entry.lease_expires_at = until;
            }
        }
        Ok(())
    }

    async fn delete(
        &self,
        repository: RepositoryId,
        sequence: u64,
    ) -> Result<(), DirectoryOutboxError> {
        lock(&self.directory).outbox.retain(|entry| {
            entry.change.repository != repository || entry.change.sequence != sequence
        });
        Ok(())
    }
}
