//! One user of the built `ch`.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use crate::{BOUND, unique};

/// What one run of `ch` did.
pub struct Run {
    /// The exit code.
    pub code: Option<i32>,
    /// What it wrote to stdout.
    pub stdout: Vec<u8>,
    /// What it wrote to stderr.
    pub stderr: String,
}

impl Run {
    /// Stdout as text.
    #[must_use]
    pub fn out(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }
}

/// A user of `ch`: a configuration directory of its own, so a state file of its own, removed when
/// the user is dropped.
pub struct ChUser {
    binary: PathBuf,
    home: PathBuf,
}

impl ChUser {
    /// A user of `binary`, the built `ch`, whose configuration directory is created under `parent`
    /// with a name that starts with `prefix`.
    ///
    /// # Panics
    ///
    /// Panics if the directory cannot be created.
    #[must_use]
    pub fn new(binary: &Path, parent: &Path, prefix: &str) -> Self {
        let home = parent.join(unique(prefix));
        std::fs::create_dir_all(&home).unwrap();
        Self {
            binary: binary.to_owned(),
            home,
        }
    }

    /// The user's configuration directory.
    #[must_use]
    pub fn home(&self) -> &Path {
        &self.home
    }

    /// The file `ch` keeps its state in.
    #[must_use]
    pub fn state_file(&self) -> PathBuf {
        self.home.join("ch").join("state.json")
    }

    /// `ch` with `arguments`, this user's configuration directory and no `HOME`, so that a state
    /// file anywhere else is not found, with stdout and stderr piped.
    #[must_use]
    pub fn command(&self, arguments: &[&str]) -> tokio::process::Command {
        let mut command = tokio::process::Command::new(&self.binary);
        command
            .args(arguments)
            .env("XDG_CONFIG_HOME", &self.home)
            .env_remove("HOME")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        command
    }

    /// Runs `ch` with `arguments` and waits for it.
    ///
    /// # Panics
    ///
    /// Panics if it cannot be started or does not finish within the bound.
    pub async fn ch(&self, arguments: &[&str]) -> Run {
        let child = self.command(arguments).spawn().unwrap();
        let output = tokio::time::timeout(BOUND, child.wait_with_output())
            .await
            .unwrap_or_else(|_| panic!("ch {arguments:?} did not finish within the bound"))
            .unwrap();
        Run {
            code: output.status.code(),
            stdout: output.stdout,
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }

    /// Runs `ch` with `arguments`, expects exit 0 and returns its stdout as text.
    ///
    /// # Panics
    ///
    /// Panics if `ch` exits with anything else.
    pub async fn ok(&self, arguments: &[&str]) -> String {
        let run = self.ch(arguments).await;
        assert_eq!(run.code, Some(0), "ch {arguments:?}: {}", run.stderr);
        run.out()
    }
}

impl Drop for ChUser {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.home);
    }
}
