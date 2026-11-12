//! The object store: each repository's immutable blobs, held by content address and moved by
//! grants the client uses directly (ADR 0015).

use std::fmt;

use crate::domain::address::Address;
use crate::domain::repository::RepositoryId;

/// The method a grant's holder sends.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GrantMethod {
    /// Reads the object.
    Get,
    /// Writes the object.
    Put,
}

/// Permission, until it expires, to move one object's bytes directly to or from the store.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Grant {
    /// Where to send the request.
    pub url: String,
    /// The method to send it with.
    pub method: GrantMethod,
    /// The headers to send with it, by name and value.
    pub headers: Vec<(String, String)>,
    /// When the grant stops working, in seconds since 1970-01-01T00:00:00Z.
    pub expires_at: i64,
}

/// Holds each repository's blobs, each under the address of its bytes.
pub trait ObjectStore {
    /// Stores `bytes` and returns their address.
    ///
    /// # Errors
    ///
    /// The store's [`ObjectStoreError`].
    fn put(
        &self,
        repository: RepositoryId,
        bytes: Vec<u8>,
    ) -> impl Future<Output = Result<Address, ObjectStoreError>> + Send;

    /// The bytes stored under `address`.
    ///
    /// # Errors
    ///
    /// [`ObjectStoreError::NotFound`] where nothing is stored under `address`.
    fn get(
        &self,
        repository: RepositoryId,
        address: Address,
    ) -> impl Future<Output = Result<Vec<u8>, ObjectStoreError>> + Send;

    /// The size in bytes of the object stored under `address`, answered without its bytes.
    ///
    /// # Errors
    ///
    /// [`ObjectStoreError::NotFound`] where nothing is stored under `address`.
    fn size(
        &self,
        repository: RepositoryId,
        address: Address,
    ) -> impl Future<Output = Result<u64, ObjectStoreError>> + Send;

    /// Whether an object is stored under `address`.
    ///
    /// # Errors
    ///
    /// The store's [`ObjectStoreError`].
    fn exists(
        &self,
        repository: RepositoryId,
        address: Address,
    ) -> impl Future<Output = Result<bool, ObjectStoreError>> + Send;

    /// A grant to upload the `size` bytes stored under `address`. The store takes bytes only of
    /// that size.
    ///
    /// # Errors
    ///
    /// The store's [`ObjectStoreError`].
    fn upload_grant(
        &self,
        repository: RepositoryId,
        address: Address,
        size: u64,
    ) -> impl Future<Output = Result<Grant, ObjectStoreError>> + Send;

    /// A grant to download the bytes stored under `address`. It does not check that they exist.
    ///
    /// # Errors
    ///
    /// The store's [`ObjectStoreError`].
    fn download_grant(
        &self,
        repository: RepositoryId,
        address: Address,
    ) -> impl Future<Output = Result<Grant, ObjectStoreError>> + Send;
}

/// Why the object store did not answer.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ObjectStoreError {
    /// Nothing is stored under the address.
    NotFound(Address),
    /// The store's backend failed; carries a description.
    Backend(String),
}

impl fmt::Display for ObjectStoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(address) => write!(f, "no object {address}"),
            Self::Backend(description) => write!(f, "object store failed: {description}"),
        }
    }
}

impl std::error::Error for ObjectStoreError {}
