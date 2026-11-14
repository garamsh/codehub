//! Repositories held by the in-memory adapters, with drafts staged and landed through the driving
//! ports.

use test_support::{MemoryDeployment, Taught};

/// The in-memory adapters of one deployment, holding any number of repositories.
pub type Harness = MemoryDeployment<Taught>;
