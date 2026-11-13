//! Repositories held by the in-memory adapters, with the check definitions read from TOML, with
//! drafts staged and landed through the driving ports.

use adapter_toml::TomlCheckDefinitions;
use app::domain::check::CheckDefinition;
use app::port::driven::check_definitions::CheckFileError;
use test_support::{Definitions, MemoryDeployment};

/// The check definitions read from TOML.
pub struct Toml;

impl Definitions for Toml {
    type Adapter = TomlCheckDefinitions;

    fn start() -> TomlCheckDefinitions {
        TomlCheckDefinitions::new().unwrap()
    }

    fn teach(
        _adapter: &TomlCheckDefinitions,
        _text: &str,
        _outcome: Result<Vec<CheckDefinition>, CheckFileError>,
    ) {
        // The reader parses the file itself, so there is nothing to teach it.
    }
}

/// The in-memory adapters of one deployment, holding any number of repositories.
pub type Harness = MemoryDeployment<Toml>;
