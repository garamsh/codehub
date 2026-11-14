//! Reverting a changeset: the delta that undoes it, the changeset that carries the delta, and
//! what is recorded of the attempt (ADR 0035).

use std::collections::{BTreeMap, BTreeSet};

use super::address::Address;
use super::changeset::{Change, Changeset};
use super::file::File;
use super::job::JobState;
use super::name::CheckName;
use super::path::Path;

/// The author of a changeset the server makes itself. No principal's text equals it, since a
/// principal's text holds a space.
pub const AUTHOR: &str = "codehub";

/// What came of reverting a changeset after one of its checks failed.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Revert {
    /// The revert landed on `main` as this changeset.
    Landed(Address),
    /// The revert conflicts with later changes and landed nothing; carries the conflicting
    /// paths.
    Conflicted(BTreeSet<Path>),
    /// The revert failed after it was reserved and landed nothing; carries why, in words an
    /// operator reads. It is not tried again.
    Failed {
        /// The error's text, cut to [`MAX_REASON`] characters.
        reason: String,
    },
}

/// The most characters of an error's text a failed revert keeps.
pub const MAX_REASON: usize = 200;

impl Revert {
    /// A failed revert whose reason is the text of `error`, cut to [`MAX_REASON`] characters.
    #[must_use]
    pub fn failed(error: &impl std::fmt::Display) -> Self {
        Self::Failed {
            reason: error.to_string().chars().take(MAX_REASON).collect(),
        }
    }
}

/// The delta that undoes a changeset against its first parent.
///
/// `parent` and `changed` hold the files at the changed paths in the trees of the first parent and
/// of the changeset; a path a tree holds no file at is absent. A path the changeset added is
/// deleted, and one it deleted or modified, content or kind, is set back to the parent's file.
#[must_use]
pub fn inverse(
    parent: &BTreeMap<Path, File>,
    changed: &BTreeMap<Path, File>,
) -> BTreeMap<Path, Change> {
    let paths: BTreeSet<&Path> = parent.keys().chain(changed.keys()).collect();
    let mut delta = BTreeMap::new();
    for path in paths {
        match (parent.get(path), changed.get(path)) {
            (Some(before), after) if after != Some(before) => {
                delta.insert(path.clone(), Change::File(*before));
            }
            (None, Some(_)) => {
                delta.insert(path.clone(), Change::Deletion);
            }
            (Some(_), Some(_) | None) | (None, None) => {}
        }
    }
    delta
}

/// The message of the changeset that reverts `changeset` because `check` ended in `state`; `None`
/// where `state` is not a failure or a timeout, which revert nothing.
#[must_use]
pub fn message(changeset: Address, check: &CheckName, state: JobState) -> Option<String> {
    let outcome = match state {
        JobState::Failed => "failed",
        JobState::TimedOut => "timed out",
        JobState::Waiting
        | JobState::Queued
        | JobState::Running { .. }
        | JobState::Passed
        | JobState::Cancelled
        | JobState::Skipped
        | JobState::Cached => {
            return None;
        }
    };
    Some(format!("Revert {changeset}: check {check} {outcome}"))
}

/// Whether `changeset` is one the server made to revert another, which is never reverted
/// automatically.
#[must_use]
pub fn is_automatic(changeset: &Changeset) -> bool {
    changeset.author == AUTHOR && changeset.message.starts_with("Revert ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::file::FileKind;

    fn path(text: &str) -> Path {
        text.parse().unwrap()
    }

    fn file(kind: FileKind, content: &str) -> File {
        File {
            kind,
            content: Address::of(content.as_bytes()),
        }
    }

    fn regular(content: &str) -> File {
        file(FileKind::Regular, content)
    }

    fn files(entries: &[(&str, File)]) -> BTreeMap<Path, File> {
        entries
            .iter()
            .map(|(name, file)| (path(name), *file))
            .collect()
    }

    #[test]
    fn keeps_the_text_of_the_error_as_the_reason_of_a_failed_revert() {
        assert_eq!(
            Revert::failed(&"job queue failed: down"),
            Revert::Failed {
                reason: "job queue failed: down".to_owned()
            }
        );
    }

    #[test]
    fn cuts_a_long_reason_to_the_most_characters_it_keeps() {
        let Revert::Failed { reason } = Revert::failed(&"é".repeat(MAX_REASON + 5)) else {
            panic!("not a failed revert");
        };
        assert_eq!(reason.chars().count(), MAX_REASON);
    }

    #[test]
    fn deletes_a_path_the_changeset_added() {
        let delta = inverse(&files(&[]), &files(&[("new", regular("n"))]));
        assert_eq!(delta, BTreeMap::from([(path("new"), Change::Deletion)]));
    }

    #[test]
    fn restores_a_path_the_changeset_deleted() {
        let before = file(FileKind::Executable, "run");
        let delta = inverse(&files(&[("bin/run", before)]), &files(&[]));
        assert_eq!(
            delta,
            BTreeMap::from([(path("bin/run"), Change::File(before))])
        );
    }

    #[test]
    fn sets_a_modified_path_back_to_the_content_of_the_parent() {
        let delta = inverse(
            &files(&[("a", regular("old"))]),
            &files(&[("a", regular("new"))]),
        );
        assert_eq!(
            delta,
            BTreeMap::from([(path("a"), Change::File(regular("old")))])
        );
    }

    #[test]
    fn sets_a_path_whose_kind_changed_back_to_its_kind() {
        let delta = inverse(
            &files(&[("run", file(FileKind::Regular, "same"))]),
            &files(&[("run", file(FileKind::Executable, "same"))]),
        );
        assert_eq!(
            delta,
            BTreeMap::from([(path("run"), Change::File(file(FileKind::Regular, "same")))])
        );
    }

    #[test]
    fn leaves_a_path_that_is_the_same_in_both_trees() {
        let same = regular("same");
        let delta = inverse(
            &files(&[("a", same), ("b", regular("old"))]),
            &files(&[("a", same), ("b", regular("new"))]),
        );
        assert_eq!(
            delta,
            BTreeMap::from([(path("b"), Change::File(regular("old")))])
        );
    }

    #[test]
    fn names_the_reverted_changeset_and_the_check_that_failed_or_timed_out() {
        let id = Address::of(b"c");
        let name: CheckName = "unit".parse().unwrap();
        assert_eq!(
            message(id, &name, JobState::Failed),
            Some(format!("Revert {id}: check unit failed"))
        );
        assert_eq!(
            message(id, &name, JobState::TimedOut),
            Some(format!("Revert {id}: check unit timed out"))
        );
    }

    #[test]
    fn gives_no_message_for_a_check_that_neither_failed_nor_timed_out() {
        let id = Address::of(b"c");
        let name: CheckName = "unit".parse().unwrap();
        for state in [
            JobState::Waiting,
            JobState::Queued,
            JobState::Running {
                lease_expires_at: 0,
                deadline: 0,
            },
            JobState::Passed,
            JobState::Cancelled,
            JobState::Skipped,
            JobState::Cached,
        ] {
            assert_eq!(message(id, &name, state), None, "{state:?}");
        }
    }

    #[test]
    fn takes_only_a_changeset_the_server_authored_with_a_revert_message_for_an_automatic_revert() {
        let changeset = |author: &str, message: &str| Changeset {
            parents: Vec::new(),
            author: author.to_owned(),
            timestamp: 0,
            message: message.to_owned(),
            changes: BTreeMap::new(),
        };
        assert!(is_automatic(&changeset(
            AUTHOR,
            "Revert sha256:x: check a failed"
        )));
        assert!(!is_automatic(&changeset(
            "https://id.test ada",
            "Revert sha256:x: check a failed"
        )));
        assert!(!is_automatic(&changeset(AUTHOR, "something else")));
    }
}
