//! What the repository directory answers from the database.

#[cfg(test)]
pub mod common;

use app::domain::name::{OrganisationName, RepositoryName};
use app::domain::repository::RepositoryId;
use app::port::driven::repository_directory::RepositoryDirectory;
use common::Harness;
use test_support::contract::Deployment;

#[tokio::test]
async fn answers_the_names_of_a_repository_from_its_identifier() {
    let harness = Harness::new().await;
    let ada = harness.caller("ada").await;
    let id = harness.repository(&ada, "acme").await.id;
    let acme: OrganisationName = "acme".parse().unwrap();
    let app: RepositoryName = "app".parse().unwrap();

    assert_eq!(harness.directory.names_of(id).await, Ok(Some((acme, app))));
    // An identifier nothing was assigned, and one beyond the stored range, name nothing.
    assert_eq!(
        harness
            .directory
            .names_of(RepositoryId::new(id.value() + 1))
            .await,
        Ok(None)
    );
    assert_eq!(
        harness
            .directory
            .names_of(RepositoryId::new(u64::MAX))
            .await,
        Ok(None)
    );
}
