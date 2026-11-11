//! The identifier the repository directory assigns to a repository.

use std::fmt;

/// A repository inside the core: opaque, assigned by the repository directory, and what every
/// storage port is scoped by.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct RepositoryId(u64);

impl RepositoryId {
    /// The identifier a directory assigns from `value`; distinct values are distinct repositories.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }
}

impl fmt::Display for RepositoryId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
