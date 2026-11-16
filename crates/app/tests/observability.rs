//! What `Observed` counts as a failed call and what `BacklogGauges` leaves on the gauges (ADR
//! 0056), read from the text a recorder renders.

use std::sync::{Mutex, PoisonError};

use adapter_memory::{MemoryDirectoryChanges, MemoryIdentityVerification, MemoryObjectStore};
use app::domain::address::Address;
use app::domain::credential::Credential;
use app::domain::name::{OrganisationName, RepositoryName};
use app::domain::principal::Principal;
use app::domain::repository::RepositoryId;
use app::observability::{BacklogGauges, MAX_LABEL_SETS, Observed};
use app::port::driven::backlog::{Backlog, BacklogError, BacklogReading, QueueBacklog};
use app::port::driven::directory_changes::{
    ChangeKind, DirectoryChanges, OrganisationChange, RepositoryChange,
};
use app::port::driven::identity_verification::{IdentityError, IdentityVerification};
use app::port::driven::object_store::{Grant, ObjectStore, ObjectStoreError};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle, PrometheusRecorder};

const ERRORS: &str = "codehub_outbound_call_errors_total";

/// A recorder that is this thread's for as long as it lives, and the handle that renders it.
fn recording() -> (PrometheusRecorder, PrometheusHandle) {
    let recorder = PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    (recorder, handle)
}

#[cfg(test)]
fn credential() -> Credential {
    "token".parse().unwrap()
}

#[cfg(test)]
fn change() -> RepositoryChange {
    RepositoryChange {
        organisation: "acme".parse::<OrganisationName>().unwrap(),
        name: "app".parse::<RepositoryName>().unwrap(),
        repository: RepositoryId::new(1),
        kind: ChangeKind::Created,
        sequence: 1,
    }
}

struct FailingIdentity(IdentityError);

#[async_trait::async_trait]
impl IdentityVerification for FailingIdentity {
    async fn verify(&self, _: &Credential) -> Result<Principal, IdentityError> {
        Err(self.0.clone())
    }
}

#[tokio::test]
async fn an_identity_backend_failure_counts_and_a_rejection_does_not() {
    let (recorder, handle) = recording();
    let _installed = metrics::set_default_local_recorder(&recorder);

    let rejecting = Observed::new("identity", MemoryIdentityVerification::default());
    assert_eq!(
        rejecting.verify(&credential()).await,
        Err(IdentityError::Rejected)
    );
    let text = handle.render();
    assert!(
        text.contains("codehub_outbound_call_duration_seconds"),
        "{text}"
    );
    assert!(!text.contains(ERRORS), "{text}");

    let failing = Observed::new(
        "identity",
        FailingIdentity(IdentityError::Backend("down".into())),
    );
    assert!(failing.verify(&credential()).await.is_err());
    let text = handle.render();
    assert!(
        text.contains(&format!("{ERRORS}{{dependency=\"identity\"}} 1")),
        "{text}"
    );
}

struct FailingStore(ObjectStoreError);

#[async_trait::async_trait]
impl ObjectStore for FailingStore {
    async fn put(&self, _: RepositoryId, _: Vec<u8>) -> Result<Address, ObjectStoreError> {
        Err(self.0.clone())
    }
    async fn get(&self, _: RepositoryId, _: Address) -> Result<Vec<u8>, ObjectStoreError> {
        Err(self.0.clone())
    }
    async fn size(&self, _: RepositoryId, _: Address) -> Result<u64, ObjectStoreError> {
        Err(self.0.clone())
    }
    async fn exists(&self, _: RepositoryId, _: Address) -> Result<bool, ObjectStoreError> {
        Err(self.0.clone())
    }
    async fn upload_grant(
        &self,
        _: RepositoryId,
        _: Address,
        _: u64,
    ) -> Result<Grant, ObjectStoreError> {
        Err(self.0.clone())
    }
    async fn download_grant(&self, _: RepositoryId, _: Address) -> Result<Grant, ObjectStoreError> {
        Err(self.0.clone())
    }
}

#[tokio::test]
async fn an_object_store_backend_failure_counts_and_a_missing_object_does_not() {
    let (recorder, handle) = recording();
    let _installed = metrics::set_default_local_recorder(&recorder);
    let repository = RepositoryId::new(1);
    let address = Address::of(b"absent");

    let memory = Observed::new("object_store", MemoryObjectStore::default());
    assert!(matches!(
        memory.get(repository, address).await,
        Err(ObjectStoreError::NotFound(_))
    ));
    assert!(!handle.render().contains(ERRORS));

    let failing = Observed::new(
        "object_store",
        FailingStore(ObjectStoreError::Backend("down".into())),
    );
    assert!(failing.get(repository, address).await.is_err());
    let text = handle.render();
    assert!(
        text.contains(&format!("{ERRORS}{{dependency=\"object_store\"}} 1")),
        "{text}"
    );

    // A grant is signed here and sent nowhere: it is not a call to the store.
    assert!(failing.upload_grant(repository, address, 1).await.is_err());
    let text = handle.render();
    assert!(
        text.contains(&format!("{ERRORS}{{dependency=\"object_store\"}} 1")),
        "{text}"
    );
}

#[tokio::test]
async fn a_not_found_object_is_not_a_backend_failure_even_for_the_failing_store() {
    let (recorder, handle) = recording();
    let _installed = metrics::set_default_local_recorder(&recorder);
    let store = Observed::new(
        "object_store",
        FailingStore(ObjectStoreError::NotFound(Address::of(b"x"))),
    );
    assert!(
        store
            .size(RepositoryId::new(1), Address::of(b"x"))
            .await
            .is_err()
    );
    assert!(!handle.render().contains(ERRORS));
}

struct Refusing;

#[async_trait::async_trait]
impl DirectoryChanges for Refusing {
    async fn repository_changed(&self, _: &RepositoryChange) -> bool {
        false
    }
    async fn organisation_changed(&self, _: &OrganisationChange) {}
}

#[tokio::test]
async fn a_repository_change_the_directory_did_not_accept_counts_and_an_accepted_one_does_not() {
    let (recorder, handle) = recording();
    let _installed = metrics::set_default_local_recorder(&recorder);

    let accepting = Observed::new("directory", MemoryDirectoryChanges::default());
    assert!(accepting.repository_changed(&change()).await);
    assert!(!handle.render().contains(ERRORS));

    let refusing = Observed::new("directory", Refusing);
    assert!(!refusing.repository_changed(&change()).await);
    let text = handle.render();
    assert!(
        text.contains(&format!("{ERRORS}{{dependency=\"directory\"}} 1")),
        "{text}"
    );

    // An organisation change reports nothing back, so it never counts as failed.
    refusing
        .organisation_changed(&OrganisationChange::Created(
            "acme".parse::<OrganisationName>().unwrap(),
        ))
        .await;
    let text = handle.render();
    assert!(
        text.contains(&format!("{ERRORS}{{dependency=\"directory\"}} 1")),
        "{text}"
    );
}

struct Reading(Mutex<Result<BacklogReading, BacklogError>>);

#[cfg(test)]
impl Reading {
    fn of(queues: Vec<QueueBacklog>, outbox_pending: u64) -> Self {
        Self(Mutex::new(Ok(BacklogReading {
            queues,
            outbox_pending,
        })))
    }

    fn set(&self, queues: Vec<QueueBacklog>) {
        *self.0.lock().unwrap() = Ok(BacklogReading {
            queues,
            outbox_pending: 0,
        });
    }
}

#[cfg(test)]
#[async_trait::async_trait]
impl Backlog for Reading {
    async fn read(&self) -> Result<BacklogReading, BacklogError> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

fn queue(kind: &'static str, labels: &str, depth: u64, age: f64) -> QueueBacklog {
    QueueBacklog {
        kind,
        labels: labels.to_owned(),
        depth,
        oldest_age_seconds: age,
    }
}

fn series(text: &str, name: &str) -> Vec<String> {
    text.lines()
        .filter(|line| line.starts_with(name) && !line.starts_with('#'))
        .map(ToOwned::to_owned)
        .collect()
}

#[tokio::test]
async fn a_queue_that_empties_is_set_to_zero_once_and_then_left_alone() {
    let (recorder, handle) = recording();
    let _installed = metrics::set_default_local_recorder(&recorder);
    let backlog = Reading::of(vec![queue("job", "linux", 3, 12.0)], 5);
    let gauges = BacklogGauges::new();

    gauges.update(&backlog).await.unwrap();
    let text = handle.render();
    assert!(
        text.contains("codehub_queue_depth{kind=\"job\",labels=\"linux\"} 3"),
        "{text}"
    );
    assert!(text.contains("codehub_outbox_pending 5"), "{text}");

    backlog.set(Vec::new());
    gauges.update(&backlog).await.unwrap();
    let text = handle.render();
    assert!(
        text.contains("codehub_queue_depth{kind=\"job\",labels=\"linux\"} 0"),
        "{text}"
    );
    assert!(
        text.contains("codehub_queue_oldest_age_seconds{kind=\"job\",labels=\"linux\"} 0"),
        "{text}"
    );

    // Set to zero once: a value set from outside in between is not set back.
    metrics::gauge!("codehub_queue_depth", "kind" => "job", "labels" => "linux").set(9.0);
    gauges.update(&backlog).await.unwrap();
    let text = handle.render();
    assert!(
        text.contains("codehub_queue_depth{kind=\"job\",labels=\"linux\"} 9"),
        "{text}"
    );
}

#[tokio::test]
async fn a_failed_read_leaves_the_gauges_as_they_were() {
    let (recorder, handle) = recording();
    let _installed = metrics::set_default_local_recorder(&recorder);
    let backlog = Reading::of(vec![queue("run", "x", 2, 1.0)], 0);
    let gauges = BacklogGauges::new();
    gauges.update(&backlog).await.unwrap();

    *backlog.0.lock().unwrap() = Err(BacklogError("down".into()));
    assert!(gauges.update(&backlog).await.is_err());
    let text = handle.render();
    assert!(
        text.contains("codehub_queue_depth{kind=\"run\",labels=\"x\"} 2"),
        "{text}"
    );
}

#[tokio::test]
async fn label_sets_beyond_the_cap_are_folded_into_other() {
    let (recorder, handle) = recording();
    let _installed = metrics::set_default_local_recorder(&recorder);
    let extra: u64 = 3;
    let queues: Vec<QueueBacklog> = (0..MAX_LABEL_SETS as u64 + extra)
        .map(|index| queue("job", &format!("set-{index:02}"), 100 - index, index as f64))
        .collect();
    let backlog = Reading::of(queues, 0);
    BacklogGauges::new().update(&backlog).await.unwrap();

    let text = handle.render();
    let depths = series(&text, "codehub_queue_depth");
    assert_eq!(depths.len(), MAX_LABEL_SETS + 1, "{text}");
    // The deepest sets keep their own series; the shallowest `extra` are summed into `other`.
    assert!(
        depths.iter().any(|line| line.contains("labels=\"set-00\"")),
        "{text}"
    );
    assert!(
        !depths
            .iter()
            .any(|line| line.contains(&format!("set-{:02}", MAX_LABEL_SETS))),
        "{text}"
    );
    let folded: u64 = (MAX_LABEL_SETS as u64..MAX_LABEL_SETS as u64 + extra)
        .map(|index| 100 - index)
        .sum();
    assert!(
        text.contains(&format!(
            "codehub_queue_depth{{kind=\"job\",labels=\"other\"}} {folded}"
        )),
        "{text}"
    );
    // The folded series carries the oldest age of what it folded.
    let oldest = MAX_LABEL_SETS as u64 + extra - 1;
    assert!(
        text.contains(&format!(
            "codehub_queue_oldest_age_seconds{{kind=\"job\",labels=\"other\"}} {oldest}"
        )),
        "{text}"
    );
}

#[tokio::test]
async fn the_cap_is_per_kind() {
    let (recorder, handle) = recording();
    let _installed = metrics::set_default_local_recorder(&recorder);
    let mut queues = Vec::new();
    for kind in ["job", "run"] {
        for index in 0..MAX_LABEL_SETS as u64 {
            queues.push(queue(kind, &format!("l{index:02}"), 1, 0.0));
        }
    }
    let backlog = Reading::of(queues, 0);
    BacklogGauges::new().update(&backlog).await.unwrap();

    let text = handle.render();
    let depths = series(&text, "codehub_queue_depth");
    assert_eq!(depths.len(), 2 * MAX_LABEL_SETS, "{text}");
    assert!(!text.contains("labels=\"other\""), "{text}");
}
