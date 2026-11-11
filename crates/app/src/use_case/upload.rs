//! Upload grants: permission to send an object's bytes straight to the store, up to a maximum
//! size.

use std::fmt;

use crate::domain::address::Address;
use crate::domain::repository::RepositoryId;
use crate::port::driven::object_store::{Grant, ObjectStore, ObjectStoreError};

/// A grant to upload the `size` bytes of the object `address` to `repository`.
///
/// # Errors
///
/// [`UploadError::TooLarge`] where `size` exceeds `max_size`, and otherwise the failure of the
/// object store.
pub async fn request_upload<O: ObjectStore>(
    objects: &O,
    repository: RepositoryId,
    address: Address,
    size: u64,
    max_size: u64,
) -> Result<Grant, UploadError> {
    if size > max_size {
        return Err(UploadError::TooLarge { size, max_size });
    }
    Ok(objects.upload_grant(repository, address, size).await?)
}

/// Why no upload grant was issued.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum UploadError {
    /// The object is larger than the maximum the server accepts.
    TooLarge {
        /// The object's size, in bytes.
        size: u64,
        /// The largest size accepted, in bytes.
        max_size: u64,
    },
    /// The object store failed.
    Objects(ObjectStoreError),
}

impl fmt::Display for UploadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge { size, max_size } => {
                write!(f, "{size} bytes exceed the {max_size} byte maximum")
            }
            Self::Objects(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for UploadError {}

impl From<ObjectStoreError> for UploadError {
    fn from(error: ObjectStoreError) -> Self {
        Self::Objects(error)
    }
}
