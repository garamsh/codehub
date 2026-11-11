//! Changesets: deltas against their first parent, identified by their address (ADR 0011, ADR 0016).

use std::collections::BTreeMap;

use super::address::Address;
use super::encoding::{
    CHANGESET_KIND, DELETION, DecodeError, EncodeError, Reader, Writer, sorted_list,
};
use super::file::File;
use super::path::Path;

/// What a changeset does to one path.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Change {
    /// The path is removed.
    Deletion,
    /// The path holds this file.
    File(File),
}

/// A delta against its first parent: the parents, metadata, and the changed paths.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Changeset {
    /// Parent changeset identifiers. The first is the one the delta is against; empty for a
    /// repository's first changeset.
    pub parents: Vec<Address>,
    /// Who made the changeset.
    pub author: String,
    /// When, in seconds since 1970-01-01T00:00:00Z.
    pub timestamp: i64,
    /// Why.
    pub message: String,
    /// The changed paths, each with its change.
    pub changes: BTreeMap<Path, Change>,
}

impl Changeset {
    /// The changeset's encoding.
    ///
    /// # Errors
    ///
    /// [`EncodeError::TooLong`] where a field's length or count exceeds `u32`.
    pub fn encode(&self) -> Result<Vec<u8>, EncodeError> {
        let mut writer = Writer::new(CHANGESET_KIND);
        writer.count(self.parents.len())?;
        for parent in &self.parents {
            writer.address(parent)?;
        }
        writer.text(&self.author)?;
        writer.i64(self.timestamp);
        writer.text(&self.message)?;
        writer.count(self.changes.len())?;
        for (path, change) in &self.changes {
            writer.path(path)?;
            match change {
                Change::Deletion => writer.u8(DELETION),
                Change::File(file) => file.encode(&mut writer)?,
            }
        }
        Ok(writer.finish())
    }

    /// Decodes a changeset, refusing any input that is not the one encoding of its value.
    ///
    /// # Errors
    ///
    /// A [`DecodeError`] naming the first field that is not canonical.
    pub fn decode(input: &[u8]) -> Result<Self, DecodeError> {
        let mut reader = Reader::open(input, CHANGESET_KIND)?;
        let parent_count = reader.count()?;
        let parents = (0..parent_count)
            .map(|_| reader.address())
            .collect::<Result<Vec<_>, _>>()?;
        let author = reader.text()?;
        let timestamp = reader.i64()?;
        let message = reader.text()?;
        let changes = sorted_list(
            &mut reader,
            |reader| {
                let path = reader.path()?;
                let change = match reader.u8()? {
                    DELETION => Change::Deletion,
                    code => Change::File(File::decode(code, reader)?),
                };
                Ok((path, change))
            },
            |(path, _)| path,
        )?;
        reader.finish()?;
        Ok(Self {
            parents,
            author,
            timestamp,
            message,
            changes: changes.into_iter().collect(),
        })
    }

    /// The changeset's identifier: the address of its encoding.
    ///
    /// # Errors
    ///
    /// [`EncodeError::TooLong`] where the changeset has no encoding.
    pub fn id(&self) -> Result<Address, EncodeError> {
        Ok(Address::of(&self.encode()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::file::FileKind;

    const ENCODED: &str = "0101000000010100000020e47125968b3b71049fbc4802d1e40a71ea1359decfabacf70b34588037d4ff0c00000003616461000000006553f10000000004696e6974000000030000000100000006524541444d450101000000205891b5b522d5df086d0ff0b110fbd9d21bb4fc7163af34d08286a2e846f6be03000000020000000362696e0000000372756e020100000020a8076d3d28d21e02012b20eaf7dbf75409a6277134439025f282e368e3305abf00000001000000036f6c6400";
    const ID: &str = "sha256:624110722a2e3c912dbf748479081bbe0345ac5627a9a313673a74453a15514e";

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    fn fixed() -> Changeset {
        let file = |kind, content: &[u8]| {
            Change::File(File {
                kind,
                content: Address::of(content),
            })
        };
        Changeset {
            parents: vec![Address::of(b"parent")],
            author: "ada".to_owned(),
            timestamp: 1_700_000_000,
            message: "init".to_owned(),
            changes: BTreeMap::from([
                (
                    "README".parse().unwrap(),
                    file(FileKind::Regular, b"hello\n"),
                ),
                (
                    "bin/run".parse().unwrap(),
                    file(FileKind::Executable, b"#!/bin/sh\n"),
                ),
                ("old".parse().unwrap(), Change::Deletion),
            ]),
        }
    }

    #[test]
    fn encodes_a_fixed_changeset_to_pinned_bytes_and_identifier() {
        let changeset = fixed();
        assert_eq!(changeset.encode().unwrap(), hex(ENCODED));
        assert_eq!(changeset.id().unwrap().to_string(), ID);
    }

    #[test]
    fn decodes_its_own_encoding_to_the_same_changeset() {
        assert_eq!(Changeset::decode(&hex(ENCODED)), Ok(fixed()));
    }

    #[test]
    fn rejects_changes_out_of_order_or_repeated() {
        let mut changeset = fixed();
        changeset.changes = BTreeMap::from([
            ("b".parse().unwrap(), Change::Deletion),
            ("c".parse().unwrap(), Change::Deletion),
        ]);
        let sorted = changeset.encode().unwrap();
        // Each change is a one-component path (4 + 4 + 1 bytes) and a deletion byte.
        let (first, second) = (sorted.len() - 12, sorted.len() - 2);
        assert_eq!((sorted[first], sorted[second]), (b'b', b'c'));
        assert!(Changeset::decode(&sorted).is_ok());

        let mut swapped = sorted.clone();
        swapped[first] = b'c';
        swapped[second] = b'b';
        assert_eq!(Changeset::decode(&swapped), Err(DecodeError::NotSorted));

        let mut repeated = sorted.clone();
        repeated[second] = b'b';
        assert_eq!(Changeset::decode(&repeated), Err(DecodeError::NotSorted));
    }

    #[test]
    fn rejects_trailing_bytes() {
        let mut bytes = hex(ENCODED);
        assert!(Changeset::decode(&bytes).is_ok());
        bytes.push(0);
        assert_eq!(Changeset::decode(&bytes), Err(DecodeError::TrailingBytes));
    }

    #[test]
    fn rejects_an_unknown_format_version_and_the_wrong_object_kind() {
        let mut bytes = hex(ENCODED);
        bytes[0] = 0x02;
        assert_eq!(
            Changeset::decode(&bytes),
            Err(DecodeError::UnsupportedVersion(0x02))
        );
        let mut bytes = hex(ENCODED);
        bytes[1] = 0x02;
        assert_eq!(
            Changeset::decode(&bytes),
            Err(DecodeError::UnexpectedObjectKind(0x02))
        );
    }

    #[test]
    fn rejects_the_reserved_chunked_file_and_a_directory_as_a_change() {
        let mut changeset = fixed();
        changeset.changes = BTreeMap::from([(
            "a".parse().unwrap(),
            Change::File(File {
                kind: FileKind::Regular,
                content: Address::of(b""),
            }),
        )]);
        let bytes = changeset.encode().unwrap();
        // Entry kind: header, parents, author, timestamp, message, change count, one-component path "a".
        let at = 2 + 4 + 37 + 7 + 8 + 8 + 4 + 4 + 5;
        assert_eq!(bytes[at], 0x01);
        assert!(Changeset::decode(&bytes).is_ok());
        for code in [0x03, 0x04] {
            let mut bytes = bytes.clone();
            bytes[at] = code;
            assert_eq!(
                Changeset::decode(&bytes),
                Err(DecodeError::UnexpectedEntryKind(code))
            );
        }
    }

    #[test]
    fn rejects_a_digest_of_the_wrong_length() {
        let mut bytes = hex(ENCODED);
        // The first parent's digest length, after the header, the parent count and the algorithm byte.
        assert_eq!(bytes[2 + 4 + 1..2 + 4 + 5], [0, 0, 0, 32]);
        bytes[2 + 4 + 4] = 31;
        bytes.remove(2 + 4 + 5);
        assert_eq!(
            Changeset::decode(&bytes),
            Err(DecodeError::InvalidDigestLength(31))
        );
    }
}
