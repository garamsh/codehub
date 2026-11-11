//! The object store: immutable blobs put and got by content address.

use std::fmt;

use crate::domain::address::Address;

/// Holds blobs, each under the address of its bytes.
pub trait ObjectStore {
    /// Stores `bytes` and returns their address.
    ///
    /// # Errors
    ///
    /// The store's [`ObjectStoreError`].
    fn put(&self, bytes: Vec<u8>)
    -> impl Future<Output = Result<Address, ObjectStoreError>> + Send;

    /// The bytes stored under `address`.
    ///
    /// # Errors
    ///
    /// [`ObjectStoreError::NotFound`] where nothing is stored under `address`.
    fn get(
        &self,
        address: Address,
    ) -> impl Future<Output = Result<Vec<u8>, ObjectStoreError>> + Send;
}

/// Why the object store did not answer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ObjectStoreError {
    /// Nothing is stored under the address.
    NotFound(Address),
}

impl fmt::Display for ObjectStoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(address) => write!(f, "no object {address}"),
        }
    }
}

impl std::error::Error for ObjectStoreError {}
