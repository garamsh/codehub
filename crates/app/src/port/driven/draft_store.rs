//! The draft store: pending changesets, held outside the log until they land (ADR 0012).

use std::fmt;

use crate::domain::address::Address;
use crate::domain::changeset::Changeset;
use crate::domain::encoding::EncodeError;

/// Holds drafts, each under its address.
pub trait DraftStore {
    /// Stores `draft` and returns its address.
    ///
    /// # Errors
    ///
    /// [`DraftStoreError::Encode`] where the draft has no encoding.
    fn put(
        &self,
        draft: Changeset,
    ) -> impl Future<Output = Result<Address, DraftStoreError>> + Send;

    /// The draft stored under `address`.
    ///
    /// # Errors
    ///
    /// [`DraftStoreError::NotFound`] where no draft is stored under `address`.
    fn get(
        &self,
        address: Address,
    ) -> impl Future<Output = Result<Changeset, DraftStoreError>> + Send;
}

/// Why the draft store refused or did not answer.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum DraftStoreError {
    /// No draft is stored under the address.
    NotFound(Address),
    /// A draft has no encoding, so no address.
    Encode(EncodeError),
}

impl fmt::Display for DraftStoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(address) => write!(f, "no draft {address}"),
            Self::Encode(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for DraftStoreError {}
