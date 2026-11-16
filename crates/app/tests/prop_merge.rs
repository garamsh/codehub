//! The three-way merge of a path's version and the rebase of a landing's delta: the laws a merge
//! keeps whatever the content.

use std::collections::BTreeMap;

use app::domain::file::FileKind;
use app::domain::merge::{Change, FileConflict, FileVersion, Rebase, merge_file, rebase};
use proptest::collection::{btree_map, vec};
use proptest::prelude::*;

fn kind() -> impl Strategy<Value = FileKind> {
    prop_oneof![Just(FileKind::Regular), Just(FileKind::Executable)]
}

/// Text of up to six lines out of three, the last with or without its newline.
fn lines() -> impl Strategy<Value = Vec<u8>> {
    (
        vec(prop::sample::select(vec!["a\n", "b\n", "c\n"]), 0..6),
        any::<bool>(),
    )
        .prop_map(|(lines, bare_end)| {
            let mut text = lines.concat();
            if bare_end {
                text.push_str("end");
            }
            text.into_bytes()
        })
}

/// Content that is text, with now and then bytes that are not.
fn content() -> impl Strategy<Value = Vec<u8>> {
    prop_oneof![9 => lines(), 1 => vec(any::<u8>(), 0..6)]
}

fn version() -> impl Strategy<Value = FileVersion> {
    (kind(), content()).prop_map(|(kind, content)| FileVersion { kind, content })
}

fn maybe_version() -> impl Strategy<Value = Option<FileVersion>> {
    prop::option::of(version())
}

fn change() -> impl Strategy<Value = Change> {
    prop_oneof![Just(Change::Delete), version().prop_map(Change::Write)]
}

/// What a side does to one base line; `Insert` keeps the line and adds one before it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Edit {
    Keep,
    Insert,
    Replace,
    Delete,
}

fn edit() -> impl Strategy<Value = Edit> {
    prop_oneof![
        Just(Edit::Keep),
        Just(Edit::Insert),
        Just(Edit::Replace),
        Just(Edit::Delete),
    ]
}

/// Whether `edit` changes the line it is on, as a replacement or a deletion does.
fn changes(edit: Edit) -> bool {
    matches!(edit, Edit::Replace | Edit::Delete)
}

/// What each side does to each of up to seven base lines: no line is changed by both and no
/// point has an insertion from both. An insertion may sit beside the other side's change, at its
/// start or its end, but not inside it, where the lines either side of the point are both changed.
fn disjoint_edits() -> impl Strategy<Value = Vec<(Edit, Edit)>> {
    vec((edit(), edit()), 1..8).prop_map(|edits| {
        let mut edits: Vec<(Edit, Edit)> = edits
            .into_iter()
            .map(|(ours, theirs)| match (ours, theirs) {
                (Edit::Keep, _) | (_, Edit::Keep) => (ours, theirs),
                (Edit::Insert, Edit::Insert) => (ours, Edit::Keep),
                (Edit::Insert, _) | (_, Edit::Insert) => (ours, theirs),
                _ => (ours, Edit::Keep),
            })
            .collect();
        let mut before = (Edit::Keep, Edit::Keep);
        for entry in &mut edits {
            let (ours_before, theirs_before) = before;
            before = *entry;
            if entry.0 == Edit::Insert && changes(theirs_before) && changes(entry.1) {
                entry.0 = Edit::Keep;
            }
            if entry.1 == Edit::Insert && changes(ours_before) && changes(entry.0) {
                entry.1 = Edit::Keep;
            }
        }
        edits
    })
}

/// The text of one unique line per edit, each edited as `edits` says by the side `name`.
fn text(edits: &[Edit], name: &str) -> Vec<u8> {
    let mut text = String::new();
    for (index, edit) in edits.iter().enumerate() {
        match edit {
            Edit::Keep => text.push_str(&format!("line {index}\n")),
            Edit::Insert => text.push_str(&format!("{name} inserted {index}\nline {index}\n")),
            Edit::Replace => text.push_str(&format!("{name} {index}\n")),
            Edit::Delete => {}
        }
    }
    text.into_bytes()
}

fn plain(content: Vec<u8>) -> FileVersion {
    FileVersion {
        kind: FileKind::Regular,
        content,
    }
}

proptest! {
    #[test]
    fn merging_a_side_with_an_unchanged_base_gives_that_side(
        base in maybe_version(),
        side in maybe_version(),
    ) {
        prop_assert_eq!(merge_file(base.as_ref(), side.as_ref(), base.as_ref()), Ok(side.clone()));
        prop_assert_eq!(merge_file(base.as_ref(), base.as_ref(), side.as_ref()), Ok(side));
    }

    #[test]
    fn merging_identical_sides_gives_that_side(
        base in maybe_version(),
        side in maybe_version(),
    ) {
        prop_assert_eq!(merge_file(base.as_ref(), side.as_ref(), side.as_ref()), Ok(side));
    }

    #[test]
    fn a_merge_is_symmetric_in_its_two_sides(
        base in maybe_version(),
        ours in maybe_version(),
        theirs in maybe_version(),
    ) {
        let forward = merge_file(base.as_ref(), ours.as_ref(), theirs.as_ref());
        let backward = merge_file(base.as_ref(), theirs.as_ref(), ours.as_ref());
        prop_assert_eq!(forward, backward);
    }

    #[test]
    fn a_clean_merge_holds_every_change_of_both_sides(edits in disjoint_edits()) {
        let base = text(&vec![Edit::Keep; edits.len()], "");
        let ours: Vec<Edit> = edits.iter().map(|(ours, _)| *ours).collect();
        let theirs: Vec<Edit> = edits.iter().map(|(_, theirs)| *theirs).collect();
        // Each line is replaced by the side that edits it, so the expected text takes both.
        let mut expected = String::new();
        for (index, (ours, theirs)) in edits.iter().enumerate() {
            if *ours == Edit::Insert {
                expected.push_str(&format!("ours inserted {index}\n"));
            }
            if *theirs == Edit::Insert {
                expected.push_str(&format!("theirs inserted {index}\n"));
            }
            match (ours, theirs) {
                (Edit::Replace, _) => expected.push_str(&format!("ours {index}\n")),
                (_, Edit::Replace) => expected.push_str(&format!("theirs {index}\n")),
                (Edit::Delete, _) | (_, Edit::Delete) => {}
                (Edit::Keep | Edit::Insert, Edit::Keep | Edit::Insert) => expected.push_str(&format!("line {index}\n")),
            }
        }
        let result = merge_file(
            Some(&plain(base)),
            Some(&plain(text(&ours, "ours"))),
            Some(&plain(text(&theirs, "theirs"))),
        );
        prop_assert_eq!(result, Ok(Some(plain(expected.into_bytes()))));
    }

    #[test]
    fn a_line_changed_differently_on_both_sides_conflicts_and_changed_alike_does_not(
        count in 1_usize..6,
        at in 0_usize..6,
    ) {
        let at = at % count;
        let mut keep = vec![Edit::Keep; count];
        let base = plain(text(&keep, ""));
        if let Some(edit) = keep.get_mut(at) {
            *edit = Edit::Replace;
        }
        let ours = plain(text(&keep, "ours"));
        let theirs = plain(text(&keep, "theirs"));
        prop_assert_eq!(
            merge_file(Some(&base), Some(&ours), Some(&theirs)),
            Err(FileConflict::Lines)
        );
        prop_assert_eq!(
            merge_file(Some(&base), Some(&ours), Some(&ours.clone())),
            Ok(Some(ours))
        );
    }

    #[test]
    fn the_kind_and_the_content_merge_apart(
        edits in disjoint_edits(),
        ours_kind in kind(),
    ) {
        let ours: Vec<Edit> = edits.iter().map(|(ours, _)| *ours).collect();
        let base = FileVersion {
            kind: FileKind::Regular,
            content: text(&vec![Edit::Keep; edits.len()], ""),
        };
        let theirs = FileVersion {
            kind: FileKind::Regular,
            content: text(&ours, "ours"),
        };
        let kind_side = FileVersion {
            kind: ours_kind,
            content: base.content.clone(),
        };
        let merged = merge_file(Some(&base), Some(&kind_side), Some(&theirs)).unwrap().unwrap();
        prop_assert_eq!(merged.kind, ours_kind);
        prop_assert_eq!(merged.content, theirs.content);
    }

    #[test]
    fn a_landing_that_meets_no_change_on_main_is_reparented(
        base in btree_map(0_u8..4, version(), 0..4),
        main in btree_map(0_u8..4, change(), 0..4),
        landing in btree_map(4_u8..8, change(), 0..4),
    ) {
        prop_assert_eq!(rebase(&base, &main, &landing), Ok(Rebase::Reparent));
    }

    #[test]
    fn a_landing_that_main_already_made_adds_nothing(
        base in btree_map(0_u8..4, version(), 0..4),
        landing in btree_map(0_u8..4, change(), 1..4),
    ) {
        prop_assert_eq!(rebase(&base, &landing, &landing), Ok(Rebase::Merge(BTreeMap::new())));
    }

    #[test]
    fn a_rebase_keeps_the_paths_main_did_not_change_and_adds_no_other(
        base in btree_map(0_u8..4, version(), 0..4),
        main in btree_map(0_u8..4, change(), 0..4),
        landing in btree_map(0_u8..4, change(), 0..4),
    ) {
        match rebase(&base, &main, &landing) {
            Ok(Rebase::Merge(merged)) => {
                for (path, change) in &landing {
                    if !main.contains_key(path) {
                        prop_assert_eq!(merged.get(path), Some(change));
                    }
                }
                prop_assert!(merged.keys().all(|path| landing.contains_key(path)));
            }
            Ok(Rebase::Reparent) => {
                prop_assert!(landing.keys().all(|path| !main.contains_key(path)));
            }
            Err(conflicts) => {
                let overlap = |path: &u8| landing.contains_key(path) && main.contains_key(path);
                prop_assert!(conflicts.paths().keys().all(overlap));
            }
        }
    }

    #[test]
    fn a_rebase_is_symmetric_in_its_two_deltas(
        base in btree_map(0_u8..4, version(), 0..4),
        main in btree_map(0_u8..4, change(), 0..4),
        landing in btree_map(0_u8..4, change(), 0..4),
    ) {
        let forward = settle(&base, &main, &landing);
        let backward = settle(&base, &landing, &main);
        prop_assert_eq!(forward, backward);
    }
}

/// What every path holds once `second` is rebased onto `first`, both against `base`, and the
/// result applied over `first`; `Err` where the rebase conflicts, with the conflicting paths.
fn settle(
    base: &BTreeMap<u8, FileVersion>,
    first: &BTreeMap<u8, Change>,
    second: &BTreeMap<u8, Change>,
) -> Result<BTreeMap<u8, Option<FileVersion>>, Vec<u8>> {
    let over_first = match rebase(base, first, second) {
        Ok(Rebase::Reparent) => second.clone(),
        Ok(Rebase::Merge(merged)) => merged,
        Err(conflicts) => return Err(conflicts.paths().keys().copied().collect()),
    };
    let mut state: BTreeMap<u8, Option<FileVersion>> = base
        .iter()
        .map(|(path, version)| (*path, Some(version.clone())))
        .collect();
    for delta in [first, &over_first] {
        for (path, change) in delta {
            state.insert(
                *path,
                match change {
                    Change::Write(version) => Some(version.clone()),
                    Change::Delete => None,
                },
            );
        }
    }
    state.retain(|_, version| version.is_some());
    Ok(state)
}
