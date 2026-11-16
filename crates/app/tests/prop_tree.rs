//! Deriving a tree from a delta: the delta between two trees takes the first to the second, and a
//! delta and its inverse cancel.

pub mod generators;

use std::collections::BTreeMap;

use app::domain::address::Address;
use app::domain::changeset::Change;
use app::domain::file::File;
use app::domain::path::{Component, Path};
use app::domain::revert::inverse;
use app::domain::tree::{Derived, Entry, Tree, derive};
use proptest::prelude::*;

use generators::tree_files;

type Files = BTreeMap<Path, File>;

/// The tree holding exactly `files`, derived from the empty tree.
#[cfg(test)]
fn build(files: &Files) -> Derived {
    let delta = files
        .iter()
        .map(|(path, file)| (path.clone(), Change::File(*file)))
        .collect();
    derive(&Tree::default(), &delta, &BTreeMap::new()).unwrap()
}

/// What turns the files `before` into the files `after`.
fn delta(before: &Files, after: &Files) -> BTreeMap<Path, Change> {
    before
        .keys()
        .chain(after.keys())
        .filter(|path| before.get(*path) != after.get(*path))
        .map(|path| {
            let change = after
                .get(path)
                .map_or(Change::Deletion, |file| Change::File(*file));
            (path.clone(), change)
        })
        .collect()
}

/// The files a tree holds, found by walking it from `root` through `trees`.
#[cfg(test)]
fn files_of(root: Address, trees: &BTreeMap<Address, Tree>) -> Files {
    fn walk(tree: &Tree, trees: &BTreeMap<Address, Tree>, at: &[Component], out: &mut Files) {
        for (name, entry) in &tree.entries {
            let mut here = at.to_vec();
            here.push(name.clone());
            match entry {
                Entry::File(file) => {
                    out.insert(Path::new(here).unwrap(), *file);
                }
                Entry::Directory(address) => walk(trees.get(address).unwrap(), trees, &here, out),
            }
        }
    }
    let mut out = Files::new();
    walk(trees.get(&root).unwrap(), trees, &[], &mut out);
    out
}

/// The files `files` hold at the paths `delta` changes.
fn at_changed(files: &Files, delta: &BTreeMap<Path, Change>) -> Files {
    files
        .iter()
        .filter(|(path, _)| delta.contains_key(*path))
        .map(|(path, file)| (path.clone(), *file))
        .collect()
}

proptest! {
    #[test]
    fn a_built_tree_holds_the_files_it_was_built_from(files in tree_files()) {
        let built = build(&files);
        prop_assert_eq!(files_of(built.root, &built.trees), files);
    }

    #[test]
    fn the_same_files_build_the_same_root(files in tree_files()) {
        prop_assert_eq!(build(&files).root, build(&files).root);
    }

    #[test]
    fn the_delta_between_two_trees_takes_the_first_to_the_second(
        before in tree_files(),
        after in tree_files(),
    ) {
        let first = build(&before);
        let second = build(&after);
        let derived = derive(
            first.trees.get(&first.root).unwrap(),
            &delta(&before, &after),
            &first.trees,
        )
        .unwrap();
        prop_assert_eq!(derived.root, second.root);
        let mut known = first.trees;
        known.extend(derived.trees);
        prop_assert_eq!(files_of(derived.root, &known), after);
    }

    #[test]
    fn a_delta_and_its_inverse_cancel(before in tree_files(), after in tree_files()) {
        let forward = delta(&before, &after);
        let first = build(&before);
        let second = build(&after);
        let back = inverse(&at_changed(&before, &forward), &at_changed(&after, &forward));
        let mut known = first.trees.clone();
        known.extend(second.trees.clone());
        let restored = derive(second.trees.get(&second.root).unwrap(), &back, &known).unwrap();
        prop_assert_eq!(restored.root, first.root);
    }

    #[test]
    fn the_inverse_of_a_delta_changes_only_the_paths_the_delta_changes(
        before in tree_files(),
        after in tree_files(),
    ) {
        let forward = delta(&before, &after);
        let back = inverse(&at_changed(&before, &forward), &at_changed(&after, &forward));
        prop_assert!(back.keys().all(|path| forward.contains_key(path)));
        prop_assert_eq!(back.len(), forward.len());
    }

    #[test]
    fn a_delta_that_changes_nothing_leaves_the_root(files in tree_files()) {
        let built = build(&files);
        let same = derive(
            built.trees.get(&built.root).unwrap(),
            &BTreeMap::new(),
            &built.trees,
        )
        .unwrap();
        prop_assert_eq!(same.root, built.root);
    }
}
