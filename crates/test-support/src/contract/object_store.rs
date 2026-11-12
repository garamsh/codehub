//! What an object store answers about a blob's size.

use app::domain::address::Address;
use app::domain::repository::RepositoryId;
use app::port::driven::object_store::{ObjectStore, ObjectStoreError};

/// A stored blob's size is its length, an empty blob's is zero, and a blob stored in `other` is
/// not found in `repository`, nor is one never stored. `repository` and `other` are repositories
/// the store holds nothing in.
pub async fn answers_the_size_of_a_stored_blob_and_not_found_for_a_missing_one<S: ObjectStore>(
    store: &S,
    repository: RepositoryId,
    other: RepositoryId,
) {
    let bytes = b"seventeen bytes!!";
    let address = store.put(other, bytes.to_vec()).await.unwrap();
    assert_eq!(store.put(repository, bytes.to_vec()).await, Ok(address));
    let empty = store.put(repository, Vec::new()).await.unwrap();
    let elsewhere = store.put(other, b"only elsewhere".to_vec()).await.unwrap();

    assert_eq!(store.size(repository, address).await, Ok(17));
    assert_eq!(store.size(repository, empty).await, Ok(0));

    assert_eq!(
        store.size(repository, elsewhere).await,
        Err(ObjectStoreError::NotFound(elsewhere))
    );
    let never = Address::of(b"never stored");
    assert_eq!(
        store.size(repository, never).await,
        Err(ObjectStoreError::NotFound(never))
    );
}
