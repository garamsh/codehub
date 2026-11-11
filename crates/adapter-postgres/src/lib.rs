//! PostgreSQL implementations of the driven ports (ADR 0013, ADR 0021, ADR 0022).
//!
//! A server opens one [`Database`], applies its migrations with [`Database::migrate`], and builds
//! each adapter over it.

mod authorisation_decision;
mod changeset_log;
mod conversion;
mod database;
mod derived_data;
mod draft_store;
mod membership;
mod repository_directory;

pub use authorisation_decision::PostgresAuthorisationDecision;
pub use changeset_log::PostgresChangesetLog;
pub use database::{Database, DatabaseError};
pub use derived_data::PostgresDerivedData;
pub use draft_store::PostgresDraftStore;
pub use membership::PostgresMembership;
pub use repository_directory::PostgresRepositoryDirectory;
