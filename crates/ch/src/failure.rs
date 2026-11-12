//! Why a command failed, and the exit code that says so.

use std::fmt;
use std::io;

use crate::wire::Problem;

/// The exit code for a failure no other code names.
pub const EXIT_OTHER: u8 = 1;
/// The exit code for no login, or a token the server refused.
pub const EXIT_NOT_LOGGED_IN: u8 = 3;
/// The exit code for a landing the server rejected.
pub const EXIT_LANDING_REJECTED: u8 = 4;
/// The exit code for `checks --wait` ending with a blocking check that did not pass, or timing out.
pub const EXIT_CHECKS_NOT_PASSED: u8 = 4;
/// The exit code for an operation other than landing that was denied or found nothing.
pub const EXIT_DENIED: u8 = 5;

/// What a failed command reports; each variant carries the text to print.
#[derive(Debug, PartialEq, Eq)]
pub enum Failure {
    /// There is no login, or the server answered 401.
    NotLoggedIn(String),
    /// A landing was answered 409 or 403.
    LandingRejected(String),
    /// `checks --wait` ended with a blocking check that did not pass, or timed out.
    ChecksNotPassed(String),
    /// Any other operation was answered 403 or 404.
    Denied(String),
    /// Anything else.
    Other(String),
}

/// The operation a request belongs to, which decides how 403 is read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    /// Landing a draft.
    Land,
    /// Any other operation.
    Other,
}

impl Failure {
    /// The failure the server's answer `status` with `problem` means for `operation`.
    #[must_use]
    pub fn from_problem(operation: Operation, status: u16, problem: &Problem) -> Self {
        let mut message = problem.detail.clone();
        for path in &problem.paths {
            message.push('\n');
            message.push_str(path);
        }
        match (operation, status) {
            (_, 401) => Self::NotLoggedIn(message),
            (Operation::Land, 403 | 409) => Self::LandingRejected(message),
            (Operation::Land | Operation::Other, 403 | 404) => Self::Denied(message),
            (Operation::Land | Operation::Other, _) => Self::Other(message),
        }
    }

    /// The process exit code.
    #[must_use]
    pub const fn exit_code(&self) -> u8 {
        match self {
            Self::NotLoggedIn(_) => EXIT_NOT_LOGGED_IN,
            Self::LandingRejected(_) => EXIT_LANDING_REJECTED,
            Self::ChecksNotPassed(_) => EXIT_CHECKS_NOT_PASSED,
            Self::Denied(_) => EXIT_DENIED,
            Self::Other(_) => EXIT_OTHER,
        }
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotLoggedIn(message)
            | Self::LandingRejected(message)
            | Self::ChecksNotPassed(message)
            | Self::Denied(message)
            | Self::Other(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for Failure {}

impl From<io::Error> for Failure {
    fn from(error: io::Error) -> Self {
        Self::Other(error.to_string())
    }
}

impl From<reqwest::Error> for Failure {
    /// A transport failure, without the URL, which may carry a signature.
    fn from(error: reqwest::Error) -> Self {
        Self::Other(error.without_url().to_string())
    }
}
