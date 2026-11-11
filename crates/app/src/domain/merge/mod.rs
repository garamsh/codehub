//! The landing merge: rebasing a landing's delta onto what `main` changed since its base (ADR 0012, ADR 0017, ADR 0019).

mod file;

use std::collections::BTreeMap;
use std::fmt;

pub use file::{FileConflict, FileKind, FileVersion, merge_file};

/// What a delta records for one path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// The path holds this version.
    Write(FileVersion),
    /// The path is removed.
    Delete,
}

impl Change {
    fn version(&self) -> Option<&FileVersion> {
        match self {
            Self::Write(version) => Some(version),
            Self::Delete => None,
        }
    }

    fn from_version(version: Option<FileVersion>) -> Self {
        version.map_or(Self::Delete, Self::Write)
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
    /// version is not in it.
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
/// `base` holds the base version of every path both deltas change; a path it
/// lacks did not exist at the base. Paths only one delta changes are kept as
/// that delta has them.
///
/// # Errors
///
/// Returns every path whose merge conflicts, with its [`FileConflict`].
pub fn rebase<P: Ord + Clone>(
    base: &BTreeMap<P, FileVersion>,
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
        match merge_file(base.get(path), ours.version(), theirs.version()) {
            Ok(version) if version.as_ref() == ours.version() => {}
            Ok(version) => {
                merged.insert(path.clone(), Change::from_version(version));
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

    fn file(text: &str) -> FileVersion {
        FileVersion {
            kind: FileKind::File,
            content: text.as_bytes().to_vec(),
        }
    }

    fn write(text: &str) -> Change {
        Change::Write(file(text))
    }

    fn write_executable(text: &str) -> Change {
        Change::Write(FileVersion {
            kind: FileKind::Executable,
            content: text.as_bytes().to_vec(),
        })
    }

    fn delta(changes: &[(&'static str, Change)]) -> BTreeMap<&'static str, Change> {
        changes.iter().cloned().collect()
    }

    fn base(files: &[(&'static str, &str)]) -> BTreeMap<&'static str, FileVersion> {
        files
            .iter()
            .map(|(path, text)| (*path, file(text)))
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
    fn keeps_a_landing_kind_change_on_a_path_main_edited() {
        let base = base(&[("f", "a\n")]);
        let main = delta(&[("f", write("b\n"))]);
        let landing = delta(&[("f", write_executable("a\n"))]);
        assert_eq!(
            rebase(&base, &main, &landing),
            Ok(Rebase::Merge(delta(&[("f", write_executable("b\n"))])))
        );
    }

    #[test]
    fn returns_every_conflicting_path_and_only_those() {
        let base = base(&[("lines", "a\n"), ("deleted", "d\n"), ("clean", "a\nb\nc\n")]);
        let main = delta(&[
            ("lines", write("x\n")),
            ("deleted", Change::Delete),
            ("added", write("1\n")),
            ("kind", write("k\n")),
            ("clean", write("A\nb\nc\n")),
        ]);
        let landing = delta(&[
            ("lines", write("y\n")),
            ("deleted", write("D\n")),
            ("added", write("2\n")),
            ("kind", write_executable("k\n")),
            ("clean", write("a\nb\nC\n")),
        ]);
        assert_eq!(
            rebase(&base, &main, &landing).map_err(|conflicts| conflicts.paths().clone()),
            Err(BTreeMap::from([
                ("added", FileConflict::AddAdd),
                ("deleted", FileConflict::DeleteModify),
                ("kind", FileConflict::Kind),
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
