//! A view: a directory that holds one workspace's files and keeps its own state in `.ch/`
//! (ADR 0048).

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use serde::{Deserialize, Serialize};

use crate::command::address;
use crate::failure::Failure;
use crate::state::RepositoryRef;
use crate::wire::{Kind, SnapshotChange};

/// The directory a view keeps its state in, at its root; never snapshotted.
const STATE_DIRECTORY: &str = ".ch";
const STATE_FILE: &str = "view.json";
/// The repository's ignore rules, in gitignore syntax.
const IGNORE_FILE: &str = ".codehub/ignore";

/// What a file held: its kind and the address of its bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// The kind of the file.
    pub kind: Kind,
    /// The address of its bytes.
    pub content: String,
}

/// Whether the view's workspace is still there to change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    /// The workspace is open.
    Open,
    /// The workspace landed.
    Closed,
}

/// A view and the state it keeps.
#[derive(Debug, Serialize, Deserialize)]
pub struct View {
    /// The directory the view is in.
    #[serde(skip)]
    pub root: PathBuf,
    /// The repository of the workspace.
    pub repository: RepositoryRef,
    /// The workspace's draft name.
    pub draft: String,
    /// The changeset the workspace is based on; absent where `main` was unset.
    pub base: Option<String>,
    /// The directory of the repository the view covers; empty for all of it.
    pub prefix: String,
    /// Whether the workspace is open.
    pub phase: Phase,
    /// The base's files under the prefix.
    pub base_files: BTreeMap<String, Entry>,
    /// What each path held at the last snapshot.
    pub recorded: BTreeMap<String, Entry>,
}

/// What a snapshot sends, and what the view then records.
#[derive(Debug, PartialEq, Eq)]
pub struct Plan {
    /// The changes since the last snapshot.
    pub changes: Vec<SnapshotChange>,
    /// What each path holds once they are recorded.
    pub recorded: BTreeMap<String, Entry>,
}

/// The ignore rules of a view's `.codehub/ignore`.
pub struct Rules(Gitignore);

impl Rules {
    /// The rules as the file stands in `root`; none where it does not exist.
    ///
    /// # Errors
    ///
    /// Fails where the file cannot be read or holds a rule that does not parse.
    pub fn read(root: &Path) -> Result<Self, Failure> {
        let file = root.join(IGNORE_FILE);
        if !file.is_file() {
            return Ok(Self(Gitignore::empty()));
        }
        let mut builder = GitignoreBuilder::new(root);
        if let Some(error) = builder.add(&file) {
            return Err(Failure::Other(format!("{IGNORE_FILE}: {error}")));
        }
        let rules = builder
            .build()
            .map_err(|error| Failure::Other(format!("{IGNORE_FILE}: {error}")))?;
        Ok(Self(rules))
    }

    /// Whether a rule matches `path`, relative to the view's root, or a directory above it.
    #[must_use]
    pub fn matches(&self, path: &Path, directory: bool) -> bool {
        self.0
            .matched_path_or_any_parents(path, directory)
            .is_ignore()
    }
}

fn outside_a_view() -> Failure {
    Failure::Usage("not in a view; run `ch open <directory>` to open one".to_owned())
}

fn closed_view() -> Failure {
    Failure::Usage(
        "this view is closed, its workspace landed; run `ch open <directory>` to open a new one"
            .to_owned(),
    )
}

/// The path in a view at `root` of the repository path `path`.
///
/// # Errors
///
/// Fails where `path` is empty, climbs out of the root or names the state directory.
pub fn file_path(root: &Path, path: &str) -> Result<PathBuf, Failure> {
    let relative = Path::new(path);
    let plain = !path.is_empty()
        && relative
            .components()
            .all(|component| matches!(component, Component::Normal(_)));
    if !plain || relative.starts_with(STATE_DIRECTORY) {
        return Err(Failure::Other(format!(
            "{path:?} is not a path a view can hold"
        )));
    }
    Ok(root.join(relative))
}

/// The kind of the file `metadata` describes.
fn kind_of(metadata: &fs::Metadata) -> Kind {
    if metadata.permissions().mode() & 0o111 == 0 {
        Kind::Regular
    } else {
        Kind::Executable
    }
}

/// Writes `bytes` at `path` of the view at `root`, with the mode `kind` says.
///
/// # Errors
///
/// Fails where the path is not one a view holds or the file cannot be written.
pub fn write_file(root: &Path, path: &str, kind: Kind, bytes: &[u8]) -> Result<(), Failure> {
    let target = file_path(root, path)?;
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|error| failure(parent, &error))?;
    }
    fs::write(&target, bytes).map_err(|error| failure(&target, &error))?;
    let mode = match kind {
        Kind::Regular => 0o644,
        Kind::Executable => 0o755,
    };
    fs::set_permissions(&target, fs::Permissions::from_mode(mode))
        .map_err(|error| failure(&target, &error))
}

fn failure(path: &Path, error: &io::Error) -> Failure {
    Failure::Other(format!("{}: {error}", path.display()))
}

impl View {
    /// The view that holds `start` or a directory above it, if any.
    ///
    /// # Errors
    ///
    /// Fails where a view's state cannot be read or does not parse.
    pub fn find(start: &Path) -> Result<Option<Self>, Failure> {
        for root in start.ancestors() {
            let file = root.join(STATE_DIRECTORY).join(STATE_FILE);
            let text = match fs::read_to_string(&file) {
                Ok(text) => text,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(failure(&file, &error)),
            };
            let mut view: Self = serde_json::from_str(&text)
                .map_err(|error| Failure::Other(format!("{}: {error}", file.display())))?;
            root.clone_into(&mut view.root);
            return Ok(Some(view));
        }
        Ok(None)
    }

    /// The open view that holds `start` or a directory above it.
    ///
    /// # Errors
    ///
    /// Fails as a usage error where there is no view or it is closed, and otherwise as
    /// [`View::find`] does.
    pub fn find_open(start: &Path) -> Result<Self, Failure> {
        Self::find(start)?.ok_or_else(outside_a_view)?.ensure_open()
    }

    /// The view itself where its workspace is open.
    ///
    /// # Errors
    ///
    /// Fails as a usage error where the workspace landed.
    pub fn ensure_open(self) -> Result<Self, Failure> {
        match self.phase {
            Phase::Open => Ok(self),
            Phase::Closed => Err(closed_view()),
        }
    }

    /// Writes the state, which replaces the earlier one whole or not at all.
    ///
    /// # Errors
    ///
    /// Fails where the state cannot be written.
    pub fn save(&self) -> Result<(), Failure> {
        let directory = self.root.join(STATE_DIRECTORY);
        fs::create_dir_all(&directory).map_err(|error| failure(&directory, &error))?;
        let text = serde_json::to_string(self)
            .map_err(|error| Failure::Other(format!("cannot write the view's state: {error}")))?;
        let temporary = directory.join(format!("{STATE_FILE}.tmp"));
        fs::write(&temporary, text).map_err(|error| failure(&temporary, &error))?;
        let file = directory.join(STATE_FILE);
        fs::rename(&temporary, &file).map_err(|error| failure(&file, &error))
    }

    /// Whether `path`, relative to the root, is the prefix or lies under it.
    #[must_use]
    pub fn covers(&self, path: &Path) -> bool {
        path.starts_with(&self.prefix)
    }

    /// Whether the snapshot tracks `path`: a rule does not match it, or the base holds it.
    fn tracks(&self, rules: &Rules, path: &str) -> bool {
        self.base_files.contains_key(path) || !rules.matches(Path::new(path), false)
    }

    /// Whether the base holds a file under the directory `path`.
    fn base_holds_under(&self, path: &Path) -> bool {
        let Some(directory) = path.to_str() else {
            return false;
        };
        let start = format!("{directory}/");
        self.base_files
            .range(start.clone()..)
            .next()
            .is_some_and(|(held, _)| held.starts_with(&start))
    }

    /// The files of the view that a snapshot tracks, by path.
    ///
    /// # Errors
    ///
    /// Fails where a directory or a file cannot be read, or a tracked file's name is not UTF-8.
    pub fn scan(&self, rules: &Rules) -> Result<BTreeMap<String, Entry>, Failure> {
        let mut found = BTreeMap::new();
        self.walk(rules, &self.root, Path::new(""), &mut found)?;
        Ok(found)
    }

    fn walk(
        &self,
        rules: &Rules,
        directory: &Path,
        relative: &Path,
        found: &mut BTreeMap<String, Entry>,
    ) -> Result<(), Failure> {
        let entries = fs::read_dir(directory).map_err(|error| failure(directory, &error))?;
        for entry in entries {
            let entry = entry.map_err(|error| failure(directory, &error))?;
            let name = entry.file_name();
            if relative.as_os_str().is_empty() && name == STATE_DIRECTORY {
                continue;
            }
            let below = relative.join(&name);
            let full = entry.path();
            let file_type = entry.file_type().map_err(|error| failure(&full, &error))?;
            if file_type.is_dir() {
                // Another view's directory is not part of this one.
                if full.join(STATE_DIRECTORY).exists() {
                    continue;
                }
                let near =
                    below.starts_with(&self.prefix) || Path::new(&self.prefix).starts_with(&below);
                if near && (!rules.matches(&below, true) || self.base_holds_under(&below)) {
                    self.walk(rules, &full, &below, found)?;
                }
            } else if file_type.is_file() && self.covers(&below) {
                let Some(path) = below.to_str() else {
                    if rules.matches(&below, false) {
                        continue;
                    }
                    return Err(Failure::Other(format!(
                        "{}: the name is not UTF-8",
                        full.display()
                    )));
                };
                if !self.tracks(rules, path) {
                    continue;
                }
                let bytes = fs::read(&full).map_err(|error| failure(&full, &error))?;
                let metadata = entry.metadata().map_err(|error| failure(&full, &error))?;
                found.insert(
                    path.to_owned(),
                    Entry {
                        kind: kind_of(&metadata),
                        content: address(&bytes),
                    },
                );
            }
        }
        Ok(())
    }

    /// What a snapshot of the view as it now stands, `current`, sends and records.
    ///
    /// A path back at the base's content is sent as a restore, a path gone that the base lacks
    /// likewise, and one gone that the base holds as a deletion. A recorded path that a rule now
    /// ignores is no longer scanned, so it is gone and the base lacks it: a restore.
    #[must_use]
    pub fn plan(&self, current: BTreeMap<String, Entry>) -> Plan {
        let mut changes = Vec::new();
        for (path, entry) in &current {
            if self.recorded.get(path) == Some(entry) {
                continue;
            }
            changes.push(if self.base_files.get(path) == Some(entry) {
                SnapshotChange::Restore { path: path.clone() }
            } else {
                SnapshotChange::File {
                    path: path.clone(),
                    kind: entry.kind,
                    content: entry.content.clone(),
                }
            });
        }
        let recorded = current;
        for path in self.recorded.keys() {
            if recorded.contains_key(path) {
                continue;
            }
            changes.push(if self.base_files.contains_key(path) {
                SnapshotChange::Delete { path: path.clone() }
            } else {
                SnapshotChange::Restore { path: path.clone() }
            });
        }
        Plan { changes, recorded }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(content: &str) -> Entry {
        Entry {
            kind: Kind::Regular,
            content: content.to_owned(),
        }
    }

    fn view(base: &[(&str, &str)], recorded: &[(&str, &str)]) -> View {
        let map = |files: &[(&str, &str)]| {
            files
                .iter()
                .map(|(path, content)| ((*path).to_owned(), entry(content)))
                .collect()
        };
        View {
            root: PathBuf::new(),
            repository: "o/r".parse().unwrap(),
            draft: "default".to_owned(),
            base: None,
            prefix: String::new(),
            phase: Phase::Open,
            base_files: map(base),
            recorded: map(recorded),
        }
    }

    fn rules(lines: &[&str]) -> Rules {
        let mut builder = GitignoreBuilder::new("");
        for line in lines {
            builder.add_line(None, line).unwrap();
        }
        Rules(builder.build().unwrap())
    }

    fn current(files: &[(&str, &str)]) -> BTreeMap<String, Entry> {
        files
            .iter()
            .map(|(path, content)| ((*path).to_owned(), entry(content)))
            .collect()
    }

    #[test]
    fn sends_a_file_back_at_its_base_content_as_a_restore() {
        let view = view(&[("a", "base")], &[("a", "edited")]);
        let plan = view.plan(current(&[("a", "base")]));
        assert_eq!(plan.changes, [SnapshotChange::Restore { path: "a".into() }]);
        assert_eq!(plan.recorded, current(&[("a", "base")]));
    }

    #[test]
    fn sends_a_removed_file_as_a_deletion_where_the_base_holds_it_and_as_a_restore_where_not() {
        let view = view(&[("a", "base")], &[("a", "base"), ("b", "new")]);
        let plan = view.plan(current(&[]));
        assert_eq!(
            plan.changes,
            [
                SnapshotChange::Delete { path: "a".into() },
                SnapshotChange::Restore { path: "b".into() }
            ]
        );
        assert!(plan.recorded.is_empty());
    }

    #[test]
    fn restores_a_recorded_path_a_rule_now_ignores_and_the_base_lacks() {
        let view = view(&[], &[("out/x", "one")]);
        let plan = view.plan(current(&[]));
        assert_eq!(
            plan.changes,
            [SnapshotChange::Restore {
                path: "out/x".into()
            }]
        );
        assert!(plan.recorded.is_empty());
    }

    #[test]
    fn tracks_an_ignored_path_only_where_the_base_holds_it() {
        let view = view(&[("out/kept", "base")], &[]);
        let rules = rules(&["out/"]);
        assert!(view.tracks(&rules, "out/kept"));
        assert!(!view.tracks(&rules, "out/other"));
    }

    #[test]
    fn refuses_a_path_that_climbs_out_of_the_view_or_names_its_state() {
        let root = Path::new("/v");
        assert!(file_path(root, "a/b").is_ok());
        for path in ["", "../a", "a/../b", "/a", ".ch/view.json"] {
            assert!(file_path(root, path).is_err(), "{path}");
        }
    }
}
