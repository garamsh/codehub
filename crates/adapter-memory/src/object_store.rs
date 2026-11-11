use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use app::domain::address::Address;
use app::domain::repository::RepositoryId;
use app::port::driven::object_store::{Grant, GrantMethod, ObjectStore, ObjectStoreError};

use crate::lock;

/// How long a grant lasts, in seconds.
const GRANT_LIFETIME: i64 = 900;

/// An object store in memory. Its grants are `memory://` URLs no client can reach: a test that
/// stands in for the client puts the bytes with [`ObjectStore::put`].
#[derive(Default)]
pub struct MemoryObjectStore {
    objects: Mutex<BTreeMap<(RepositoryId, Address), Vec<u8>>>,
}

fn grant(
    repository: RepositoryId,
    address: Address,
    method: GrantMethod,
    headers: Vec<(String, String)>,
) -> Grant {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
        });
    Grant {
        url: format!("memory://{repository}/{address}"),
        method,
        headers,
        expires_at: now.saturating_add(GRANT_LIFETIME),
    }
}

impl ObjectStore for MemoryObjectStore {
    async fn put(
        &self,
        repository: RepositoryId,
        bytes: Vec<u8>,
    ) -> Result<Address, ObjectStoreError> {
        let address = Address::of(&bytes);
        lock(&self.objects).insert((repository, address), bytes);
        Ok(address)
    }

    async fn get(
        &self,
        repository: RepositoryId,
        address: Address,
    ) -> Result<Vec<u8>, ObjectStoreError> {
        lock(&self.objects)
            .get(&(repository, address))
            .cloned()
            .ok_or(ObjectStoreError::NotFound(address))
    }

    async fn exists(
        &self,
        repository: RepositoryId,
        address: Address,
    ) -> Result<bool, ObjectStoreError> {
        Ok(lock(&self.objects).contains_key(&(repository, address)))
    }

    async fn upload_grant(
        &self,
        repository: RepositoryId,
        address: Address,
        size: u64,
    ) -> Result<Grant, ObjectStoreError> {
        Ok(grant(
            repository,
            address,
            GrantMethod::Put,
            vec![("content-length".to_owned(), size.to_string())],
        ))
    }

    async fn download_grant(
        &self,
        repository: RepositoryId,
        address: Address,
    ) -> Result<Grant, ObjectStoreError> {
        Ok(grant(repository, address, GrantMethod::Get, Vec::new()))
    }
}
