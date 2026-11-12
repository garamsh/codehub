//! A claim learns each file's size and address without reading any blob's bytes.

#[cfg(test)]
pub mod common;

use std::sync::atomic::{AtomicU32, Ordering};

use adapter_memory::MemoryObjectStore;
use app::domain::address::Address;
use app::domain::repository::RepositoryId;
use app::port::driven::check_definitions::CheckDefinitions;
use app::port::driven::object_store::{Grant, ObjectStore, ObjectStoreError};
use app::port::driving::jobs::Jobs;
use app::use_case::jobs::{JobsAdapters, JobsService};
use app::use_case::landing_attempts::LandingAttempts;
use common::Harness;
use test_support::contract::Deployment;
use test_support::contract::runner_deployment::{
    RunnerDeployment, check, definitions, land, organisation, runner, text,
};

/// An object store that answers as the memory store does and counts the reads of bytes.
struct CountingObjects<'a> {
    inner: &'a MemoryObjectStore,
    gets: AtomicU32,
}

impl ObjectStore for CountingObjects<'_> {
    async fn put(
        &self,
        repository: RepositoryId,
        bytes: Vec<u8>,
    ) -> Result<Address, ObjectStoreError> {
        self.inner.put(repository, bytes).await
    }

    async fn get(
        &self,
        repository: RepositoryId,
        address: Address,
    ) -> Result<Vec<u8>, ObjectStoreError> {
        self.gets.fetch_add(1, Ordering::SeqCst);
        self.inner.get(repository, address).await
    }

    async fn size(
        &self,
        repository: RepositoryId,
        address: Address,
    ) -> Result<u64, ObjectStoreError> {
        self.inner.size(repository, address).await
    }

    async fn exists(
        &self,
        repository: RepositoryId,
        address: Address,
    ) -> Result<bool, ObjectStoreError> {
        self.inner.exists(repository, address).await
    }

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

#[tokio::test]
async fn a_claim_reads_no_blob_bytes_and_carries_each_files_size_and_address() {
    let harness = Harness::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let specs = [check("unit")];
    let body = text(&specs);
    harness.teach(&body, Ok(definitions(&specs)));
    let file = harness.definitions().file().to_string();
    land(
        &repo,
        &ada,
        "work",
        &[(&file, &body), ("src/a.txt", "hello")],
    )
    .await
    .unwrap();
    let bot = runner(&harness, &ada, &organisation("acme"), "bot", &[]).await;
    let objects = CountingObjects {
        inner: &harness.objects,
        gets: AtomicU32::new(0),
    };
    let jobs = JobsService::new(
        JobsAdapters {
            registry: &harness.registry,
            queue: &harness.queue,
            objects: &objects,
            derived: &harness.derived,
            log: &harness.log,
            directory: &harness.directory,
            definitions: &harness.definitions,
        },
        30,
        LandingAttempts::default(),
    );

    let claimed = jobs.claim(&bot, 0).await.unwrap().unwrap();

    assert_eq!(claimed.files.len(), 2);
    let a = claimed
        .files
        .iter()
        .find(|f| f.path.to_string() == "src/a.txt")
        .unwrap();
    assert_eq!(a.size, 5);
    assert_eq!(a.address, Address::of(b"hello"));
    assert_eq!(objects.gets.load(Ordering::SeqCst), 0);
}
