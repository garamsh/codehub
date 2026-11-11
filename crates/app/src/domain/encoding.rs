//! Format version 1 of the object encoding: the field types every object is built from
//! (`docs/architecture/source-control.md` §Object encoding).

use std::fmt;

use super::address::Address;
use super::path::{Component, Path};

const FORMAT_VERSION: u8 = 0x01;
const SHA256: u8 = 0x01;
const SHA256_DIGEST_LENGTH: usize = 32;

pub(crate) const CHANGESET_KIND: u8 = 0x01;
pub(crate) const TREE_KIND: u8 = 0x02;
pub(crate) const FILE_VERSION_KIND: u8 = 0x03;

pub(crate) const DELETION: u8 = 0x00;
pub(crate) const FILE: u8 = 0x01;
pub(crate) const EXECUTABLE_FILE: u8 = 0x02;
pub(crate) const DIRECTORY: u8 = 0x03;

/// Why a value has no encoding.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EncodeError {
    /// A length or count does not fit the `u32` that prefixes it.
    TooLong(usize),
}

impl fmt::Display for EncodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLong(length) => write!(f, "{length} exceeds the u32 length prefix"),
        }
    }
}

impl std::error::Error for EncodeError {}

/// Why bytes are not the encoding of an object.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DecodeError {
    /// The input ends inside a field.
    UnexpectedEnd,
    /// Bytes follow the last field.
    TrailingBytes,
    /// The format-version byte is not one this decoder reads.
    UnsupportedVersion(u8),
    /// The object-kind byte is not the kind being decoded.
    UnexpectedObjectKind(u8),
    /// The address algorithm byte names no supported algorithm.
    UnsupportedAlgorithm(u8),
    /// A digest's length does not match its algorithm.
    InvalidDigestLength(usize),
    /// An entry-kind byte is unknown, reserved, or not allowed where it appears.
    UnexpectedEntryKind(u8),
    /// A `text` field is not valid UTF-8.
    InvalidUtf8,
    /// A component is empty, holds `/` or NUL, or is `.` or `..`.
    InvalidComponent,
    /// A path has no components.
    EmptyPath,
    /// Entries are out of order or repeated.
    NotSorted,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedEnd => f.write_str("input ends inside a field"),
            Self::TrailingBytes => f.write_str("bytes follow the last field"),
            Self::UnsupportedVersion(v) => write!(f, "unsupported format version {v:#04x}"),
            Self::UnexpectedObjectKind(k) => write!(f, "unexpected object kind {k:#04x}"),
            Self::UnsupportedAlgorithm(a) => write!(f, "unsupported address algorithm {a:#04x}"),
            Self::InvalidDigestLength(n) => write!(f, "digest of {n} bytes"),
            Self::UnexpectedEntryKind(k) => write!(f, "unexpected entry kind {k:#04x}"),
            Self::InvalidUtf8 => f.write_str("text is not valid UTF-8"),
            Self::InvalidComponent => f.write_str("invalid path component"),
            Self::EmptyPath => f.write_str("path has no components"),
            Self::NotSorted => f.write_str("entries are out of order or repeated"),
        }
    }
}

impl std::error::Error for DecodeError {}

/// Appends fields to an object's encoding, header first.
pub(crate) struct Writer(Vec<u8>);

impl Writer {
    pub(crate) fn new(kind: u8) -> Self {
        Self(vec![FORMAT_VERSION, kind])
    }

    pub(crate) fn u8(&mut self, value: u8) {
        self.0.push(value);
    }

    /// A `u32` length or count.
    pub(crate) fn count(&mut self, n: usize) -> Result<(), EncodeError> {
        let n = u32::try_from(n).map_err(|_| EncodeError::TooLong(n))?;
        self.0.extend_from_slice(&n.to_be_bytes());
        Ok(())
    }

    pub(crate) fn i64(&mut self, value: i64) {
        self.0.extend_from_slice(&value.to_be_bytes());
    }

    pub(crate) fn bytes(&mut self, bytes: &[u8]) -> Result<(), EncodeError> {
        self.count(bytes.len())?;
        self.0.extend_from_slice(bytes);
        Ok(())
    }

    pub(crate) fn text(&mut self, text: &str) -> Result<(), EncodeError> {
        self.bytes(text.as_bytes())
    }

    pub(crate) fn address(&mut self, address: &Address) -> Result<(), EncodeError> {
        self.u8(SHA256);
        self.bytes(address.digest())
    }

    pub(crate) fn component(&mut self, component: &Component) -> Result<(), EncodeError> {
        self.bytes(component.as_bytes())
    }

    pub(crate) fn path(&mut self, path: &Path) -> Result<(), EncodeError> {
        self.count(path.components().len())?;
        path.components()
            .iter()
            .try_for_each(|component| self.component(component))
    }

    pub(crate) fn finish(self) -> Vec<u8> {
        self.0
    }
}

/// Reads fields from an object's encoding, header first.
pub(crate) struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    /// Opens `input`, checking its format version and that it is an object of `kind`.
    pub(crate) fn open(input: &'a [u8], kind: u8) -> Result<Self, DecodeError> {
        let mut reader = Self(input);
        let version = reader.u8()?;
        if version != FORMAT_VERSION {
            return Err(DecodeError::UnsupportedVersion(version));
        }
        let found = reader.u8()?;
        if found != kind {
            return Err(DecodeError::UnexpectedObjectKind(found));
        }
        Ok(reader)
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        let (head, rest) = self
            .0
            .split_at_checked(n)
            .ok_or(DecodeError::UnexpectedEnd)?;
        self.0 = rest;
        Ok(head)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], DecodeError> {
        self.take(N)?
            .try_into()
            .map_err(|_| DecodeError::UnexpectedEnd)
    }

    pub(crate) fn u8(&mut self) -> Result<u8, DecodeError> {
        let [byte] = self.array()?;
        Ok(byte)
    }

    /// A `u32` length or count.
    pub(crate) fn count(&mut self) -> Result<usize, DecodeError> {
        let n = u32::from_be_bytes(self.array()?);
        usize::try_from(n).map_err(|_| DecodeError::UnexpectedEnd)
    }

    pub(crate) fn i64(&mut self) -> Result<i64, DecodeError> {
        Ok(i64::from_be_bytes(self.array()?))
    }

    pub(crate) fn bytes(&mut self) -> Result<&'a [u8], DecodeError> {
        let n = self.count()?;
        self.take(n)
    }

    pub(crate) fn text(&mut self) -> Result<String, DecodeError> {
        let bytes = self.bytes()?;
        String::from_utf8(bytes.to_vec()).map_err(|_| DecodeError::InvalidUtf8)
    }

    pub(crate) fn address(&mut self) -> Result<Address, DecodeError> {
        let algorithm = self.u8()?;
        if algorithm != SHA256 {
            return Err(DecodeError::UnsupportedAlgorithm(algorithm));
        }
        let digest = self.bytes()?;
        let digest: [u8; SHA256_DIGEST_LENGTH] = digest
            .try_into()
            .map_err(|_| DecodeError::InvalidDigestLength(digest.len()))?;
        Ok(Address::from_digest(digest))
    }

    pub(crate) fn component(&mut self) -> Result<Component, DecodeError> {
        Component::new(self.bytes()?.to_vec()).map_err(|_| DecodeError::InvalidComponent)
    }

    pub(crate) fn path(&mut self) -> Result<Path, DecodeError> {
        let n = self.count()?;
        let components = (0..n)
            .map(|_| self.component())
            .collect::<Result<Vec<_>, _>>()?;
        Path::new(components).map_err(|_| DecodeError::EmptyPath)
    }

    /// Ends decoding, refusing bytes after the last field.
    pub(crate) fn finish(self) -> Result<(), DecodeError> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(DecodeError::TrailingBytes)
        }
    }
}

/// Reads a `list<T>` whose elements must be strictly increasing by `key`.
pub(crate) fn sorted_list<'a, T, K: Ord>(
    reader: &mut Reader<'a>,
    mut element: impl FnMut(&mut Reader<'a>) -> Result<T, DecodeError>,
    key: impl Fn(&T) -> &K,
) -> Result<Vec<T>, DecodeError> {
    let n = reader.count()?;
    let mut elements: Vec<T> = Vec::new();
    for _ in 0..n {
        let next = element(reader)?;
        if elements.last().is_some_and(|last| key(last) >= key(&next)) {
            return Err(DecodeError::NotSorted);
        }
        elements.push(next);
    }
    Ok(elements)
}
