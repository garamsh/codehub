//! A started `codehub` process.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use crate::{BOUND, TestDatabase, env};

/// A started `codehub` process, killed when dropped.
pub struct Server {
    child: Child,
    /// The URL the server answers on.
    pub base: String,
    _stderr: Receiver<String>,
}

impl Server {
    /// Starts `binary`, the built `codehub`, on `127.0.0.1:0` against `database`, the object store
    /// and the identity provider of the check environment, accepting tokens for `audience`, and
    /// waits for the line that names its address.
    ///
    /// # Panics
    ///
    /// Panics if a variable of the check environment is not set or the process does not print that
    /// line within the bound.
    #[must_use]
    pub fn start(binary: &Path, database: &TestDatabase, audience: &str) -> Self {
        let mut child = Command::new(binary)
            .env("CODEHUB_LISTEN", "127.0.0.1:0")
            .env("CODEHUB_POSTGRES_URL", database.url())
            .env("CODEHUB_S3_ENDPOINT", env("CODEHUB_TEST_S3_ENDPOINT"))
            .env("CODEHUB_S3_REGION", env("CODEHUB_TEST_S3_REGION"))
            .env("CODEHUB_S3_BUCKET", env("CODEHUB_TEST_S3_BUCKET"))
            .env("CODEHUB_S3_ACCESS_KEY", env("CODEHUB_TEST_S3_ACCESS_KEY"))
            .env("CODEHUB_S3_SECRET_KEY", env("CODEHUB_TEST_S3_SECRET_KEY"))
            .env("CODEHUB_OIDC_ISSUER", env("CODEHUB_TEST_OIDC_ISSUER"))
            .env("CODEHUB_OIDC_AUDIENCE", audience)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let (sender, stderr) = channel();
        let pipe = child.stderr.take().unwrap();
        // The pipe is read to its end so the child never blocks writing to it.
        std::thread::spawn(move || {
            for line in BufReader::new(pipe).lines().map_while(Result::ok) {
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        let mut server = Self {
            child,
            base: String::new(),
            _stderr: stderr,
        };
        let line = server
            ._stderr
            .recv_timeout(BOUND)
            .expect("codehub printed a line to stderr within the bound");
        let address = line
            .strip_prefix("codehub listening on ")
            .unwrap_or_else(|| panic!("codehub did not start: {line}"));
        server.base = format!("http://{address}");
        server
    }

    /// Sends the signal `name` (`TERM`, `INT`) to the process and returns how it exited.
    ///
    /// # Panics
    ///
    /// Panics if the signal cannot be sent or the process is still running after the bound.
    pub fn signal(&mut self, name: &str) -> ExitStatus {
        let sent = Command::new("kill")
            .args([format!("-{name}"), self.child.id().to_string()])
            .status()
            .unwrap();
        assert!(sent.success(), "kill -{name} failed");
        let deadline = Instant::now() + BOUND;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            assert!(Instant::now() < deadline, "codehub is still running");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if self.child.try_wait().unwrap().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}
