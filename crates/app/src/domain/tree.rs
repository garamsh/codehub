//! Merkle directory trees and their derivation from a parent tree and a delta (ADR 0011).

use std::collections::BTreeMap;
use std::fmt;
use std::num::NonZeroUsize;

use super::address::Address;
use super::changeset::Change;
use super::encoding::{
    DIRECTORY, DecodeError, EncodeError, Reader, TREE_KIND, Writer, sorted_list,
};
use super::file::File;
use super::path::{Component, Path};

/// What a tree holds under one name.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Entry {
    /// A file.
    File(File),
    /// A directory, by the address of its tree.
    Directory(Address),
}

/// One directory: its entries by name. Only a root tree has no entries.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Tree {
    /// The entries, by name.
    pub entries: BTreeMap<Component, Entry>,
}

const EMPTY: Tree = Tree {
    entries: BTreeMap::new(),
};

impl Tree {
    /// The tree's encoding.
    ///
    /// # Errors
    ///
    /// [`EncodeError::TooLong`] where a name's length or the entry count exceeds `u32`.
    pub fn encode(&self) -> Result<Vec<u8>, EncodeError> {
        let mut writer = Writer::new(TREE_KIND);
        writer.count(self.entries.len())?;
        for (name, entry) in &self.entries {
            writer.component(name)?;
            match entry {
                Entry::File(file) => file.encode(&mut writer)?,
                Entry::Directory(tree) => {
                    writer.u8(DIRECTORY);
                    writer.address(tree)?;
                }
            }
        }
        Ok(writer.finish())
    }

    /// Decodes a tree, refusing any input that is not the one encoding of its value.
    ///
    /// # Errors
    ///
    /// A [`DecodeError`] naming the first field that is not canonical.
    pub fn decode(input: &[u8]) -> Result<Self, DecodeError> {
        let mut reader = Reader::open(input, TREE_KIND)?;
        let entries = sorted_list(
            &mut reader,
            |reader| {
                let name = reader.component()?;
                let entry = match reader.u8()? {
                    DIRECTORY => Entry::Directory(reader.address()?),
                    code => Entry::File(File::decode(code, reader)?),
                };
                Ok((name, entry))
            },
            |(name, _)| name,
        )?;
        reader.finish()?;
        Ok(Self {
            entries: entries.into_iter().collect(),
        })
    }

    /// The address of the tree's encoding.
    ///
    /// # Errors
    ///
    /// [`EncodeError::TooLong`] where the tree has no encoding.
    pub fn address(&self) -> Result<Address, EncodeError> {
        Ok(Address::of(&self.encode()?))
    }
}

/// Why a delta cannot be applied to a tree.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum DeriveError {
    /// A subtree the derivation must read was not supplied; supply it and derive again.
    MissingTree(Address),
    /// A deletion names a path that holds nothing.
    NotFound(Path),
    /// A change lies below a path that holds a file the delta does not delete.
    NotADirectory(Path),
    /// A file is written or deleted at a path that still holds a non-empty directory.
    IsADirectory(Path),
    /// A tree has no encoding.
    Encode(EncodeError),
}

impl fmt::Display for DeriveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingTree(address) => write!(f, "tree {address} was not supplied"),
            Self::NotFound(path) => write!(f, "{path} holds nothing to delete"),
            Self::NotADirectory(path) => write!(f, "{path} is a file, not a directory"),
            Self::IsADirectory(path) => write!(f, "{path} is a non-empty directory"),
            Self::Encode(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for DeriveError {}

impl From<EncodeError> for DeriveError {
    fn from(error: EncodeError) -> Self {
        Self::Encode(error)
    }
}

/// The result of a derivation: the new root and every tree it wrote.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Derived {
    /// The address of the new root tree.
    pub root: Address,
    /// Each tree the derivation produced, root included, by address. A subtree no change
    /// reaches is not here: the new root shares it with the parent.
    pub trees: BTreeMap<Address, Tree>,
}

/// Derives a changeset's root tree from its parent's root tree and its delta.
///
/// `known` supplies the parent's subtrees by address; only those on the path to a change
/// are read. A directory left with no entries is removed from its parent.
///
/// # Errors
///
/// A [`DeriveError`]: a subtree missing from `known`, or a delta that does not fit the tree.
pub fn derive(
    parent: &Tree,
    changes: &BTreeMap<Path, Change>,
    known: &BTreeMap<Address, Tree>,
) -> Result<Derived, DeriveError> {
    let mut levels = Levels::new();
    for (path, change) in changes {
        let mut below = &mut levels;
        for (index, name) in path.components().iter().enumerate() {
            let level = below.entry(name).or_insert_with(|| Level {
                path: path.truncated(NonZeroUsize::MIN.saturating_add(index)),
                here: None,
                below: Levels::new(),
            });
            if index + 1 == path.components().len() {
                level.here = Some(change);
            }
            below = &mut level.below;
        }
    }
    let mut trees = BTreeMap::new();
    let root = apply(parent, &levels, known, &mut trees)?;
    let address = root.address()?;
    trees.insert(address, root);
    Ok(Derived {
        root: address,
        trees,
    })
}

/// The changes in one directory, by name.
type Levels<'a> = BTreeMap<&'a Component, Level<'a>>;

/// The changes under one name: the change to the path itself, and those below it.
struct Level<'a> {
    path: Path,
    here: Option<&'a Change>,
    below: Levels<'a>,
}

fn apply(
    tree: &Tree,
    below: &Levels<'_>,
    known: &BTreeMap<Address, Tree>,
    out: &mut BTreeMap<Address, Tree>,
) -> Result<Tree, DeriveError> {
    let mut entries = tree.entries.clone();
    for (&name, level) in below {
        let existing = entries.remove(name);
        if let Some(entry) = apply_entry(existing, level, known, out)? {
            entries.insert(name.clone(), entry);
        }
    }
    Ok(Tree { entries })
}

fn apply_entry(
    existing: Option<Entry>,
    level: &Level<'_>,
    known: &BTreeMap<Address, Tree>,
    out: &mut BTreeMap<Address, Tree>,
) -> Result<Option<Entry>, DeriveError> {
    let lookup = |address: &Address| known.get(address).ok_or(DeriveError::MissingTree(*address));
    match level.here {
        None => {
            let base = match &existing {
                Some(Entry::Directory(address)) => lookup(address)?,
                Some(Entry::File(_)) => return Err(DeriveError::NotADirectory(level.path.clone())),
                None => &EMPTY,
            };
            store(apply(base, &level.below, known, out)?, out)
        }
        Some(Change::Deletion) => match existing {
            None => Err(DeriveError::NotFound(level.path.clone())),
            Some(Entry::Directory(_)) => Err(DeriveError::IsADirectory(level.path.clone())),
            // A file replaced by a directory: the changes below start from nothing.
            Some(Entry::File(_)) => store(apply(&EMPTY, &level.below, known, out)?, out),
        },
        Some(Change::File(file)) => {
            let base = match &existing {
                Some(Entry::Directory(address)) => lookup(address)?,
                Some(Entry::File(_)) if !level.below.is_empty() => {
                    return Err(DeriveError::NotADirectory(level.path.clone()));
                }
                Some(Entry::File(_)) | None => &EMPTY,
            };
            // A directory replaced by a file: the changes below must empty it.
            if apply(base, &level.below, known, out)?.entries.is_empty() {
                Ok(Some(Entry::File(*file)))
            } else {
                Err(DeriveError::IsADirectory(level.path.clone()))
            }
        }
    }
}

/// Records a non-empty subtree and returns its entry; an empty one is pruned.
fn store(tree: Tree, out: &mut BTreeMap<Address, Tree>) -> Result<Option<Entry>, DeriveError> {
    if tree.entries.is_empty() {
        return Ok(None);
    }
    let address = tree.address()?;
    out.insert(address, tree);
    Ok(Some(Entry::Directory(address)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::file::FileKind;

    const ENCODED: &str = "01020000000300000006524541444d450101000000205891b5b522d5df086d0ff0b110fbd9d21bb4fc7163af34d08286a2e846f6be030000000362696e0301000000200a0369a8dc42658354e0c10425f84a0c95226ba213f2cf571cc056cb757434a20000000672756e2e7368020100000020a8076d3d28d21e02012b20eaf7dbf75409a6277134439025f282e368e3305abf";
    const ADDRESS: &str = "sha256:1c0c3e956b5b92e69ebbcee2e68adde537a040bb779d36866230261cf0847299";

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    fn name(s: &str) -> Component {
        Component::new(s.as_bytes().to_vec()).unwrap()
    }

    fn file(content: &str) -> File {
        File {
            kind: FileKind::Regular,
            content: Address::of(content.as_bytes()),
        }
    }

    fn fixed() -> Tree {
        Tree {
            entries: BTreeMap::from([
                (name("README"), Entry::File(file("hello\n"))),
                (name("bin"), Entry::Directory(Address::of(b"subtree"))),
                (
                    name("run.sh"),
                    Entry::File(File {
                        kind: FileKind::Executable,
                        content: Address::of(b"#!/bin/sh\n"),
                    }),
                ),
            ]),
        }
    }

    fn delta(changes: &[(&str, Change)]) -> BTreeMap<Path, Change> {
        changes
            .iter()
            .map(|(path, change)| (path.parse().unwrap(), *change))
            .collect()
    }

    fn write(content: &str) -> Change {
        Change::File(file(content))
    }

    /// A root tree and every tree known so far.
    type State = (Tree, BTreeMap<Address, Tree>);

    fn empty() -> State {
        (Tree::default(), BTreeMap::new())
    }

    /// Derives `changes` from `parent`'s root.
    fn step(parent: &State, changes: &[(&str, Change)]) -> Result<State, DeriveError> {
        let derived = derive(&parent.0, &delta(changes), &parent.1)?;
        let mut known = parent.1.clone();
        known.extend(derived.trees);
        Ok((known[&derived.root].clone(), known))
    }

    fn directory(entry: &Entry) -> Address {
        match entry {
            Entry::Directory(address) => *address,
            Entry::File(_) => panic!("expected a directory"),
        }
    }

    #[test]
    fn encodes_a_fixed_tree_to_pinned_bytes_and_address() {
        assert_eq!(fixed().encode().unwrap(), hex(ENCODED));
        assert_eq!(fixed().address().unwrap().to_string(), ADDRESS);
    }

    #[test]
    fn decodes_its_own_encoding_to_the_same_tree() {
        assert_eq!(Tree::decode(&hex(ENCODED)), Ok(fixed()));
    }

    #[test]
    fn rejects_a_deletion_and_the_reserved_chunked_file_as_an_entry() {
        let bytes = hex(ENCODED);
        // The first entry's kind: after the header, the entry count and the name "README".
        let at = 2 + 4 + 4 + 6;
        assert_eq!(bytes[at], 0x01);
        assert!(Tree::decode(&bytes).is_ok());
        for code in [0x00, 0x04] {
            let mut bytes = bytes.clone();
            bytes[at] = code;
            assert_eq!(
                Tree::decode(&bytes),
                Err(DecodeError::UnexpectedEntryKind(code))
            );
        }
    }

    #[test]
    fn rejects_an_invalid_name() {
        let mut bytes = hex(ENCODED);
        // The first name's first byte.
        let at = 2 + 4 + 4;
        assert_eq!(bytes[at], b'R');
        bytes[at] = b'/';
        assert_eq!(Tree::decode(&bytes), Err(DecodeError::InvalidComponent));
    }

    #[test]
    fn unchanged_subtrees_keep_their_address() {
        let parent = step(
            &empty(),
            &[("src/a.rs", write("a")), ("docs/b.md", write("b"))],
        )
        .unwrap();
        let derived = derive(&parent.0, &delta(&[("src/a.rs", write("a2"))]), &parent.1).unwrap();
        let root = &derived.trees[&derived.root];

        let docs = directory(&root.entries[&name("docs")]);
        assert_eq!(docs, directory(&parent.0.entries[&name("docs")]));
        assert!(!derived.trees.contains_key(&docs));
        assert_ne!(
            directory(&root.entries[&name("src")]),
            directory(&parent.0.entries[&name("src")])
        );
    }

    #[test]
    fn a_deletion_removes_the_path_and_prunes_emptied_parents() {
        let parent = step(
            &empty(),
            &[("a/b/c.txt", write("c")), ("x.txt", write("x"))],
        )
        .unwrap();
        assert!(parent.0.entries.contains_key(&name("a")));

        let (root, _) = step(&parent, &[("a/b/c.txt", Change::Deletion)]).unwrap();

        assert_eq!(
            root.entries,
            BTreeMap::from([(name("x.txt"), Entry::File(file("x")))])
        );
    }

    #[test]
    fn the_same_content_yields_the_same_address() {
        let at_once = step(&empty(), &[("a/b.txt", write("b")), ("c.txt", write("c"))]).unwrap();
        let first = step(
            &empty(),
            &[("c.txt", write("old")), ("z/y.txt", write("y"))],
        )
        .unwrap();
        let in_steps = step(
            &first,
            &[
                ("a/b.txt", write("b")),
                ("c.txt", write("c")),
                ("z/y.txt", Change::Deletion),
            ],
        )
        .unwrap();

        assert_ne!(first.0, at_once.0);
        assert_eq!(in_steps.0.address(), at_once.0.address());
    }

    #[test]
    fn replaces_a_file_with_a_directory_and_an_emptied_directory_with_a_file() {
        let parent = step(&empty(), &[("a", write("file")), ("d/e", write("e"))]).unwrap();

        let (root, _) = step(
            &parent,
            &[
                ("a", Change::Deletion),
                ("a/inner", write("i")),
                ("d", write("now a file")),
                ("d/e", Change::Deletion),
            ],
        )
        .unwrap();

        assert!(matches!(root.entries[&name("a")], Entry::Directory(_)));
        assert_eq!(root.entries[&name("d")], Entry::File(file("now a file")));
    }

    #[test]
    fn refuses_a_delta_that_does_not_fit_the_tree() {
        let parent = step(&empty(), &[("a", write("file")), ("d/e", write("e"))]).unwrap();
        let path = |s: &str| s.parse::<Path>().unwrap();
        // Control: a delta naming the same paths in a way that fits.
        assert!(step(&parent, &[("a", Change::Deletion), ("d/e", write("e2"))]).is_ok());

        assert_eq!(
            step(&parent, &[("missing", Change::Deletion)]).err(),
            Some(DeriveError::NotFound(path("missing")))
        );
        assert_eq!(
            step(&parent, &[("a/below", write("x"))]).err(),
            Some(DeriveError::NotADirectory(path("a")))
        );
        assert_eq!(
            step(&parent, &[("d", Change::Deletion)]).err(),
            Some(DeriveError::IsADirectory(path("d")))
        );
        assert_eq!(
            step(&parent, &[("d", write("x"))]).err(),
            Some(DeriveError::IsADirectory(path("d")))
        );
    }

    #[test]
    fn refuses_a_file_at_a_path_with_another_path_under_it() {
        let parent = step(&empty(), &[("a", write("file")), ("d/e", write("e"))]).unwrap();
        let path = |s: &str| s.parse::<Path>().unwrap();
        // Control: the same file is accepted once nothing remains under its path.
        assert!(step(&parent, &[("d", write("x")), ("d/e", Change::Deletion)]).is_ok());

        // Under it in the tree, not deleted.
        assert_eq!(
            step(&parent, &[("d", write("x"))]).err(),
            Some(DeriveError::IsADirectory(path("d")))
        );
        // Under it in the same delta.
        assert_eq!(
            step(&parent, &[("n", write("x")), ("n/b", write("b"))]).err(),
            Some(DeriveError::IsADirectory(path("n")))
        );
        assert_eq!(
            step(&parent, &[("a", write("x")), ("a/b", write("b"))]).err(),
            Some(DeriveError::NotADirectory(path("a")))
        );
    }

    #[test]
    fn asks_for_a_subtree_it_was_not_given() {
        let parent = step(&empty(), &[("d/e", write("e"))]).unwrap();
        let d = directory(&parent.0.entries[&name("d")]);
        let changes = delta(&[("d/e", write("e2"))]);
        assert!(derive(&parent.0, &changes, &parent.1).is_ok());

        assert_eq!(
            derive(&parent.0, &changes, &BTreeMap::new()).err(),
            Some(DeriveError::MissingTree(d))
        );
    }
}
