use std::collections::BTreeMap;
use std::sync::Mutex;

use app::domain::address::Address;
use app::port::driven::object_store::{ObjectStore, ObjectStoreError};

use crate::lock;

/// An object store in memory.
#[derive(Default)]
pub struct MemoryObjectStore {
    objects: Mutex<BTreeMap<Address, Vec<u8>>>,
}

impl ObjectStore for MemoryObjectStore {
    async fn put(&self, bytes: Vec<u8>) -> Result<Address, ObjectStoreError> {
        let address = Address::of(&bytes);
        lock(&self.objects).insert(address, bytes);
        Ok(address)
    }

    async fn get(&self, address: Address) -> Result<Vec<u8>, ObjectStoreError> {
        lock(&self.objects)
            .get(&address)
            .cloned()
            .ok_or(ObjectStoreError::NotFound(address))
    }
}
