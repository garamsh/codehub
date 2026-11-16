//! Generators of the domain values the property tests draw: components and paths, files, deltas
//! and changesets, in a wide form that exercises every byte and a narrow form whose names collide.

use std::collections::BTreeMap;

use app::domain::address::Address;
use app::domain::changeset::{Change, Changeset};
use app::domain::file::{File, FileKind};
use app::domain::path::{Component, Path};
use proptest::collection::{btree_map, vec};
use proptest::prelude::*;

/// Any address.
pub fn address() -> impl Strategy<Value = Address> {
    any::<[u8; 32]>().prop_map(Address::from_digest)
}

/// One of three addresses, so that equal contents come up.
pub fn narrow_address() -> impl Strategy<Value = Address> {
    (0_u8..3).prop_map(|n| Address::of(&[n]))
}

/// A component of any bytes the component rules allow.
pub fn wide_component() -> impl Strategy<Value = Component> {
    vec(any::<u8>(), 1..6).prop_filter_map("not a component", |bytes| Component::new(bytes).ok())
}

/// A component out of four names, one of them a byte prefix of another.
pub fn narrow_component() -> impl Strategy<Value = Component> {
    prop::sample::select(vec!["a", "b", "ab", "c"]).prop_filter_map("a valid component", |name| {
        Component::new(name.as_bytes().to_vec()).ok()
    })
}

/// A path of one to four wide components.
pub fn wide_path() -> impl Strategy<Value = Path> {
    vec(wide_component(), 1..4)
        .prop_filter_map("not empty", |components| Path::new(components).ok())
}

/// A path of one to three narrow components.
pub fn narrow_path() -> impl Strategy<Value = Path> {
    vec(narrow_component(), 1..4)
        .prop_filter_map("not empty", |components| Path::new(components).ok())
}

/// A file kind.
pub fn file_kind() -> impl Strategy<Value = FileKind> {
    prop_oneof![Just(FileKind::Regular), Just(FileKind::Executable)]
}

/// A file with any content address.
pub fn wide_file() -> impl Strategy<Value = File> {
    (file_kind(), address()).prop_map(|(kind, content)| File { kind, content })
}

/// A file out of six, so that equal files come up.
pub fn narrow_file() -> impl Strategy<Value = File> {
    (file_kind(), narrow_address()).prop_map(|(kind, content)| File { kind, content })
}

/// A deletion or a file.
pub fn change() -> impl Strategy<Value = Change> {
    prop_oneof![Just(Change::Deletion), wide_file().prop_map(Change::File)]
}

/// A changeset with any field, its changes at wide paths.
pub fn changeset() -> impl Strategy<Value = Changeset> {
    (
        vec(address(), 0..3),
        any::<String>(),
        any::<i64>(),
        any::<String>(),
        btree_map(wide_path(), change(), 0..5),
    )
        .prop_map(|(parents, author, timestamp, message, changes)| Changeset {
            parents,
            author,
            timestamp,
            message,
            changes,
        })
}

/// Drops every path that is a proper prefix of another, so that no path is both a file and a
/// directory.
#[must_use]
pub fn without_file_under_file(mut files: BTreeMap<Path, File>) -> BTreeMap<Path, File> {
    let paths: Vec<Path> = files.keys().cloned().collect();
    for path in &paths {
        let is_directory = paths.iter().any(|other| {
            other.components().len() > path.components().len()
                && other.components().starts_with(path.components())
        });
        if is_directory {
            files.remove(path);
        }
    }
    files
}

/// The files of a tree: narrow paths with no file under a file, up to eight of them.
pub fn tree_files() -> impl Strategy<Value = BTreeMap<Path, File>> {
    btree_map(narrow_path(), narrow_file(), 0..8).prop_map(without_file_under_file)
}
