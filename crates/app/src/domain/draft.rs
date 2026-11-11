//! Drafts: pending work, identified by (repository, principal, draft name) and staged path by
//! path (ADR 0012).

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use super::address::Address;
use super::changeset::Change;
use super::name::DraftName;
use super::path::Path;

/// Why text is not a principal: it is empty.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct EmptyPrincipal;

impl fmt::Display for EmptyPrincipal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a principal is not empty")
    }
}

impl std::error::Error for EmptyPrincipal {}

/// Who stages and lands a draft. A plain value until the caller's identity is established.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Principal(String);

impl Principal {
    /// The principal as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for Principal {
    type Err = EmptyPrincipal;

    fn from_str(text: &str) -> Result<Self, EmptyPrincipal> {
        if text.is_empty() {
            Err(EmptyPrincipal)
        } else {
            Ok(Self(text.to_owned()))
        }
    }
}

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
        write!(f, "{}/{}", self.principal.as_str(), self.name)
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
