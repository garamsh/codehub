//! Check definitions: the commands a repository asks to be run against each landed changeset
//! (ADR 0035).

use std::collections::BTreeSet;
use std::fmt;

use crate::domain::name::{CheckName, Label};
use crate::domain::path::Path;

/// The longest timeout a check may ask for, in seconds: one day.
const MAX_TIMEOUT: u64 = 86_400;

/// The timeout a check that names none has, in seconds.
const DEFAULT_TIMEOUT: u64 = 600;

/// Why text is not a check's command or timeout.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CheckError {
    /// The command is empty or only whitespace.
    EmptyCommand,
    /// The timeout is not between 1 second and one day; carries what was asked.
    TimeoutRange(u64),
}

impl fmt::Display for CheckError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyCommand => f.write_str("a command is not empty"),
            Self::TimeoutRange(seconds) => {
                write!(f, "a timeout is 1 to {MAX_TIMEOUT} seconds, not {seconds}")
            }
        }
    }
}

impl std::error::Error for CheckError {}

/// The command a check runs: the text a runner hands to its shell, never blank.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Command(String);

impl Command {
    /// The command `text`.
    ///
    /// # Errors
    ///
    /// [`CheckError::EmptyCommand`] where `text` holds only whitespace.
    pub fn new(text: &str) -> Result<Self, CheckError> {
        if text.trim().is_empty() {
            return Err(CheckError::EmptyCommand);
        }
        Ok(Self(text.to_owned()))
    }

    /// The command as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// How long a check may run before it is timed out, in whole seconds from 1 to one day.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Timeout(u64);

impl Timeout {
    /// The timeout of `seconds`.
    ///
    /// # Errors
    ///
    /// [`CheckError::TimeoutRange`] where `seconds` is 0 or more than one day.
    pub const fn new(seconds: u64) -> Result<Self, CheckError> {
        if seconds == 0 || seconds > MAX_TIMEOUT {
            return Err(CheckError::TimeoutRange(seconds));
        }
        Ok(Self(seconds))
    }

    /// The timeout of a check that names none.
    #[must_use]
    pub const fn default_timeout() -> Self {
        Self(DEFAULT_TIMEOUT)
    }

    /// The timeout in seconds.
    #[must_use]
    pub const fn seconds(self) -> u64 {
        self.0
    }
}

/// One check a repository defines.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CheckDefinition {
    /// Names the check within the repository.
    pub name: CheckName,
    /// What a runner runs.
    pub command: Command,
    /// What a runner must offer to take the check.
    pub labels: BTreeSet<Label>,
    /// Whether `last-green` waits for the check to pass.
    pub blocking: bool,
    /// How long the check may run.
    pub timeout: Timeout,
}

impl CheckDefinition {
    /// The blocking check that stands for the check definitions at `file` that could not be read.
    /// It is recorded as failed, so that `last-green` does not move past a changeset whose checks
    /// nobody could read.
    #[must_use]
    pub fn unreadable(file: &Path) -> Self {
        Self {
            name: CheckName::unreadable(),
            command: Command(file.to_string()),
            labels: BTreeSet::new(),
            blocking: true,
            timeout: Timeout::default_timeout(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_is_not_blank() {
        assert_eq!(Command::new("  \n"), Err(CheckError::EmptyCommand));
        assert_eq!(Command::new("make ci").unwrap().as_str(), "make ci");
    }

    #[test]
    fn a_timeout_is_from_one_second_to_one_day() {
        assert_eq!(Timeout::new(0), Err(CheckError::TimeoutRange(0)));
        assert_eq!(Timeout::new(86_401), Err(CheckError::TimeoutRange(86_401)));
        assert_eq!(Timeout::new(1).unwrap().seconds(), 1);
        assert_eq!(Timeout::new(86_400).unwrap().seconds(), 86_400);
    }

    #[test]
    fn the_unreadable_check_blocks_and_names_the_file() {
        let file: Path = ".codehub/checks.toml".parse().unwrap();
        let check = CheckDefinition::unreadable(&file);
        assert!(check.blocking);
        assert_eq!(check.command.as_str(), ".codehub/checks.toml");
    }
}
