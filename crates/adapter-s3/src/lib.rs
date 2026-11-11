//! The S3-compatible implementation of the object store port (ADR 0023).

mod object_store;

pub use object_store::{DEFAULT_EXPIRY, S3ObjectStore, SINGLE_PUT_LIMIT};
