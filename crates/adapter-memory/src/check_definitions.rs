use std::collections::BTreeMap;
use std::sync::Mutex;

use app::domain::check::CheckDefinition;
use app::domain::path::Path;
use app::port::driven::check_definitions::{CheckDefinitions, CheckFileError};

use crate::lock;

/// What a taught text reads as.
type Reading = Result<Vec<CheckDefinition>, CheckFileError>;

/// Check definitions in memory. It reads no file format: it answers for the exact texts it was
/// taught with the definitions or the fault it was taught for each, and refuses any other text.
pub struct MemoryCheckDefinitions {
    file: Path,
    taught: Mutex<BTreeMap<Vec<u8>, Reading>>,
}

impl MemoryCheckDefinitions {
    /// Definitions kept at `file`, with no text taught.
    #[must_use]
    pub const fn new(file: Path) -> Self {
        Self {
            file,
            taught: Mutex::new(BTreeMap::new()),
        }
    }

    /// From now on reads `text` as `outcome`.
    pub fn teach(&self, text: &str, outcome: Reading) {
        lock(&self.taught).insert(text.as_bytes().to_vec(), outcome);
    }
}

impl CheckDefinitions for MemoryCheckDefinitions {
    fn file(&self) -> &Path {
        &self.file
    }

    fn parse(&self, bytes: &[u8]) -> Result<Vec<CheckDefinition>, CheckFileError> {
        lock(&self.taught).get(bytes).cloned().unwrap_or_else(|| {
            Err(CheckFileError::Syntax {
                line: 1,
                description: "text not taught to the in-memory definitions".to_owned(),
            })
        })
    }
}
