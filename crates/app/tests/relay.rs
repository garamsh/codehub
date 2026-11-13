//! The relay delivers the directory outbox at least once and in order, holds a failing repository
//! back for a doubling backoff, and never drops an entry.

#[cfg(test)]
pub mod common;

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use adapter_memory::MemoryDirectoryOutbox;
use app::domain::repository::RepositoryId;
use app::port::driven::directory_changes::{
    DirectoryChanges, OrganisationChange, RepositoryChange,
};
use app::port::driven::directory_outbox::{DirectoryOutbox, DirectoryOutboxError};
use app::use_case::relay::{
    Delivery, DirectoryRelay, FIRST_BACKOFF, LEASE, MAX_BACKOFF, Pass, RelayEnvironment,
};
use common::Harness;
use test_support::contract::Deployment;

/// The creation wait the relays of these tests are given.
#[cfg(test)]
const WAIT: Duration = Duration::from_secs(2);

/// A clock the test sets, whose sleeps either end at once or never.
#[cfg(test)]
struct Manual {
    now: AtomicI64,
    sleeps_end: AtomicBool,
    log: Mutex<Vec<String>>,
}

#[cfg(test)]
impl Manual {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            now: AtomicI64::new(1_000),
            sleeps_end: AtomicBool::new(false),
            log: Mutex::new(Vec::new()),
        })
    }

    fn set(&self, now: i64) {
        self.now.store(now, Ordering::SeqCst);
    }

    fn lines(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
}

#[cfg(test)]
impl RelayEnvironment for Manual {
    fn now(&self) -> i64 {
        self.now.load(Ordering::SeqCst)
    }

    async fn sleep(&self, _duration: Duration) {
        if !self.sleeps_end.load(Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
    }

    fn log(&self, line: &str) {
        self.log.lock().unwrap().push(line.to_owned());
    }
}

/// What a receiver does with a repository change.
#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum Answer {
    Accept,
    Refuse,
    Hang,
}

/// A receiver that answers as the test says, per repository, and records every attempt.
#[cfg(test)]
struct Receiver {
    refused: Mutex<BTreeSet<RepositoryId>>,
    hangs: AtomicBool,
    down: AtomicBool,
    attempts: Mutex<Vec<(RepositoryId, u64)>>,
}

#[cfg(test)]
impl Receiver {
    fn new(answer: Answer) -> Arc<Self> {
        Arc::new(Self {
            refused: Mutex::new(BTreeSet::new()),
            hangs: AtomicBool::new(answer == Answer::Hang),
            down: AtomicBool::new(answer == Answer::Refuse),
            attempts: Mutex::new(Vec::new()),
        })
    }

    fn refuse_only(&self, repository: RepositoryId) {
        self.refused.lock().unwrap().insert(repository);
    }

    fn recover(&self) {
        self.down.store(false, Ordering::SeqCst);
        self.refused.lock().unwrap().clear();
    }

    fn attempts(&self) -> Vec<(RepositoryId, u64)> {
        self.attempts.lock().unwrap().clone()
    }
}

#[cfg(test)]
impl DirectoryChanges for Receiver {
    async fn repository_changed(&self, change: &RepositoryChange) -> bool {
        self.attempts
            .lock()
            .unwrap()
            .push((change.repository, change.sequence));
        if self.hangs.load(Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
        !self.down.load(Ordering::SeqCst)
            && !self.refused.lock().unwrap().contains(&change.repository)
    }

    async fn organisation_changed(&self, _change: &OrganisationChange) {}
}

/// An outbox that cannot be reached.
#[cfg(test)]
struct Unreachable;

#[cfg(test)]
impl DirectoryOutbox for Unreachable {
    async fn claim(
        &self,
        _repository: Option<RepositoryId>,
        _now: i64,
        _lease: i64,
    ) -> Result<Vec<RepositoryChange>, DirectoryOutboxError> {
        Err(DirectoryOutboxError::Backend("outbox down".to_owned()))
    }

    async fn release(
        &self,
        _repository: RepositoryId,
        _until: i64,
    ) -> Result<(), DirectoryOutboxError> {
        Ok(())
    }

    async fn delete(
        &self,
        _repository: RepositoryId,
        _sequence: u64,
    ) -> Result<(), DirectoryOutboxError> {
        Ok(())
    }
}

/// A relay over the harness's outbox, the receiver and the clock.
#[cfg(test)]
fn relay(
    harness: &Harness,
    receiver: &Arc<Receiver>,
    clock: &Arc<Manual>,
) -> DirectoryRelay<MemoryDirectoryOutbox, Receiver, Manual> {
    DirectoryRelay::new(
        Arc::new(harness.directory.outbox()),
        Arc::clone(receiver),
        Arc::clone(clock),
        WAIT,
    )
}

/// The sequences the outbox still holds for `repository`, claimed at `now` and released again.
#[cfg(test)]
async fn pending(harness: &Harness, repository: RepositoryId, now: i64) -> Vec<u64> {
    let outbox = &harness.outbox;
    let claimed = outbox.claim(Some(repository), now, 0).await.unwrap();
    outbox.release(repository, now).await.unwrap();
    claimed.iter().map(|change| change.sequence).collect()
}

#[tokio::test]
async fn retries_a_delivery_the_receiver_failed_and_deletes_the_entry_once_it_accepts() {
    let harness = Harness::default();
    let ada = harness.caller("ada").await;
    let id = harness.repository(&ada, "acme").await.id;
    let receiver = Receiver::new(Answer::Refuse);
    let clock = Manual::new();
    let relay = relay(&harness, &receiver, &clock);

    assert_eq!(relay.pass(None, LEASE).await, Pass::Failed);
    assert_eq!(pending(&harness, id, 1_000 + 1).await, vec![1]);

    receiver.recover();
    clock.set(1_000 + FIRST_BACKOFF.as_secs().cast_signed());
    assert_eq!(relay.pass(None, LEASE).await, Pass::Delivered);

    assert_eq!(pending(&harness, id, 1_100).await, Vec::<u64>::new());
    assert_eq!(receiver.attempts(), vec![(id, 1), (id, 1)]);
    assert_eq!(relay.pass(None, LEASE).await, Pass::Idle);
}

#[tokio::test]
async fn holds_a_failing_repository_back_for_a_doubling_backoff_and_never_drops_its_entry() {
    let harness = Harness::default();
    let ada = harness.caller("ada").await;
    let id = harness.repository(&ada, "acme").await.id;
    let receiver = Receiver::new(Answer::Refuse);
    let clock = Manual::new();
    let relay = relay(&harness, &receiver, &clock);
    let mut now = 1_000_i64;

    for held in [1, 2, 4, 8, 16, 32, 60, 60, 60] {
        clock.set(now);
        assert_eq!(relay.pass(None, LEASE).await, Pass::Failed);
        clock.set(now + held - 1);
        assert_eq!(relay.pass(None, LEASE).await, Pass::Idle, "held {held}s");
        now += held;
    }

    assert_eq!(MAX_BACKOFF.as_secs(), 60);
    assert_eq!(receiver.attempts().len(), 9);
    assert_eq!(pending(&harness, id, now).await, vec![1]);
}

#[tokio::test]
async fn delivers_in_sequence_order_and_a_failure_holds_back_the_later_entries() {
    let harness = Harness::default();
    let ada = harness.caller("ada").await;
    let id = harness.repository(&ada, "acme").await.id;
    harness
        .write_change(
            id,
            app::port::driven::directory_changes::ChangeKind::Deleted,
        )
        .await;
    let receiver = Receiver::new(Answer::Refuse);
    let clock = Manual::new();
    let relay = relay(&harness, &receiver, &clock);

    assert_eq!(relay.pass(None, LEASE).await, Pass::Failed);
    // The second entry is not tried while the first is not accepted.
    assert_eq!(receiver.attempts(), vec![(id, 1)]);

    receiver.recover();
    clock.set(1_001);
    assert_eq!(relay.pass(None, LEASE).await, Pass::Delivered);

    assert_eq!(receiver.attempts(), vec![(id, 1), (id, 1), (id, 2)]);
    assert_eq!(pending(&harness, id, 1_100).await, Vec::<u64>::new());
}

#[tokio::test]
async fn goes_on_with_another_repository_while_one_is_held_back() {
    let harness = Harness::default();
    let ada = harness.caller("ada").await;
    let stuck = harness.repository(&ada, "acme").await.id;
    let other = harness.repository(&ada, "other").await.id;
    let receiver = Receiver::new(Answer::Accept);
    receiver.refuse_only(stuck);
    let clock = Manual::new();
    let relay = relay(&harness, &receiver, &clock);

    assert_eq!(relay.pass(None, LEASE).await, Pass::Failed);
    assert_eq!(relay.pass(None, LEASE).await, Pass::Delivered);
    assert_eq!(relay.pass(None, LEASE).await, Pass::Idle);

    assert_eq!(pending(&harness, other, 1_000).await, Vec::<u64>::new());
    assert_eq!(pending(&harness, stuck, 1_000 + 1).await, vec![1]);
}

#[tokio::test]
async fn logs_every_failure_with_the_repository_and_the_sequence() {
    let harness = Harness::default();
    let ada = harness.caller("ada").await;
    let id = harness.repository(&ada, "acme").await.id;
    let receiver = Receiver::new(Answer::Refuse);
    let clock = Manual::new();
    let relay = relay(&harness, &receiver, &clock);

    relay.pass(None, LEASE).await;
    clock.set(1_001);
    relay.pass(None, LEASE).await;

    let expected = format!("repository {id} (acme/app): sequence 1 was not delivered");
    assert_eq!(clock.lines(), vec![expected.clone(), expected]);
}

#[tokio::test]
async fn logs_an_outbox_that_cannot_be_claimed_from_and_reports_it() {
    let receiver = Receiver::new(Answer::Accept);
    let clock = Manual::new();
    let relay = DirectoryRelay::new(
        Arc::new(Unreachable),
        Arc::clone(&receiver),
        Arc::clone(&clock),
        WAIT,
    );

    assert_eq!(relay.pass(None, LEASE).await, Pass::Unavailable);

    assert_eq!(
        clock.lines(),
        vec!["directory outbox failed: outbox down".to_owned()]
    );
    assert_eq!(receiver.attempts(), vec![]);
}

#[tokio::test]
async fn a_creation_delivers_within_the_wait_and_deletes_the_entry() {
    let harness = Harness::default();
    let ada = harness.caller("ada").await;
    let id = harness.repository(&ada, "acme").await.id;
    let receiver = Receiver::new(Answer::Accept);
    let clock = Manual::new();

    relay(&harness, &receiver, &clock).deliver(id).await;

    assert_eq!(receiver.attempts(), vec![(id, 1)]);
    assert_eq!(pending(&harness, id, 1_000).await, Vec::<u64>::new());
}

#[tokio::test]
async fn the_in_memory_composition_offers_each_creation_to_its_relay_and_keeps_what_is_refused() {
    let harness = Harness::default();
    let ada = harness.caller("ada").await;

    let id = harness.repository(&ada, "acme").await.id;

    assert_eq!(harness.delivery.receiver.attempts(), 1);
    assert_eq!(pending(&harness, id, 0).await, vec![1]);
}

#[tokio::test]
async fn a_creation_stops_waiting_at_the_wait_while_the_receiver_hangs_and_keeps_the_entry() {
    let harness = Harness::default();
    let ada = harness.caller("ada").await;
    let id = harness.repository(&ada, "acme").await.id;
    let receiver = Receiver::new(Answer::Hang);
    let clock = Manual::new();
    clock.sleeps_end.store(true, Ordering::SeqCst);

    tokio::time::timeout(
        Duration::from_secs(5),
        relay(&harness, &receiver, &clock).deliver(id),
    )
    .await
    .unwrap();

    assert_eq!(receiver.attempts(), vec![(id, 1)]);
    assert_eq!(
        pending(&harness, id, 1_000 + WAIT.as_secs().cast_signed()).await,
        vec![1]
    );
}
