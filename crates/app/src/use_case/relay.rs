//! The relay: delivers the repository changes of the directory outbox through the directory
//! changes, at least once and in sequence order for each repository (ADR 0044).
//!
//! The relay claims one repository's entries at a time and deletes each entry once the receiver
//! accepts it. A repository whose delivery fails is held back, for a backoff that doubles from
//! [`FIRST_BACKOFF`] to [`MAX_BACKOFF`] while it keeps failing, and the other repositories go on.
//! No entry is dropped: one the receiver refuses stays and is tried again at the cap. Every
//! failure is logged with the repository and the sequence.

use std::collections::BTreeMap;
use std::future::{Future, poll_fn};
use std::pin::pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::Poll;
use std::time::Duration;

use crate::domain::repository::RepositoryId;
use crate::port::driven::directory_changes::DirectoryChanges;
use crate::port::driven::directory_outbox::DirectoryOutbox;

/// How long a repository is held back after its first failed delivery.
pub const FIRST_BACKOFF: Duration = Duration::from_secs(1);
/// The longest a repository is held back; a delivery that keeps failing is tried at this interval.
pub const MAX_BACKOFF: Duration = Duration::from_secs(60);
/// How long [`DirectoryRelay::run`] waits where nothing is pending.
pub const IDLE_WAIT: Duration = Duration::from_secs(1);
/// How long the relay's claim on a repository's entries lasts.
pub const LEASE: Duration = Duration::from_secs(30);

/// What the relay takes from outside the application: the time, a way to wait, and where its
/// log lines go.
pub trait RelayEnvironment {
    /// Seconds since 1970-01-01T00:00:00Z.
    fn now(&self) -> i64;

    /// Completes after `duration`.
    fn sleep(&self, duration: Duration) -> impl Future<Output = ()> + Send;

    /// Writes one line to the log.
    fn log(&self, line: &str);
}

/// What the creation of a repository asks once it has committed.
pub trait Delivery {
    /// Delivers the entries of `repository`, returning when that is done or given up on. Where
    /// the delivery is bounded in time, the implementation bounds it.
    fn deliver(&self, repository: RepositoryId) -> impl Future<Output = ()> + Send;
}

/// Delivers nothing.
impl Delivery for () {
    async fn deliver(&self, _repository: RepositoryId) {}
}

/// How one pass over the outbox ended.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pass {
    /// No repository was claimable.
    Idle,
    /// The outbox could not be claimed from.
    Unavailable,
    /// Every claimed entry was delivered and deleted.
    Delivered,
    /// A delivery failed; the repository is held back.
    Failed,
}

/// Delivers the entries of the directory outbox.
pub struct DirectoryRelay<O, C, E> {
    outbox: Arc<O>,
    changes: Arc<C>,
    environment: Arc<E>,
    creation_wait: Duration,
    /// Failed deliveries in a row, for each repository that has any.
    failures: Mutex<BTreeMap<RepositoryId, u32>>,
}

impl<O, C, E> DirectoryRelay<O, C, E> {
    /// A relay over `outbox` and `changes`. [`Delivery::deliver`] waits at most `creation_wait`.
    pub const fn new(
        outbox: Arc<O>,
        changes: Arc<C>,
        environment: Arc<E>,
        creation_wait: Duration,
    ) -> Self {
        Self {
            outbox,
            changes,
            environment,
            creation_wait,
            failures: Mutex::new(BTreeMap::new()),
        }
    }
}

/// How long a repository is held back after `failures` failed deliveries in a row.
#[must_use]
pub fn backoff(failures: u32) -> Duration {
    let doublings = failures.saturating_sub(1);
    let factor = 1u32.checked_shl(doublings).unwrap_or(u32::MAX);
    FIRST_BACKOFF.saturating_mul(factor).min(MAX_BACKOFF)
}

/// `duration` in whole seconds, as the outbox counts them.
fn seconds(duration: Duration) -> i64 {
    i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
}

impl<O, C, E> DirectoryRelay<O, C, E>
where
    O: DirectoryOutbox + Send + Sync,
    C: DirectoryChanges + Send + Sync,
    E: RelayEnvironment + Send + Sync,
{
    /// Delivers pending entries until the future is dropped.
    pub async fn run(&self) {
        loop {
            match self.pass(None, LEASE).await {
                Pass::Idle | Pass::Unavailable => self.environment.sleep(IDLE_WAIT).await,
                Pass::Delivered | Pass::Failed => {}
            }
        }
    }

    /// One claim over `repository` where given, otherwise over the repository whose oldest entry
    /// was written first, held for `lease`, and the delivery of what it claimed.
    pub async fn pass(&self, repository: Option<RepositoryId>, lease: Duration) -> Pass {
        let now = self.environment.now();
        let entries = match self.outbox.claim(repository, now, seconds(lease)).await {
            Ok(entries) => entries,
            Err(error) => {
                self.environment.log(&format!("{error}"));
                return Pass::Unavailable;
            }
        };
        let Some(first) = entries.first() else {
            return Pass::Idle;
        };
        let repository = first.repository;
        for entry in &entries {
            if !self.changes.repository_changed(entry).await {
                self.environment.log(&format!(
                    "repository {} ({}/{}): sequence {} was not delivered",
                    entry.repository, entry.organisation, entry.name, entry.sequence
                ));
                self.hold(repository).await;
                return Pass::Failed;
            }
            if let Err(error) = self.outbox.delete(repository, entry.sequence).await {
                self.environment.log(&format!(
                    "repository {} ({}/{}): sequence {} was delivered but not deleted: {error}",
                    entry.repository, entry.organisation, entry.name, entry.sequence
                ));
                self.hold(repository).await;
                return Pass::Failed;
            }
        }
        self.failures
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&repository);
        Pass::Delivered
    }

    /// Holds `repository` back for the backoff its failures so far come to.
    async fn hold(&self, repository: RepositoryId) {
        let failures = {
            let mut failures = self.failures.lock().unwrap_or_else(PoisonError::into_inner);
            let held = failures.entry(repository).or_insert(0);
            *held = held.saturating_add(1);
            *held
        };
        let until = self
            .environment
            .now()
            .saturating_add(seconds(backoff(failures)));
        if let Err(error) = self.outbox.release(repository, until).await {
            self.environment.log(&format!(
                "repository {repository}: its claim was not released and lapses on its own: {error}"
            ));
        }
    }
}

impl<O, C, E> Delivery for DirectoryRelay<O, C, E>
where
    O: DirectoryOutbox + Send + Sync,
    C: DirectoryChanges + Send + Sync,
    E: RelayEnvironment + Send + Sync,
{
    /// One pass over `repository`'s entries, left where `creation_wait` ends. The claim lasts as
    /// long as the wait, so a pass left unfinished does not hold the relay off for long.
    async fn deliver(&self, repository: RepositoryId) {
        let lease = self.creation_wait.max(Duration::from_secs(1));
        let work = self.pass(Some(repository), lease);
        let limit = self.environment.sleep(self.creation_wait);
        within(work, limit).await;
    }
}

/// The result of `work` where it completes before `limit` does, and `None` otherwise.
async fn within<T>(work: impl Future<Output = T>, limit: impl Future<Output = ()>) -> Option<T> {
    let mut work = pin!(work);
    let mut limit = pin!(limit);
    poll_fn(|context| {
        if let Poll::Ready(done) = work.as_mut().poll(context) {
            return Poll::Ready(Some(done));
        }
        if limit.as_mut().poll(context).is_ready() {
            return Poll::Ready(None);
        }
        Poll::Pending
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doubles_from_the_first_backoff_to_the_cap() {
        let held: Vec<u64> = (1..=9)
            .map(|failures| backoff(failures).as_secs())
            .collect();

        assert_eq!(held, [1, 2, 4, 8, 16, 32, 60, 60, 60]);
        assert_eq!(backoff(u32::MAX), MAX_BACKOFF);
    }
}
