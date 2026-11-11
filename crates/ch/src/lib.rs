//! The `ch` command-line client and its composition site.

use std::fmt;
use std::io::Write;
use std::process::ExitCode;

/// A request the command line can express.
#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    /// Print the program name and version.
    Version,
}

/// Why the arguments did not name a [`Command`].
#[derive(Debug, PartialEq, Eq)]
pub enum CliError {
    /// No argument was given.
    MissingArgument,
    /// The first argument names no command.
    UnknownArgument(String),
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingArgument => f.write_str("ch: missing argument; try --version"),
            Self::UnknownArgument(arg) => write!(f, "ch: unknown argument '{arg}'; try --version"),
        }
    }
}

impl std::error::Error for CliError {}

/// Parses the arguments after the program name into a [`Command`].
///
/// # Errors
///
/// Returns [`CliError::MissingArgument`] for no arguments and
/// [`CliError::UnknownArgument`] for anything but `--version` alone.
pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Command, CliError> {
    let mut args = args.into_iter();
    match (args.next().as_deref(), args.next()) {
        (None, _) => Err(CliError::MissingArgument),
        (Some("--version"), None) => Ok(Command::Version),
        (Some(first), _) => Err(CliError::UnknownArgument(first.to_owned())),
    }
}

/// Runs `ch` over the arguments after the program name, reporting on stdout and stderr.
///
/// This is the composition site; it has no adapters to wire yet.
pub fn run<I: IntoIterator<Item = String>>(args: I) -> ExitCode {
    match parse(args) {
        Ok(Command::Version) => {
            match writeln!(std::io::stdout(), "ch {}", env!("CARGO_PKG_VERSION")) {
                Ok(()) => ExitCode::SUCCESS,
                Err(_) => ExitCode::FAILURE,
            }
        }
        Err(error) => {
            // Nowhere left to report a failed stderr write.
            let _ = writeln!(std::io::stderr(), "{error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn accepts_version_alone() {
        assert_eq!(parse(args(&["--version"])), Ok(Command::Version));
    }

    #[test]
    fn rejects_no_arguments() {
        assert_eq!(parse(args(&[])), Err(CliError::MissingArgument));
    }

    #[test]
    fn rejects_unknown_argument() {
        assert_eq!(
            parse(args(&["--help"])),
            Err(CliError::UnknownArgument("--help".to_owned()))
        );
    }

    #[test]
    fn rejects_version_followed_by_more() {
        assert_eq!(
            parse(args(&["--version", "extra"])),
            Err(CliError::UnknownArgument("--version".to_owned()))
        );
    }

    #[test]
    fn names_the_offending_argument_in_the_message() {
        let message = CliError::UnknownArgument("--help".to_owned()).to_string();
        assert!(message.contains("--help"));
    }
}
