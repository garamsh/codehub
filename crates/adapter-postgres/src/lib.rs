//! PostgreSQL implementations of the driven ports (ADR 0013, ADR 0021, ADR 0022).
//!
//! A server opens one [`Database`], applies its migrations with [`Database::migrate`], and builds
//! each adapter over it.

mod authorisation_decision;
mod changeset_log;
mod conversion;
mod database;
mod derived_data;
mod directory_outbox;
mod draft_store;
mod job_queue;
mod membership;
mod repository_directory;
mod runner_registry;

pub use authorisation_decision::PostgresAuthorisationDecision;
pub use changeset_log::PostgresChangesetLog;
pub use database::{Database, DatabaseError};
pub use derived_data::PostgresDerivedData;
pub use directory_outbox::PostgresDirectoryOutbox;
pub use draft_store::PostgresDraftStore;
pub use job_queue::PostgresJobQueue;
pub use membership::PostgresMembership;
pub use repository_directory::PostgresRepositoryDirectory;
pub use runner_registry::PostgresRunnerRegistry;
