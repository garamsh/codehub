//! A landing whose delta since its base cannot be read: a base the log lacks is not on `main`,
//! and any other changeset the log lacks on the chain is the log's failure.

pub mod common;

use std::collections::BTreeMap;

use app::domain::address::Address;
use app::domain::changeset::Changeset;
use app::domain::draft::{Draft, Instance};
use app::domain::repository::RepositoryId;
use app::port::driven::changeset_log::{ChangesetLog, ChangesetLogError, Ref};
use app::port::driving::source_control::{LandError, Landing};
use app::use_case::land::land;
use app::use_case::landing_attempts::LandingAttempts;
use common::Harness;
use test_support::contract::Deployment;

const REPOSITORY: RepositoryId = RepositoryId::new(1);

/// Lands an empty draft staged on `base` on the repository's `main`.
async fn land_on(harness: &Harness, base: Option<Address>) -> Result<Address, LandError> {
    let staged = Draft {
        instance: Instance::new(0),
        version: 0,
        base,
        description: String::new(),
        changes: BTreeMap::new(),
    };
    let landing = Landing {
        message: "land".to_owned(),
        timestamp: 0,
    };
    land(
        &*harness.objects,
        &*harness.log,
        &*harness.derived,
        REPOSITORY,
        "ada",
        staged,
        landing,
        LandingAttempts::default(),
    )
    .await
}

/// Appends a changeset with no parent and points `main` at it.
async fn seed_main(harness: &Harness) -> Result<Address, ChangesetLogError> {
    let seed = Changeset {
        parents: Vec::new(),
        author: "ada".to_owned(),
        timestamp: 0,
        message: "seed".to_owned(),
        changes: BTreeMap::new(),
    };
    let id = harness.log.append(REPOSITORY, seed).await?;
    harness
        .log
        .compare_and_swap(REPOSITORY, Ref::Main, None, id)
        .await?;
    Ok(id)
}

#[tokio::test]
async fn a_base_the_log_lacks_is_not_on_main() {
    let harness = Harness::start().await;
    seed_main(&harness).await.unwrap();

    let landed = land_on(&harness, Some(Address::of(b"absent base"))).await;

    assert_eq!(landed, Err(LandError::BaseNotOnMain));
}

#[tokio::test]
async fn a_changeset_the_log_lacks_on_the_chain_is_the_logs_failure() {
    let harness = Harness::start().await;
    let ghost = Address::of(b"absent head");
    harness
        .log
        .compare_and_swap(REPOSITORY, Ref::Main, None, ghost)
        .await
        .unwrap();

    let landed = land_on(&harness, None).await;

    assert_eq!(
        landed,
        Err(LandError::Log(ChangesetLogError::NotFound(ghost)))
    );
}
