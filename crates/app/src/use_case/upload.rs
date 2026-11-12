//! Upload grants: permission to send an object's bytes straight to the store, up to a maximum
//! size.

use crate::domain::address::Address;
use crate::domain::repository::RepositoryId;
use crate::port::driven::object_store::{Grant, ObjectStore};
use crate::port::driving::source_control::UploadError;

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
