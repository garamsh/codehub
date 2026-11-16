use std::collections::BTreeMap;
use std::sync::Mutex;

use app::domain::check::CheckDefinition;
use app::domain::path::{Path, Prefix};
use app::domain::run::RunSettings;
use app::port::driven::check_definitions::{CheckDefinitions, CheckFileError};

use crate::lock;

/// What a taught text reads as.
type Reading = Result<Vec<CheckDefinition>, CheckFileError>;

/// What a taught run settings text reads as.
type RunReading = Result<RunSettings, CheckFileError>;

/// Check definitions in memory. It reads no file format: it answers for the exact texts it was
/// taught with the definitions or the fault it was taught for each, and refuses any other text.
/// The definitions it answers with are in the folder it is asked about.
pub struct MemoryCheckDefinitions {
    file: Path,
    run_file: Path,
    taught: Mutex<BTreeMap<Vec<u8>, Reading>>,
    taught_runs: Mutex<BTreeMap<Vec<u8>, RunReading>>,
}

impl MemoryCheckDefinitions {
    /// Definitions kept at `file`, and run settings kept at `run_file`, with no text taught.
    #[must_use]
    pub const fn new(file: Path, run_file: Path) -> Self {
        Self {
            file,
            run_file,
            taught: Mutex::new(BTreeMap::new()),
            taught_runs: Mutex::new(BTreeMap::new()),
        }
    }

    /// From now on reads the run settings `text` as `outcome`.
    pub fn teach_run(&self, text: &str, outcome: RunReading) {
        lock(&self.taught_runs).insert(text.as_bytes().to_vec(), outcome);
    }

    /// From now on reads `text` as `outcome`.
    pub fn teach(&self, text: &str, outcome: Reading) {
        lock(&self.taught).insert(text.as_bytes().to_vec(), outcome);
    }
}

#[async_trait::async_trait]
impl CheckDefinitions for MemoryCheckDefinitions {
    fn file(&self) -> &Path {
        &self.file
    }

    fn run_file(&self) -> &Path {
        &self.run_file
    }

    fn parse_run(&self, bytes: &[u8]) -> Result<RunSettings, CheckFileError> {
        lock(&self.taught_runs)
            .get(bytes)
            .cloned()
            .unwrap_or_else(|| {
                Err(CheckFileError::Syntax {
                    line: 1,
                    description: "text not taught to the in-memory definitions".to_owned(),
                })
            })
    }

    fn parse(&self, folder: &Prefix, bytes: &[u8]) -> Result<Vec<CheckDefinition>, CheckFileError> {
        let reading = lock(&self.taught).get(bytes).cloned().unwrap_or_else(|| {
            Err(CheckFileError::Syntax {
                line: 1,
                description: "text not taught to the in-memory definitions".to_owned(),
            })
        });
        reading.map(|definitions| {
            definitions
                .into_iter()
                .map(|definition| CheckDefinition {
                    folder: folder.clone(),
                    ..definition
                })
                .collect()
        })
    }
}
