//! In-process implementations of the driven ports, holding their state in memory; tests
//! substitute them for the real technologies.

mod authorisation_decision;
mod changeset_log;
mod check_definitions;
mod derived_data;
mod directory_changes;
mod draft_store;
mod identity_verification;
mod job_queue;
mod membership;
mod object_store;
mod repository_directory;
mod runner_registry;

use std::sync::{Mutex, MutexGuard, PoisonError};

pub use authorisation_decision::MemoryAuthorisationDecision;
pub use changeset_log::MemoryChangesetLog;
pub use check_definitions::MemoryCheckDefinitions;
pub use derived_data::MemoryDerivedData;
pub use directory_changes::MemoryDirectoryChanges;
pub use draft_store::MemoryDraftStore;
pub use identity_verification::MemoryIdentityVerification;
pub use job_queue::MemoryJobQueue;
pub use membership::MemoryMembership;
pub use object_store::MemoryObjectStore;
pub use repository_directory::MemoryRepositoryDirectory;
pub use runner_registry::MemoryRunnerRegistry;

/// Locks `state`; a panic while it was held leaves no write half-applied, since every write is a
/// single map operation.
fn lock<T>(state: &Mutex<T>) -> MutexGuard<'_, T> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}
