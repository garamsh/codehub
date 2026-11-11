//! Drafts: pending work, identified by (repository, principal, draft name) and staged path by
//! path (ADR 0012).

use std::collections::BTreeMap;
use std::fmt;

use super::address::Address;
use super::changeset::Change;
use super::name::DraftName;
use super::path::Path;
use super::principal::Principal;

/// A draft inside its repository: a principal's draft of that name.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct DraftId {
    /// Whose draft it is.
    pub principal: Principal,
    /// What the principal calls it.
    pub name: DraftName,
}

impl fmt::Display for DraftId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.principal, self.name)
    }
}

/// A staged draft: the changeset it was based on and the changes staged so far.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Draft {
    /// `main` when the draft was first staged; `None` where `main` was unset.
    pub base: Option<Address>,
    /// The staged paths, each with its latest change.
    pub changes: BTreeMap<Path, Change>,
}
