//! In-process implementations of the driven ports, holding their state in memory; tests
//! substitute them for the real technologies.

mod changeset_log;
mod derived_data;
mod draft_store;
mod object_store;
mod repository_directory;

use std::sync::{Mutex, MutexGuard, PoisonError};

pub use changeset_log::MemoryChangesetLog;
pub use derived_data::MemoryDerivedData;
pub use draft_store::MemoryDraftStore;
pub use object_store::MemoryObjectStore;
pub use repository_directory::MemoryRepositoryDirectory;

/// Locks `state`; a panic while it was held leaves no write half-applied, since every write is a
/// single map operation.
fn lock<T>(state: &Mutex<T>) -> MutexGuard<'_, T> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}
