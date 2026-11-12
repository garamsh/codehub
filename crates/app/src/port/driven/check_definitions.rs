//! Check definitions: where a repository keeps them and how their text is read (ADR 0035).

use std::fmt;

use crate::domain::check::CheckDefinition;
use crate::domain::name::CheckName;
use crate::domain::path::Path;

/// Reads the check definitions a repository holds in a file of its tree. The format of the file
/// is the adapter's.
pub trait CheckDefinitions {
    /// The path of the file in a repository's tree that holds its check definitions.
    fn file(&self) -> &Path;

    /// The check definitions in `bytes`, the content of [`CheckDefinitions::file`], in the order
    /// the file lists them.
    ///
    /// # Errors
    ///
    /// The [`CheckFileError`] naming the first fault in `bytes`.
    fn parse(&self, bytes: &[u8]) -> Result<Vec<CheckDefinition>, CheckFileError>;
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
    /// A check is not valid: a name, command, label or timeout the domain refuses, or a field
    /// the format does not know.
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
