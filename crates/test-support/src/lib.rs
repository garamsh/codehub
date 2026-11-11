//! Helpers that the tests of more than one package share: the check environment, a fresh
//! PostgreSQL database, a started `codehub` and a user's `ch`.
//!
//! Every helper panics when something is not as expected, as a failed assertion does, so a test
//! calls it without handling a `Result`. Every wait is bounded.

#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "the helpers panic as a failed assertion does, and the test exemptions in clippy.toml do not reach a library"
)]

mod ch;
mod database;
mod server;

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

pub use ch::{ChUser, Run};
pub use database::TestDatabase;
pub use server::Server;

/// How long a start, a shutdown or one run of `ch` may take.
const BOUND: Duration = Duration::from_secs(60);

/// The value of the environment variable `name`, which `make test` and `make e2e` set.
///
/// # Panics
///
/// Panics if it is not set.
#[must_use]
pub fn env(name: &str) -> String {
    std::env::var(name)
        .unwrap_or_else(|_| panic!("{name} is not set; run through `make test` or `make e2e`"))
}

/// A name no other test of any run uses: `prefix`, the process and a counter.
#[must_use]
pub fn unique(prefix: &str) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    format!(
        "{prefix}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}
