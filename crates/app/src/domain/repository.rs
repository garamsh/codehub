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

    /// The number the identifier was made from; [`RepositoryId::new`] of it is this identifier.
    #[must_use]
    pub const fn value(self) -> u64 {
        self.0
    }
}

impl fmt::Display for RepositoryId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_the_value_it_was_made_from() {
        assert_eq!(RepositoryId::new(7).value(), 7);
        assert_eq!(
            RepositoryId::new(RepositoryId::new(u64::MAX).value()),
            RepositoryId::new(u64::MAX)
        );
    }
}
