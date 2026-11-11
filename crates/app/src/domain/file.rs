//! A file as a changeset or a tree names it: its mode and its content's address.

use super::address::Address;
use super::encoding::{DecodeError, EXECUTABLE_FILE, EncodeError, FILE, Reader, Writer};

/// The file mode. The executable bit is the only one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FileKind {
    /// A file without the executable bit.
    Regular,
    /// A file with the executable bit.
    Executable,
}

/// A file: its mode and the address of its blob.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct File {
    /// The file mode.
    pub kind: FileKind,
    /// The address of the file's content.
    pub content: Address,
}

impl File {
    /// Reads the content's address of a file whose entry-kind byte, already read, is `code`.
    pub(crate) fn decode(code: u8, reader: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let kind = match code {
            FILE => FileKind::Regular,
            EXECUTABLE_FILE => FileKind::Executable,
            other => return Err(DecodeError::UnexpectedEntryKind(other)),
        };
        Ok(Self {
            kind,
            content: reader.address()?,
        })
    }

    /// Writes the entry-kind byte, then the content's address.
    pub(crate) fn encode(&self, writer: &mut Writer) -> Result<(), EncodeError> {
        writer.u8(match self.kind {
            FileKind::Regular => FILE,
            FileKind::Executable => EXECUTABLE_FILE,
        });
        writer.address(&self.content)
    }
}
