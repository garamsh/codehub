//! The local state: the server URL, the token and the selected repository (ADR 0025).

use std::ffi::OsString;
use std::fmt;
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::failure::Failure;

/// An `<org>/<repo>` pair.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct RepositoryRef {
    /// The organisation's name.
    pub organisation: String,
    /// The repository's name.
    pub repository: String,
}

/// The text is not `<org>/<repo>`.
#[derive(Debug, PartialEq, Eq)]
pub struct RepositoryRefError(String);

impl fmt::Display for RepositoryRefError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?} is not <org>/<repo>", self.0)
    }
}

impl std::error::Error for RepositoryRefError {}

impl FromStr for RepositoryRef {
    type Err = RepositoryRefError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text.split_once('/') {
            Some((organisation, repository))
                if !organisation.is_empty()
                    && !repository.is_empty()
                    && !repository.contains('/') =>
            {
                Ok(Self {
                    organisation: organisation.to_owned(),
                    repository: repository.to_owned(),
                })
            }
            Some(_) | None => Err(RepositoryRefError(text.to_owned())),
        }
    }
}

impl TryFrom<String> for RepositoryRef {
    type Error = RepositoryRefError;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        text.parse()
    }
}

impl From<RepositoryRef> for String {
    fn from(reference: RepositoryRef) -> Self {
        format!("{}/{}", reference.organisation, reference.repository)
    }
}

/// What `ch` keeps between runs.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    /// The server's base URL.
    pub server: String,
    /// The access token `ch login` stored; absent where the state was made with a token source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// The repository `use` selected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<RepositoryRef>,
}

/// Where the state file is: `$XDG_CONFIG_HOME/ch/state.json`, else `$HOME/.config/ch/state.json`.
/// An empty or relative `XDG_CONFIG_HOME` is ignored, as the XDG specification says.
#[must_use]
pub fn path(config_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    let base = config_home
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            home.filter(|home| !home.is_empty())
                .map(|home| PathBuf::from(home).join(".config"))
        })?;
    Some(base.join("ch").join("state.json"))
}

/// The state file at one path.
pub struct StateFile {
    path: PathBuf,
}

impl StateFile {
    /// The state file at `path`.
    #[must_use]
    pub const fn at(path: PathBuf) -> Self {
        Self { path }
    }

    /// The state, or `None` where nothing was saved.
    ///
    /// # Errors
    ///
    /// Fails where the file cannot be read or does not parse.
    pub fn load(&self) -> Result<Option<State>, Failure> {
        let text = match fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(self.failure(&error)),
        };
        serde_json::from_str(&text)
            .map(Some)
            .map_err(|error| self.failure(&error))
    }

    /// The state, or the failure that says to log in.
    ///
    /// # Errors
    ///
    /// Fails where there is no state, or [`StateFile::load`] fails.
    pub fn require(&self) -> Result<State, Failure> {
        self.load()?
            .ok_or_else(|| Failure::NotLoggedIn("not logged in; run `ch login`".to_owned()))
    }

    /// Writes `state`, readable by the user only.
    ///
    /// # Errors
    ///
    /// Fails where the directory or the file cannot be written.
    pub fn save(&self, state: &State) -> Result<(), Failure> {
        let text = serde_json::to_string(state).map_err(|error| self.failure(&error))?;
        self.write(&text).map_err(|error| self.failure(&error))
    }

    fn write(&self, text: &str) -> io::Result<()> {
        if let Some(directory) = self.path.parent() {
            DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(directory)?;
        }
        // A file is created whole and renamed, so it is never seen with another mode.
        let staging = self.path.with_extension("json.tmp");
        remove_if_present(&staging)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&staging)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        fs::rename(&staging, &self.path)
    }

    fn failure(&self, error: &dyn fmt::Display) -> Failure {
        Failure::Other(format!("{}: {error}", self.path.display()))
    }
}

fn remove_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        Ok(()) | Err(_) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_an_organisation_and_a_repository() {
        assert_eq!(
            "acme/app".parse(),
            Ok(RepositoryRef {
                organisation: "acme".to_owned(),
                repository: "app".to_owned(),
            })
        );
    }

    #[test]
    fn rejects_text_that_is_not_two_names_around_one_slash() {
        for text in ["acme", "/app", "acme/", "a/b/c", ""] {
            assert!(text.parse::<RepositoryRef>().is_err(), "{text:?}");
        }
    }

    #[test]
    fn prefers_xdg_config_home_over_home() {
        let path = path(Some("/xdg".into()), Some("/home/u".into()));
        assert_eq!(path, Some(PathBuf::from("/xdg/ch/state.json")));
    }

    #[test]
    fn falls_back_to_home_without_an_absolute_xdg_config_home() {
        for xdg in [None, Some(OsString::new()), Some("relative".into())] {
            let path = path(xdg, Some("/home/u".into()));
            assert_eq!(path, Some(PathBuf::from("/home/u/.config/ch/state.json")));
        }
    }

    #[test]
    fn has_no_path_without_either() {
        assert_eq!(path(None, None), None);
    }

    #[test]
    fn writes_a_private_file_in_a_private_directory() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("ch-state-{}", std::process::id()));
        let file = StateFile::at(root.join("ch").join("state.json"));
        let state = State {
            server: "http://s".to_owned(),
            token: Some("t".to_owned()),
            repository: None,
        };
        file.save(&state).unwrap();
        file.save(&state).unwrap();
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&root.join("ch")), 0o700);
        assert_eq!(mode(&root.join("ch").join("state.json")), 0o600);
        fs::remove_dir_all(&root).unwrap();
    }
}
