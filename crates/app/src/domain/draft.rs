//! Drafts: pending work, identified by (repository, principal, draft name) and staged path by
//! path (ADR 0012). A draft is the caller's workspace, with a description (ADR 0048).

use std::collections::BTreeMap;
use std::fmt;

use super::address::Address;
use super::changeset::Change;
use super::file::File;
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

/// A staged draft: the changeset it was based on, its description and the changes staged so far.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Draft {
    /// `main` when the draft was first staged; `None` where `main` was unset.
    pub base: Option<Address>,
    /// What the work is for; a landing without a message takes it as the message. Empty until set.
    pub description: String,
    /// The staged paths, each with its latest change.
    pub changes: BTreeMap<Path, Change>,
}

/// What a draft's change to a path does against the base's file at that path.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ChangeStatus {
    /// The base holds no file at the path and the draft writes one.
    Added,
    /// The base holds a file at the path and the draft writes a different one.
    Modified,
    /// The base holds a file at the path and the draft removes it.
    Deleted,
}

impl ChangeStatus {
    /// The status of `change` where the base holds `base` at the path; `None` where the change
    /// leaves the path as the base holds it.
    #[must_use]
    pub fn of(base: Option<File>, change: Change) -> Option<Self> {
        match (base, change) {
            (None, Change::Deletion) => None,
            (None, Change::File(_)) => Some(Self::Added),
            (Some(_), Change::Deletion) => Some(Self::Deleted),
            (Some(base), Change::File(file)) => (base != file).then_some(Self::Modified),
        }
    }
}

/// What a snapshot does to one path of a draft.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Edit {
    /// Records the change, replacing an earlier one to the path.
    Record(Change),
    /// Removes the path's change, so the path is as the base holds it.
    Restore,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::file::FileKind;

    fn file(content: &str, kind: FileKind) -> File {
        File {
            kind,
            content: Address::of(content.as_bytes()),
        }
    }

    #[test]
    fn a_change_is_added_modified_or_deleted_against_the_base_file() {
        let one = file("one", FileKind::Regular);
        let two = file("two", FileKind::Regular);
        assert_eq!(
            ChangeStatus::of(None, Change::File(one)),
            Some(ChangeStatus::Added)
        );
        assert_eq!(
            ChangeStatus::of(Some(one), Change::File(two)),
            Some(ChangeStatus::Modified)
        );
        assert_eq!(
            ChangeStatus::of(Some(one), Change::File(file("one", FileKind::Executable))),
            Some(ChangeStatus::Modified)
        );
        assert_eq!(
            ChangeStatus::of(Some(one), Change::Deletion),
            Some(ChangeStatus::Deleted)
        );
    }

    #[test]
    fn a_change_that_leaves_the_base_file_has_no_status() {
        let one = file("one", FileKind::Regular);
        assert_eq!(ChangeStatus::of(Some(one), Change::File(one)), None);
        assert_eq!(ChangeStatus::of(None, Change::Deletion), None);
    }
}
