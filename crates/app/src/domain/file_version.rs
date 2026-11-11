//! File-version nodes: one per changed file, each pointing at the previous version of its path,
//! and the per-path history they form (ADR 0011).

use std::collections::BTreeMap;
use std::fmt;

use super::address::Address;
use super::changeset::{Change, Changeset};
use super::derivation::{Derivation, Flag, Step};
use super::encoding::{DecodeError, EncodeError, FILE_VERSION_KIND, Reader, Writer};
use super::file::File;
use super::path::Path;

/// One version of the file at a path.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FileVersion {
    /// The changeset that introduced this version.
    pub changeset: Address,
    /// The path the file is at.
    pub path: Path,
    /// The file.
    pub file: File,
    /// The nodes of the same path in the parent changesets, in parent order; empty where no
    /// parent has the path.
    pub previous: Vec<Address>,
}

impl FileVersion {
    /// The node's encoding.
    ///
    /// # Errors
    ///
    /// [`EncodeError::TooLong`] where a field's length or count exceeds `u32`.
    pub fn encode(&self) -> Result<Vec<u8>, EncodeError> {
        let mut writer = Writer::new(FILE_VERSION_KIND);
        writer.address(&self.changeset)?;
        writer.path(&self.path)?;
        self.file.encode(&mut writer)?;
        writer.count(self.previous.len())?;
        for previous in &self.previous {
            writer.address(previous)?;
        }
        Ok(writer.finish())
    }

    /// Decodes a node, refusing any input that is not the one encoding of its value.
    ///
    /// # Errors
    ///
    /// A [`DecodeError`] naming the first field that is not canonical.
    pub fn decode(input: &[u8]) -> Result<Self, DecodeError> {
        let mut reader = Reader::open(input, FILE_VERSION_KIND)?;
        let changeset = reader.address()?;
        let path = reader.path()?;
        let code = reader.u8()?;
        let file = File::decode(code, &mut reader)?;
        let count = reader.count()?;
        let previous = (0..count)
            .map(|_| reader.address())
            .collect::<Result<Vec<_>, _>>()?;
        reader.finish()?;
        Ok(Self {
            changeset,
            path,
            file,
            previous,
        })
    }

    /// The address of the node's encoding.
    ///
    /// # Errors
    ///
    /// [`EncodeError::TooLong`] where the node has no encoding.
    pub fn address(&self) -> Result<Address, EncodeError> {
        Ok(Address::of(&self.encode()?))
    }
}

/// Why file-version nodes cannot be derived for a changeset.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum FileVersionError {
    /// A deletion names a path that has no node in the first parent.
    NotFound(Path),
    /// A changeset or a node has no encoding.
    Encode(EncodeError),
}

impl fmt::Display for FileVersionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(path) => write!(f, "{path} has no file version to delete"),
            Self::Encode(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for FileVersionError {}

impl From<EncodeError> for FileVersionError {
    fn from(error: EncodeError) -> Self {
        Self::Encode(error)
    }
}

/// File-version nodes as a derived type: a changeset's value maps each file's path to its
/// current node.
pub struct FileVersions;

impl Derivation for FileVersions {
    const FLAG: Flag = Flag::Deferred;
    type Value = BTreeMap<Path, Address>;
    type Object = FileVersion;
    type Error = FileVersionError;

    /// Starts from the first parent's value: an unchanged path keeps its node, a deleted path
    /// loses it, and a written path gets a new node pointing at each parent's node for it.
    fn step(
        &self,
        changeset: &Changeset,
        parents: &[Self::Value],
    ) -> Result<Step<Self::Value, FileVersion>, FileVersionError> {
        let id = changeset.id()?;
        let mut value = parents.first().cloned().unwrap_or_default();
        let mut objects = BTreeMap::new();
        for (path, change) in &changeset.changes {
            match change {
                Change::Deletion => {
                    value
                        .remove(path)
                        .ok_or_else(|| FileVersionError::NotFound(path.clone()))?;
                }
                Change::File(file) => {
                    let node = FileVersion {
                        changeset: id,
                        path: path.clone(),
                        file: *file,
                        previous: parents
                            .iter()
                            .filter_map(|parent| parent.get(path).copied())
                            .collect(),
                    };
                    let address = node.address()?;
                    value.insert(path.clone(), address);
                    objects.insert(address, node);
                }
            }
        }
        Ok(Step { value, objects })
    }
}

/// Why a history cannot be read.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HistoryError {
    /// A node the history reaches was not supplied; supply it and read again.
    MissingNode(Address),
}

impl fmt::Display for HistoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingNode(address) => write!(f, "file version {address} was not supplied"),
        }
    }
}

impl std::error::Error for HistoryError {}

/// The versions of `path`, newest first: its node in `value`, then each node's first previous.
/// A path with no node in `value` has no history.
///
/// # Errors
///
/// [`HistoryError::MissingNode`] where a node on the chain is missing from `nodes`.
pub fn history<'a>(
    path: &Path,
    value: &BTreeMap<Path, Address>,
    nodes: &'a BTreeMap<Address, FileVersion>,
) -> Result<Vec<&'a FileVersion>, HistoryError> {
    let mut versions = Vec::new();
    let mut next = value.get(path);
    while let Some(address) = next {
        let node = nodes
            .get(address)
            .ok_or(HistoryError::MissingNode(*address))?;
        versions.push(node);
        next = node.previous.first();
    }
    Ok(versions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::derivation::derive;
    use crate::domain::file::FileKind;

    const ENCODED: &str = "010301000000208f75dabae22a0e19461ce9fdd5ff2ef52b8456db857b0423a9b8c37710188b82000000020000000362696e0000000372756e020100000020a8076d3d28d21e02012b20eaf7dbf75409a6277134439025f282e368e3305abf000000020100000020a7937b64b8caa58f03721bb6bacf5c78cb235febe0e70b1b84cd99541461a08e010000002016367aacb67a4a017c8da8ab95682ccb390863780f7114dda0a0e0c55644c7c4";
    const ADDRESS: &str = "sha256:aa249dd32270f86147e451c04682ff43a415da6a7a27e47094ba6d8f0853371f";

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    fn fixed() -> FileVersion {
        FileVersion {
            changeset: Address::of(b"changeset"),
            path: "bin/run".parse().unwrap(),
            file: File {
                kind: FileKind::Executable,
                content: Address::of(b"#!/bin/sh\n"),
            },
            previous: vec![Address::of(b"first"), Address::of(b"second")],
        }
    }

    #[test]
    fn encodes_a_fixed_node_to_pinned_bytes_and_address() {
        assert_eq!(fixed().encode().unwrap(), hex(ENCODED));
        assert_eq!(fixed().address().unwrap().to_string(), ADDRESS);
    }

    #[test]
    fn decodes_its_own_encoding_to_the_same_node() {
        assert_eq!(FileVersion::decode(&hex(ENCODED)), Ok(fixed()));
    }

    #[test]
    fn rejects_a_deletion_a_directory_and_the_reserved_chunked_file_as_content() {
        let bytes = hex(ENCODED);
        // The content's entry kind: after the header, the changeset address and "bin/run".
        let at = 2 + 37 + 4 + 7 + 7;
        assert_eq!(bytes[at], 0x02);
        assert!(FileVersion::decode(&bytes).is_ok());
        for code in [0x00, 0x03, 0x04] {
            let mut bytes = bytes.clone();
            bytes[at] = code;
            assert_eq!(
                FileVersion::decode(&bytes),
                Err(DecodeError::UnexpectedEntryKind(code))
            );
        }
    }

    #[test]
    fn rejects_trailing_bytes_and_the_wrong_object_kind() {
        let mut bytes = hex(ENCODED);
        bytes.push(0);
        assert_eq!(FileVersion::decode(&bytes), Err(DecodeError::TrailingBytes));
        let mut bytes = hex(ENCODED);
        bytes[1] = 0x02;
        assert_eq!(
            FileVersion::decode(&bytes),
            Err(DecodeError::UnexpectedObjectKind(0x02))
        );
    }

    type Value = BTreeMap<Path, Address>;

    /// A log of changesets and every node derived for them.
    #[derive(Default)]
    struct Log {
        values: BTreeMap<Address, Value>,
        nodes: BTreeMap<Address, FileVersion>,
    }

    impl Log {
        /// Appends a changeset with `parents` and `changes`, derives its nodes, and returns its id.
        fn append(&mut self, parents: &[Address], changes: &[(&str, Change)]) -> Address {
            let changeset = Changeset {
                parents: parents.to_vec(),
                author: "ada".to_owned(),
                timestamp: i64::try_from(self.values.len()).unwrap(),
                message: String::new(),
                changes: changes
                    .iter()
                    .map(|(path, change)| (path.parse().unwrap(), *change))
                    .collect(),
            };
            let values: Vec<Value> = parents.iter().map(|p| self.values[p].clone()).collect();
            let step = derive(&FileVersions, &changeset, &values).unwrap();
            let id = changeset.id().unwrap();
            self.values.insert(id, step.value);
            self.nodes.extend(step.objects);
            id
        }

        /// The changesets that introduced each version of `path` at `at`, newest first.
        fn history(&self, at: Address, path: &str) -> Vec<Address> {
            history(&path.parse().unwrap(), &self.values[&at], &self.nodes)
                .unwrap()
                .into_iter()
                .map(|node| node.changeset)
                .collect()
        }

        fn node(&self, at: Address, path: &str) -> Option<Address> {
            self.values[&at].get(&path.parse().unwrap()).copied()
        }
    }

    fn write(content: &str) -> Change {
        Change::File(File {
            kind: FileKind::Regular,
            content: Address::of(content.as_bytes()),
        })
    }

    #[test]
    fn a_path_changed_in_changesets_1_3_and_5_has_the_history_5_3_1() {
        let mut log = Log::default();
        let one = log.append(&[], &[("a", write("1")), ("b", write("1"))]);
        let two = log.append(&[one], &[("b", write("2"))]);
        let three = log.append(&[two], &[("a", write("3"))]);
        let four = log.append(&[three], &[("b", write("4"))]);
        let five = log.append(&[four], &[("a", write("5"))]);

        assert_eq!(log.history(five, "a"), [five, three, one]);
        assert_eq!(log.history(five, "b"), [four, two, one]);
    }

    #[test]
    fn an_unchanged_path_inherits_its_node() {
        let mut log = Log::default();
        let one = log.append(&[], &[("a", write("1")), ("b", write("1"))]);
        let two = log.append(&[one], &[("b", write("2"))]);

        assert!(log.node(one, "a").is_some());
        assert_eq!(log.node(two, "a"), log.node(one, "a"));
        assert_ne!(log.node(two, "b"), log.node(one, "b"));
    }

    #[test]
    fn a_deleted_path_ends_its_chain() {
        let mut log = Log::default();
        let one = log.append(&[], &[("a", write("1"))]);
        let two = log.append(&[one], &[("a", Change::Deletion)]);
        let three = log.append(&[two], &[("a", write("3"))]);

        assert_eq!(log.history(one, "a"), [one]);
        assert_eq!(log.history(two, "a"), []);
        assert_eq!(log.history(three, "a"), [three]);
    }

    #[test]
    fn a_node_points_at_each_parents_node_in_parent_order() {
        let mut log = Log::default();
        let one = log.append(&[], &[("a", write("1"))]);
        let left = log.append(&[one], &[("a", write("left"))]);
        let right = log.append(&[one], &[("a", write("right")), ("r", write("r"))]);
        let merge = log.append(&[left, right], &[("a", write("merged"))]);

        let node = &log.nodes[&log.node(merge, "a").unwrap()];
        assert_eq!(
            node.previous,
            [log.node(left, "a").unwrap(), log.node(right, "a").unwrap()]
        );
        // Only the first parent is the delta's base: a path only the second parent has is absent.
        assert_eq!(log.node(merge, "r"), None);
        assert_eq!(log.history(merge, "a"), [merge, left, one]);
    }

    #[test]
    fn refuses_to_delete_a_path_with_no_node() {
        let changeset = |change| Changeset {
            parents: Vec::new(),
            author: "ada".to_owned(),
            timestamp: 0,
            message: String::new(),
            changes: BTreeMap::from([("a".parse().unwrap(), change)]),
        };
        // Control: the same path written rather than deleted.
        assert!(FileVersions.step(&changeset(write("a")), &[]).is_ok());

        assert_eq!(
            FileVersions.step(&changeset(Change::Deletion), &[]),
            Err(FileVersionError::NotFound("a".parse().unwrap()))
        );
    }

    #[test]
    fn asks_for_a_node_it_was_not_given() {
        let mut log = Log::default();
        let one = log.append(&[], &[("a", write("1"))]);
        let two = log.append(&[one], &[("a", write("2"))]);
        let first = log.node(one, "a").unwrap();
        assert_eq!(log.history(two, "a"), [two, one]);

        log.nodes.remove(&first);
        assert_eq!(
            history(&"a".parse().unwrap(), &log.values[&two], &log.nodes).err(),
            Some(HistoryError::MissingNode(first))
        );
    }
}
