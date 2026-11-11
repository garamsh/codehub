//! The `ch` command-line client and its composition site.

use std::ffi::OsString;
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
            Self::UnknownArgument(arg) => write!(f, "ch: unknown argument {arg:?}; try --version"),
        }
    }
}

impl std::error::Error for CliError {}

/// Parses the arguments after the program name into a [`Command`].
///
/// # Errors
///
/// Returns [`CliError::MissingArgument`] for no arguments and
/// [`CliError::UnknownArgument`] for anything but `--version` alone. A non-Unicode
/// argument is unknown and is held lossily.
pub fn parse<I: IntoIterator<Item = OsString>>(args: I) -> Result<Command, CliError> {
    let mut args = args.into_iter();
    match (args.next(), args.next()) {
        (None, _) => Err(CliError::MissingArgument),
        (Some(first), None) if first == "--version" => Ok(Command::Version),
        (Some(first), _) => Err(CliError::UnknownArgument(
            first.to_string_lossy().into_owned(),
        )),
    }
}

/// Runs `ch` over the arguments after the program name, reporting on stdout and stderr.
///
/// This is the composition site; it has no adapters to wire yet.
pub fn run<I: IntoIterator<Item = OsString>>(args: I) -> ExitCode {
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

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
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

    #[cfg(unix)]
    #[test]
    fn rejects_non_unicode_argument() {
        use std::os::unix::ffi::OsStringExt;

        let bad = OsString::from_vec(vec![b'a', 0xff]);
        assert_eq!(
            parse([bad]),
            Err(CliError::UnknownArgument("a\u{fffd}".to_owned()))
        );
    }

    #[test]
    fn keeps_the_message_on_one_line() {
        let message = CliError::UnknownArgument("a\nb".to_owned()).to_string();
        assert_eq!(message.lines().count(), 1);
    }

    #[test]
    fn names_the_offending_argument_in_the_message() {
        let message = CliError::UnknownArgument("--help".to_owned()).to_string();
        assert!(message.contains("--help"));
    }
}
