//! What the application reports about itself: the metrics and the spans of ADR 0056.
//!
//! The application records through the `metrics` and `tracing` facades and does not know where they
//! go; the composition site installs the recorder and the subscriber. Until it does, every call
//! here costs nothing and reports nowhere.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use tracing::Instrument;

use crate::domain::address::Address;
use crate::domain::credential::Credential;
use crate::domain::principal::Principal;
use crate::domain::repository::RepositoryId;
use crate::port::driven::authorisation_decision::{
    Action, AuthorisationDecision, AuthorisationError, Decision, Resource,
};
use crate::port::driven::backlog::{Backlog, BacklogError, QueueBacklog};
use crate::port::driven::directory_changes::{
    DirectoryChanges, OrganisationChange, RepositoryChange,
};
use crate::port::driven::identity_verification::{IdentityError, IdentityVerification};
use crate::port::driven::object_store::{Grant, ObjectStore, ObjectStoreError};

/// Seconds a landing waited at its repository's gate.
pub const LANDING_GATE_WAIT: &str = "codehub_landing_gate_wait_seconds";
/// Seconds a landing held its repository's gate.
pub const LANDING_GATE_HOLD: &str = "codehub_landing_gate_hold_seconds";
/// Attempts a landing made at the swap, lost or not.
pub const LANDING_ATTEMPTS: &str = "codehub_landing_attempts";
/// Compare-and-swaps lost to a `main` that moved first.
pub const LANDING_SWAPS_LOST: &str = "codehub_landing_swaps_lost_total";
/// Seconds an outbound call took, by dependency.
pub const OUTBOUND_DURATION: &str = "codehub_outbound_call_duration_seconds";
/// Outbound calls that failed, by dependency.
pub const OUTBOUND_ERRORS: &str = "codehub_outbound_call_errors_total";
/// Queued jobs and runs, by kind and labels.
pub const QUEUE_DEPTH: &str = "codehub_queue_depth";
/// Seconds the oldest queued job or run has waited, by kind and labels.
pub const QUEUE_OLDEST_AGE: &str = "codehub_queue_oldest_age_seconds";
/// Directory outbox entries not yet delivered.
pub const OUTBOX_PENDING: &str = "codehub_outbox_pending";
/// Connections the database pool has open.
pub const DB_POOL_CONNECTIONS: &str = "codehub_db_pool_connections";
/// Connections the database pool has lent out.
pub const DB_POOL_IN_USE: &str = "codehub_db_pool_connections_in_use";
/// The most connections the database pool opens.
pub const DB_POOL_MAX: &str = "codehub_db_pool_connections_max";
/// Seconds it took to take a connection from the database pool, or to give up on taking one.
pub const DB_POOL_ACQUIRE: &str = "codehub_db_pool_acquire_seconds";
/// Probes that failed to take a connection from the database pool.
pub const DB_POOL_ACQUIRE_FAILURES: &str = "codehub_db_pool_acquire_failures_total";

/// Records that a landing waited `waited` at the gate of its repository.
pub fn landing_gate_waited(waited: Duration) {
    metrics::histogram!(LANDING_GATE_WAIT).record(waited.as_secs_f64());
}

/// Records that a landing held the gate for `held`.
pub fn landing_gate_held(held: Duration) {
    metrics::histogram!(LANDING_GATE_HOLD).record(held.as_secs_f64());
}

/// Records that a landing finished after `attempts` attempts, `lost` of them at the swap.
pub fn landing_attempted(attempts: u32, lost: u32) {
    metrics::histogram!(LANDING_ATTEMPTS).record(f64::from(attempts));
    if lost > 0 {
        metrics::counter!(LANDING_SWAPS_LOST).increment(u64::from(lost));
    }
}

/// Times `call` to `dependency`, in a span named `<dependency>.<operation>`, and counts it as
/// failed where `failed` says its output is a failure.
pub async fn outbound<T>(
    dependency: &'static str,
    operation: &'static str,
    failed: impl FnOnce(&T) -> bool,
    call: impl Future<Output = T>,
) -> T {
    let span = tracing::info_span!(
        "outbound",
        otel.name = format!("{dependency}.{operation}"),
        dependency,
        operation
    );
    let started = Instant::now();
    let output = call.instrument(span).await;
    metrics::histogram!(OUTBOUND_DURATION, "dependency" => dependency)
        .record(started.elapsed().as_secs_f64());
    if failed(&output) {
        metrics::counter!(OUTBOUND_ERRORS, "dependency" => dependency).increment(1);
    }
    output
}

/// A port reported on: every call it makes to `dependency` is timed, counted and spanned. The
/// dependency's name is one of `decision_point`, `identity`, `object_store` and `directory`.
#[derive(Debug)]
pub struct Observed<T> {
    inner: T,
    dependency: &'static str,
}

impl<T> Observed<T> {
    /// `inner`, reported on as `dependency`.
    #[must_use]
    pub const fn new(dependency: &'static str, inner: T) -> Self {
        Self { inner, dependency }
    }
}

#[async_trait::async_trait]
impl<T: AuthorisationDecision> AuthorisationDecision for Observed<T> {
    async fn decide(
        &self,
        subject: &Principal,
        action: Action,
        resource: &Resource,
    ) -> Result<Decision, AuthorisationError> {
        outbound(
            self.dependency,
            "decide",
            Result::is_err,
            self.inner.decide(subject, action, resource),
        )
        .await
    }

    async fn decide_all(
        &self,
        subject: &Principal,
        action: Action,
        resources: &[Resource],
    ) -> Result<Vec<Decision>, AuthorisationError> {
        outbound(
            self.dependency,
            "decide_all",
            Result::is_err,
            self.inner.decide_all(subject, action, resources),
        )
        .await
    }
}

#[async_trait::async_trait]
impl<T: IdentityVerification> IdentityVerification for Observed<T> {
    async fn verify(&self, credential: &Credential) -> Result<Principal, IdentityError> {
        // A credential the verifier rejects is an answer, not a failure of the verifier.
        outbound(
            self.dependency,
            "verify",
            |result: &Result<Principal, IdentityError>| {
                matches!(result, Err(IdentityError::Backend(_)))
            },
            self.inner.verify(credential),
        )
        .await
    }
}

#[async_trait::async_trait]
impl<T: ObjectStore> ObjectStore for Observed<T> {
    async fn put(
        &self,
        repository: RepositoryId,
        bytes: Vec<u8>,
    ) -> Result<Address, ObjectStoreError> {
        outbound(
            self.dependency,
            "put",
            store_failed,
            self.inner.put(repository, bytes),
        )
        .await
    }

    async fn get(
        &self,
        repository: RepositoryId,
        address: Address,
    ) -> Result<Vec<u8>, ObjectStoreError> {
        outbound(
            self.dependency,
            "get",
            store_failed,
            self.inner.get(repository, address),
        )
        .await
    }

    async fn size(
        &self,
        repository: RepositoryId,
        address: Address,
    ) -> Result<u64, ObjectStoreError> {
        outbound(
            self.dependency,
            "size",
            store_failed,
            self.inner.size(repository, address),
        )
        .await
    }

    async fn exists(
        &self,
        repository: RepositoryId,
        address: Address,
    ) -> Result<bool, ObjectStoreError> {
        outbound(
            self.dependency,
            "exists",
            store_failed,
            self.inner.exists(repository, address),
        )
        .await
    }

    // A grant is signed here and sent nowhere: it is not an outbound call.
    async fn upload_grant(
        &self,
        repository: RepositoryId,
        address: Address,
        size: u64,
    ) -> Result<Grant, ObjectStoreError> {
        self.inner.upload_grant(repository, address, size).await
    }

    async fn download_grant(
        &self,
        repository: RepositoryId,
        address: Address,
    ) -> Result<Grant, ObjectStoreError> {
        self.inner.download_grant(repository, address).await
    }
}

/// Whether a store's answer is a failure of the store: a missing object is an answer.
fn store_failed<T>(result: &Result<T, ObjectStoreError>) -> bool {
    matches!(result, Err(ObjectStoreError::Backend(_)))
}

#[async_trait::async_trait]
impl<T: DirectoryChanges> DirectoryChanges for Observed<T> {
    async fn repository_changed(&self, change: &RepositoryChange) -> bool {
        outbound(
            self.dependency,
            "repository_changed",
            |accepted: &bool| !accepted,
            self.inner.repository_changed(change),
        )
        .await
    }

    async fn organisation_changed(&self, change: &OrganisationChange) {
        outbound(
            self.dependency,
            "organisation_changed",
            |(): &()| false,
            self.inner.organisation_changed(change),
        )
        .await;
    }
}

/// The most label sets of one kind that have a gauge of their own; the rest are folded into the
/// label set `other`. A label set is chosen by whoever queues the work, so the series must not
/// follow it without bound.
pub const MAX_LABEL_SETS: usize = 16;

/// The label set the label sets beyond [`MAX_LABEL_SETS`] are folded into.
pub const OTHER_LABELS: &str = "other";

/// Sets the backlog gauges from the backlog. A label set that no longer has anything waiting is set
/// to zero once and then left alone, so a scrape never shows a depth the queue has left behind and
/// the recorder, which drops a gauge nothing sets for a while, can forget it.
#[derive(Debug, Default)]
pub struct BacklogGauges {
    seen: Mutex<BTreeSet<(&'static str, String)>>,
}

/// `queues` with, per kind, the [`MAX_LABEL_SETS`] deepest label sets kept and the rest folded
/// into one entry of [`OTHER_LABELS`].
fn bounded(queues: &[QueueBacklog]) -> Vec<QueueBacklog> {
    let mut sorted = queues.to_vec();
    sorted.sort_by(|a, b| b.depth.cmp(&a.depth).then_with(|| a.labels.cmp(&b.labels)));
    let mut bounded: Vec<QueueBacklog> = Vec::new();
    let mut kept: BTreeMap<&'static str, usize> = BTreeMap::new();
    for queue in sorted {
        let count = kept.entry(queue.kind).or_insert(0);
        if *count < MAX_LABEL_SETS {
            *count += 1;
            bounded.push(queue);
            continue;
        }
        match bounded
            .iter_mut()
            .find(|other| other.kind == queue.kind && other.labels == OTHER_LABELS)
        {
            Some(other) => {
                other.depth = other.depth.saturating_add(queue.depth);
                other.oldest_age_seconds = other.oldest_age_seconds.max(queue.oldest_age_seconds);
            }
            None => bounded.push(QueueBacklog {
                labels: OTHER_LABELS.to_owned(),
                ..queue
            }),
        }
    }
    bounded
}

impl BacklogGauges {
    /// Gauges that have set nothing yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Reads `backlog` and sets the gauges to it.
    ///
    /// # Errors
    ///
    /// [`BacklogError`] where the backlog could not be read; the gauges keep their last values.
    pub async fn update(&self, backlog: &dyn Backlog) -> Result<(), BacklogError> {
        let reading = backlog.read().await?;
        let queues = bounded(&reading.queues);
        let mut seen = self.seen.lock().unwrap_or_else(PoisonError::into_inner);
        let current: BTreeSet<(&'static str, String)> = queues
            .iter()
            .map(|queue| (queue.kind, queue.labels.clone()))
            .collect();
        for (kind, labels) in seen.difference(&current) {
            metrics::gauge!(QUEUE_DEPTH, "kind" => *kind, "labels" => labels.clone()).set(0.0);
            metrics::gauge!(QUEUE_OLDEST_AGE, "kind" => *kind, "labels" => labels.clone()).set(0.0);
        }
        for queue in &queues {
            #[expect(
                clippy::cast_precision_loss,
                reason = "a queue depth of 2^53 is out of reach, and a gauge is a float"
            )]
            let depth = queue.depth as f64;
            metrics::gauge!(QUEUE_DEPTH, "kind" => queue.kind, "labels" => queue.labels.clone())
                .set(depth);
            metrics::gauge!(QUEUE_OLDEST_AGE, "kind" => queue.kind, "labels" => queue.labels.clone())
                .set(queue.oldest_age_seconds);
        }
        #[expect(
            clippy::cast_precision_loss,
            reason = "an outbox of 2^53 entries is out of reach, and a gauge is a float"
        )]
        let pending = reading.outbox_pending as f64;
        metrics::gauge!(OUTBOX_PENDING).set(pending);
        *seen = current;
        Ok(())
    }
}
