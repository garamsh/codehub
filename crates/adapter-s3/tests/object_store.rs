//! The S3 object store against the MinIO the checks run (`dev/check-env.sh`). Every test uses
//! repository ids no other test uses, and deletes nothing.

use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use adapter_s3::S3ObjectStore;
use app::domain::address::Address;
use app::domain::repository::RepositoryId;
use app::port::driven::object_store::{Grant, GrantMethod, ObjectStore, ObjectStoreError};
use reqwest::StatusCode;

const MAX_UPLOAD_SIZE: u64 = 1024;

#[cfg(test)]
fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} is not set; run through `make test`"))
}

#[cfg(test)]
fn store(max_upload_size: u64) -> S3ObjectStore {
    S3ObjectStore::new(
        &env("CODEHUB_TEST_S3_ENDPOINT"),
        &env("CODEHUB_TEST_S3_REGION"),
        &env("CODEHUB_TEST_S3_BUCKET"),
        &env("CODEHUB_TEST_S3_ACCESS_KEY"),
        &env("CODEHUB_TEST_S3_SECRET_KEY"),
        max_upload_size,
    )
    .unwrap()
}

/// A repository id drawn from a per-process random key, so parallel tests and other crates'
/// tests sharing the bucket do not meet.
#[cfg(test)]
fn fresh_repository() -> RepositoryId {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let clock = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    RepositoryId::new(RandomState::new().hash_one((COUNTER.fetch_add(1, Ordering::Relaxed), clock)))
}

/// Sends `bytes` the way a client holding `grant` does.
#[cfg(test)]
async fn upload(grant: &Grant, bytes: &[u8]) -> StatusCode {
    upload_without(grant, bytes, "").await
}

/// Sends `bytes` with every header of `grant` but `omitted`, which the HTTP client then sets
/// from the body.
#[cfg(test)]
async fn upload_without(grant: &Grant, bytes: &[u8], omitted: &str) -> StatusCode {
    assert_eq!(grant.method, GrantMethod::Put);
    let mut request = reqwest::Client::new().put(&grant.url).body(bytes.to_vec());
    for (name, value) in grant.headers.iter().filter(|(name, _)| name != omitted) {
        request = request.header(name, value);
    }
    request.send().await.unwrap().status()
}

#[cfg(test)]
async fn download(grant: &Grant) -> (StatusCode, Vec<u8>) {
    assert_eq!(grant.method, GrantMethod::Get);
    let response = reqwest::Client::new().get(&grant.url).send().await.unwrap();
    let status = response.status();
    (status, response.bytes().await.unwrap().to_vec())
}

#[tokio::test]
async fn stores_bytes_uploaded_through_a_grant() {
    let store = store(MAX_UPLOAD_SIZE);
    let repository = fresh_repository();
    let bytes = b"uploaded through a grant";
    let address = Address::of(bytes);
    assert!(!store.exists(repository, address).await.unwrap());

    let grant = store
        .upload_grant(repository, address, bytes.len() as u64)
        .await
        .unwrap();

    assert!(
        grant
            .headers
            .iter()
            .any(|(name, _)| name == "x-amz-checksum-sha256")
    );
    assert!(upload(&grant, bytes).await.is_success());
    assert!(store.exists(repository, address).await.unwrap());
    assert_eq!(store.get(repository, address).await.unwrap(), bytes);
}

#[tokio::test]
async fn rejects_an_upload_whose_bytes_do_not_match_the_address() {
    let store = store(MAX_UPLOAD_SIZE);
    let repository = fresh_repository();
    let bytes = b"the bytes the address names";
    let address = Address::of(bytes);
    let grant = store
        .upload_grant(repository, address, bytes.len() as u64)
        .await
        .unwrap();

    let wrong = bytes.to_ascii_uppercase();
    assert_ne!(wrong, bytes);
    assert_eq!(upload(&grant, &wrong).await, StatusCode::BAD_REQUEST);
    assert!(!store.exists(repository, address).await.unwrap());

    assert!(upload(&grant, bytes).await.is_success());
    assert!(store.exists(repository, address).await.unwrap());
}

#[tokio::test]
async fn rejects_an_upload_of_another_size_than_the_grant() {
    let store = store(MAX_UPLOAD_SIZE);
    let repository = fresh_repository();
    let bytes = b"a body of the size granted";
    let address = Address::of(bytes);
    let too_short = store
        .upload_grant(repository, address, bytes.len() as u64 - 1)
        .await
        .unwrap();

    let refused = upload_without(&too_short, bytes, "content-length").await;
    assert_eq!(refused, StatusCode::FORBIDDEN);
    assert!(!store.exists(repository, address).await.unwrap());

    let exact = store
        .upload_grant(repository, address, bytes.len() as u64)
        .await
        .unwrap();
    assert!(upload(&exact, bytes).await.is_success());
}

#[tokio::test]
async fn refuses_a_grant_above_the_maximum_size() {
    let store = store(MAX_UPLOAD_SIZE);
    let repository = fresh_repository();
    let address = Address::of(b"any");

    assert!(matches!(
        store
            .upload_grant(repository, address, MAX_UPLOAD_SIZE + 1)
            .await,
        Err(ObjectStoreError::Backend(_))
    ));
    assert!(
        store
            .upload_grant(repository, address, MAX_UPLOAD_SIZE)
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn keeps_one_address_apart_in_two_repositories() {
    let store = store(MAX_UPLOAD_SIZE);
    let (first, second) = (fresh_repository(), fresh_repository());
    let bytes = b"the same bytes in two repositories";

    let address = store.put(first, bytes.to_vec()).await.unwrap();

    assert!(store.exists(first, address).await.unwrap());
    assert!(!store.exists(second, address).await.unwrap());
    assert_eq!(
        store.get(second, address).await,
        Err(ObjectStoreError::NotFound(address))
    );

    assert_eq!(store.put(second, bytes.to_vec()).await.unwrap(), address);
    assert_eq!(store.get(first, address).await.unwrap(), bytes);
    assert_eq!(store.get(second, address).await.unwrap(), bytes);
}

#[tokio::test]
async fn serves_a_download_grant() {
    let store = store(MAX_UPLOAD_SIZE);
    let (repository, other) = (fresh_repository(), fresh_repository());
    let bytes = b"downloaded through a grant";
    let address = store.put(repository, bytes.to_vec()).await.unwrap();

    let grant = store.download_grant(repository, address).await.unwrap();
    assert_eq!(download(&grant).await, (StatusCode::OK, bytes.to_vec()));

    let elsewhere = store.download_grant(other, address).await.unwrap();
    assert_eq!(download(&elsewhere).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn reports_a_missing_object_as_not_found() {
    let store = store(MAX_UPLOAD_SIZE);
    let address = Address::of(b"never stored");

    assert_eq!(
        store.get(fresh_repository(), address).await,
        Err(ObjectStoreError::NotFound(address))
    );
}

#[tokio::test]
async fn stops_a_grant_working_when_its_expiry_passes() {
    let store = store(MAX_UPLOAD_SIZE).with_expiry(Duration::from_secs(1));
    let repository = fresh_repository();
    let address = store.put(repository, b"expiring".to_vec()).await.unwrap();
    let grant = store.download_grant(repository, address).await.unwrap();

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    assert!((grant.expires_at - now).abs() <= 2);

    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(download(&grant).await.0, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn gives_a_grant_the_default_expiry_of_fifteen_minutes() {
    let store = store(MAX_UPLOAD_SIZE);
    let repository = fresh_repository();
    let grant = store
        .download_grant(repository, Address::of(b"x"))
        .await
        .unwrap();

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    assert!((grant.expires_at - now - 900).abs() <= 2);
    assert!(grant.url.contains("X-Amz-Expires=900"));
}
