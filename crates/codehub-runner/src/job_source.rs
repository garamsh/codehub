//! The job source port: the runner's conversation with a server about jobs.

use std::fmt;
use std::future::Future;
use std::path::{Component, Path};
use std::str::FromStr;
use std::time::Duration;

/// Names a job on the server.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct JobId(u64);

impl JobId {
    /// The job the server numbers `value`.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// The server's number.
    #[must_use]
    pub const fn value(self) -> u64 {
        self.0
    }
}

impl fmt::Display for JobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// The SHA-256 digest of a file's bytes, written `sha256:<hex>`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Address([u8; 32]);

impl Address {
    /// The address of the bytes that hash to `digest`.
    #[must_use]
    pub const fn new(digest: [u8; 32]) -> Self {
        Self(digest)
    }
}

/// Why text is not an address.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct AddressError;

impl fmt::Display for AddressError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an address is \"sha256:\" and 64 lowercase hex digits")
    }
}

impl std::error::Error for AddressError {}

impl FromStr for Address {
    type Err = AddressError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let hex = text.strip_prefix("sha256:").ok_or(AddressError)?;
        if hex.len() != 64 {
            return Err(AddressError);
        }
        let mut digest = [0_u8; 32];
        for (byte, pair) in digest.iter_mut().zip(hex.as_bytes().chunks(2)) {
            let digit = |c: u8| match c {
                b'0'..=b'9' => Some(c - b'0'),
                b'a'..=b'f' => Some(c - b'a' + 10),
                _ => None,
            };
            let [high, low] = pair else {
                return Err(AddressError);
            };
            *byte = digit(*high).ok_or(AddressError)? << 4 | digit(*low).ok_or(AddressError)?;
        }
        Ok(Self(digest))
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("sha256:")?;
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

/// How a file is fetched: a URL and the headers its grant requires.
#[derive(Clone, Debug)]
pub struct Download {
    /// Where to `GET` the bytes.
    pub url: String,
    /// The headers to send with the request.
    pub headers: Vec<(String, String)>,
}

/// Why text is not a relative path.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RelativePathError;

impl fmt::Display for RelativePathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a file path is not empty, not absolute, and has only normal components")
    }
}

impl std::error::Error for RelativePathError {}

/// A path that stays inside the directory it is joined to: not empty, not absolute, made of
/// normal components only.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RelativePath(String);

impl RelativePath {
    /// The path as a [`Path`], to join to a directory.
    #[must_use]
    pub fn as_path(&self) -> &Path {
        Path::new(&self.0)
    }
}

impl fmt::Display for RelativePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for RelativePath {
    type Err = RelativePathError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if text.is_empty()
            || !Path::new(text)
                .components()
                .all(|component| matches!(component, Component::Normal(_)))
        {
            return Err(RelativePathError);
        }
        Ok(Self(text.to_owned()))
    }
}

/// One file of the changeset a job checks.
#[derive(Clone, Debug)]
pub struct JobFile {
    /// Where the file sits in the tree, `/`-separated.
    pub path: RelativePath,
    /// What its bytes hash to.
    pub address: Address,
    /// Whether it is written executable.
    pub executable: bool,
    /// How to fetch it.
    pub download: Download,
}

/// A job a claim returned.
#[derive(Clone, Debug)]
pub struct Job {
    /// Names the job.
    pub id: JobId,
    /// The organisation of the repository.
    pub organisation: String,
    /// The repository's number.
    pub repository: u64,
    /// The changeset the check runs against.
    pub changeset: Address,
    /// The check's name.
    pub check: String,
    /// The command to run under `/bin/sh -c`.
    pub command: String,
    /// How often to heartbeat.
    pub heartbeat_interval: Duration,
    /// The files of the changeset.
    pub files: Vec<JobFile>,
}

/// Why a runner is told to stop its job.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StopReason {
    /// An administrator cancelled the job.
    Cancelled,
    /// The job ran past its timeout.
    TimedOut,
}

/// The server's answer to a heartbeat.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Heartbeat {
    /// Keep running.
    Continue,
    /// Stop the job, and say why when reporting.
    Stop(StopReason),
}

/// How a job ended.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    /// The command exited 0.
    Passed,
    /// The command exited otherwise, or could not run.
    Failed,
    /// The runner stopped the job because it ran too long.
    TimedOut,
    /// The runner stopped the job because it was cancelled.
    Cancelled,
}

/// What a runner reports when a job ends.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Report {
    /// How the job ended.
    pub status: Status,
    /// The command's exit code, where it has one.
    pub exit_code: Option<i32>,
    /// The end of the output.
    pub output: String,
}

/// Why a conversation with the server failed.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SourceError {
    /// No token could be obtained.
    Token(String),
    /// The server could not be reached or its answer could not be read.
    Transport(String),
    /// The server does not hold the job for this runner.
    NotHeld,
    /// The server answered with this status.
    Status(u16),
    /// The server's answer is not what the API says.
    Malformed(String),
}

impl fmt::Display for SourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Token(reason) => write!(f, "no token: {reason}"),
            Self::Transport(reason) => write!(f, "the server did not answer: {reason}"),
            Self::NotHeld => f.write_str("the server does not hold the job for this runner"),
            Self::Status(status) => write!(f, "the server answered {status}"),
            Self::Malformed(reason) => write!(f, "the server's answer is malformed: {reason}"),
        }
    }
}

impl std::error::Error for SourceError {}

/// The claim, heartbeat and report conversation with a server.
pub trait JobSource {
    /// Waits for the server to hand out a job; `None` where it held the request and had none.
    ///
    /// # Errors
    ///
    /// A [`SourceError`] where the conversation failed.
    fn claim(&self) -> impl Future<Output = Result<Option<Job>, SourceError>> + Send;

    /// Tells the server `job` is still running, and returns whether to go on.
    ///
    /// # Errors
    ///
    /// A [`SourceError`] where the conversation failed; [`SourceError::NotHeld`] where the
    /// server no longer holds the job for this runner.
    fn heartbeat(&self, job: JobId) -> impl Future<Output = Result<Heartbeat, SourceError>> + Send;

    /// Reports how `job` ended, which ends it on the server.
    ///
    /// # Errors
    ///
    /// A [`SourceError`] where the conversation failed.
    fn report(
        &self,
        job: JobId,
        report: &Report,
    ) -> impl Future<Output = Result<(), SourceError>> + Send;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_round_trips_through_its_text() {
        let text = format!("sha256:{}", "0123456789abcdef".repeat(4));
        let address: Address = text.parse().unwrap();
        assert_eq!(address.to_string(), text);
    }

    #[test]
    fn a_relative_path_stays_inside_its_directory() {
        for text in ["../escape", "/abs", "", "a/../../b", "./a"] {
            assert!(text.parse::<RelativePath>().is_err(), "{text:?}");
        }
        for text in ["a", "a/b", ".codehub/checks.toml"] {
            assert_eq!(text.parse::<RelativePath>().unwrap().to_string(), text);
        }
    }

    #[test]
    fn refuses_text_that_is_not_an_address() {
        let hex = "0".repeat(64);
        for text in [
            hex.clone(),
            format!("sha1:{hex}"),
            format!("sha256:{}", "0".repeat(63)),
            format!("sha256:{}", "A".repeat(64)),
            format!("sha256:{}", "g".repeat(64)),
        ] {
            assert!(text.parse::<Address>().is_err(), "{text}");
        }
        assert!(format!("sha256:{hex}").parse::<Address>().is_ok());
    }
}
