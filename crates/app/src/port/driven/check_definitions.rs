//! Check definitions: where a repository keeps them and how their text is read (ADR 0035,
//! ADR 0049).

use std::fmt;

use crate::domain::check::CheckDefinition;
use crate::domain::name::CheckName;
use crate::domain::path::{Path, Prefix};

/// Reads the check definitions a repository holds in the files of its tree. The format of the
/// files is the adapter's.
pub trait CheckDefinitions {
    /// The path, relative to a folder, of the file that holds the check definitions of the folder.
    fn file(&self) -> &Path;

    /// The check definitions in `bytes`, the content of [`CheckDefinitions::file`] of `folder`,
    /// in the order the file lists them, each with `folder` as its folder.
    ///
    /// # Errors
    ///
    /// The [`CheckFileError`] naming the first fault in `bytes`.
    fn parse(&self, folder: &Prefix, bytes: &[u8]) -> Result<Vec<CheckDefinition>, CheckFileError>;
}

/// Why the check definitions' file is not read.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum CheckFileError {
    /// The text is not in the file format; `line` counts from 1.
    Syntax {
        /// Where the fault is.
        line: usize,
        /// What it is.
        description: String,
    },
    /// A check is not valid: a name, command, label, timeout, pattern, need or path the domain
    /// refuses, or a field the format does not know.
    Invalid {
        /// Where the check's definition starts.
        line: usize,
        /// Why it is not valid.
        reason: String,
    },
    /// Two checks have one name.
    Duplicate {
        /// Where the second definition starts.
        line: usize,
        /// The name both have.
        name: CheckName,
    },
}

impl fmt::Display for CheckFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Syntax { line, description } => write!(f, "line {line}: {description}"),
            Self::Invalid { line, reason } => write!(f, "line {line}: {reason}"),
            Self::Duplicate { line, name } => {
                write!(f, "line {line}: a second check named {name}")
            }
        }
    }
}

impl std::error::Error for CheckFileError {}
