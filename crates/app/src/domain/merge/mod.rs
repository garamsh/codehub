//! The landing merge: rebasing a landing's delta onto what `main` changed since its base (ADR 0012, ADR 0017).

mod file;

use std::collections::BTreeMap;
use std::fmt;

pub use file::{FileConflict, merge_file};

/// What a delta records for one path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// The path holds this content.
    Write(Vec<u8>),
    /// The path is removed.
    Delete,
}

impl Change {
    fn content(&self) -> Option<&[u8]> {
        match self {
            Self::Write(content) => Some(content),
            Self::Delete => None,
        }
    }

    fn from_content(content: Option<Vec<u8>>) -> Self {
        content.map_or(Self::Delete, Self::Write)
    }
}

/// How a landing's delta goes onto the current `main`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rebase<P> {
    /// No path the landing changes was changed on `main` since its base, so
    /// the landing's delta stands unchanged on the new parent.
    Reparent,
    /// Paths overlapped and merged cleanly; this is the landing's delta
    /// against the current `main`. A path whose merge equals `main`'s
    /// content is not in it.
    Merge(BTreeMap<P, Change>),
}

/// The paths whose merge conflicts, each with its reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflicts<P>(BTreeMap<P, FileConflict>);

impl<P> Conflicts<P> {
    /// The conflicting paths, each with why it conflicts.
    pub fn paths(&self) -> &BTreeMap<P, FileConflict> {
        &self.0
    }
}

impl<P> fmt::Display for Conflicts<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} conflicting paths", self.0.len())
    }
}

impl<P: fmt::Debug> std::error::Error for Conflicts<P> {}

/// Rebases the `landing` delta onto the `main` delta, both taken against the
/// same base.
///
/// `base` holds the base content of every path both deltas change; a path it
/// lacks did not exist at the base. Paths only one delta changes are kept as
/// that delta has them.
///
/// # Errors
///
/// Returns every path whose merge conflicts, with its [`FileConflict`].
pub fn rebase<P: Ord + Clone>(
    base: &BTreeMap<P, Vec<u8>>,
    main: &BTreeMap<P, Change>,
    landing: &BTreeMap<P, Change>,
) -> Result<Rebase<P>, Conflicts<P>> {
    if !landing.keys().any(|path| main.contains_key(path)) {
        return Ok(Rebase::Reparent);
    }

    let mut merged = BTreeMap::new();
    let mut conflicts = BTreeMap::new();
    for (path, theirs) in landing {
        let Some(ours) = main.get(path) else {
            merged.insert(path.clone(), theirs.clone());
            continue;
        };
        let base = base.get(path).map(Vec::as_slice);
        match merge_file(base, ours.content(), theirs.content()) {
            Ok(content) if content.as_deref() == ours.content() => {}
            Ok(content) => {
                merged.insert(path.clone(), Change::from_content(content));
            }
            Err(conflict) => {
                conflicts.insert(path.clone(), conflict);
            }
        }
    }

    if conflicts.is_empty() {
        Ok(Rebase::Merge(merged))
    } else {
        Err(Conflicts(conflicts))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(text: &str) -> Change {
        Change::Write(text.as_bytes().to_vec())
    }

    fn delta(changes: &[(&'static str, Change)]) -> BTreeMap<&'static str, Change> {
        changes.iter().cloned().collect()
    }

    fn base(files: &[(&'static str, &str)]) -> BTreeMap<&'static str, Vec<u8>> {
        files
            .iter()
            .map(|(path, text)| (*path, text.as_bytes().to_vec()))
            .collect()
    }

    #[test]
    fn reparents_a_landing_on_disjoint_paths() {
        let main = delta(&[("a", write("A\n"))]);
        let landing = delta(&[("b", write("B\n"))]);
        assert_eq!(rebase(&base(&[]), &main, &landing), Ok(Rebase::Reparent));
    }

    #[test]
    fn merges_overlapping_paths_and_keeps_the_landing_only_paths() {
        let base = base(&[("f", "a\nb\nc\n")]);
        let main = delta(&[("f", write("A\nb\nc\n")), ("m", write("m\n"))]);
        let landing = delta(&[("f", write("a\nb\nC\n")), ("l", Change::Delete)]);
        assert_eq!(
            rebase(&base, &main, &landing),
            Ok(Rebase::Merge(delta(&[
                ("f", write("A\nb\nC\n")),
                ("l", Change::Delete),
            ])))
        );
    }

    #[test]
    fn leaves_out_an_overlapping_path_whose_merge_equals_main() {
        let base = base(&[("f", "a\n")]);
        let main = delta(&[("f", write("b\n"))]);
        let landing = delta(&[("f", write("b\n")), ("g", write("g\n"))]);
        assert_eq!(
            rebase(&base, &main, &landing),
            Ok(Rebase::Merge(delta(&[("g", write("g\n"))])))
        );
    }

    #[test]
    fn returns_every_conflicting_path_and_only_those() {
        let base = base(&[("lines", "a\n"), ("deleted", "d\n"), ("clean", "a\nb\nc\n")]);
        let main = delta(&[
            ("lines", write("x\n")),
            ("deleted", Change::Delete),
            ("added", write("1\n")),
            ("clean", write("A\nb\nc\n")),
        ]);
        let landing = delta(&[
            ("lines", write("y\n")),
            ("deleted", write("D\n")),
            ("added", write("2\n")),
            ("clean", write("a\nb\nC\n")),
        ]);
        assert_eq!(
            rebase(&base, &main, &landing).map_err(|conflicts| conflicts.paths().clone()),
            Err(BTreeMap::from([
                ("added", FileConflict::AddAdd),
                ("deleted", FileConflict::DeleteModify),
                ("lines", FileConflict::Lines),
            ]))
        );
    }

    #[test]
    fn treats_a_rename_against_an_edit_as_a_deletion_against_a_modification() {
        let base = base(&[("old", "a\nb\n")]);
        let main = delta(&[("old", Change::Delete), ("new", write("a\nb\n"))]);
        let landing = delta(&[("old", write("a\nB\n"))]);
        assert_eq!(
            rebase(&base, &main, &landing).map_err(|conflicts| conflicts.paths().clone()),
            Err(BTreeMap::from([("old", FileConflict::DeleteModify)]))
        );
    }
}
