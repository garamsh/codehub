//! The in-memory object store against the object-store contract scenarios.

use adapter_memory::MemoryObjectStore;
use app::domain::repository::RepositoryId;
use test_support::contract::object_store;

#[tokio::test]
async fn answers_the_size_of_a_stored_blob_and_not_found_for_a_missing_one() {
    object_store::answers_the_size_of_a_stored_blob_and_not_found_for_a_missing_one(
        &MemoryObjectStore::default(),
        RepositoryId::new(1),
        RepositoryId::new(2),
    )
    .await;
}
