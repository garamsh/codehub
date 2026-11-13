//! The directory outbox: an entry is written in the transaction that creates a repository, is held
//! in order per repository, and is claimed under a lease that expires.

use crate::contract::{Deployment, Repo};
use app::domain::name::{OrganisationName, RepositoryName};
use app::port::driven::directory_changes::{ChangeKind, RepositoryChange};
use app::port::driven::directory_outbox::DirectoryOutbox;
use app::port::driving::repositories::Repositories;

/// The lease the scenarios claim for, in seconds.
const LEASE: i64 = 30;

/// The sequences of `changes`, in the order given.
fn sequences(changes: &[RepositoryChange]) -> Vec<u64> {
    changes.iter().map(|change| change.sequence).collect()
}

/// Everything the outbox holds for one claim of any repository at `now`, under a lease of one
/// second.
async fn claim_any<H: Deployment>(harness: &H, now: i64) -> Vec<RepositoryChange> {
    harness.outbox().claim(None, now, 1).await.unwrap()
}

/// The creation of a repository writes the one entry that reports it.
pub async fn creating_a_repository_writes_its_creation_entry<H: Deployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;

    let claimed = harness.outbox().claim(None, 100, LEASE).await.unwrap();

    assert_eq!(
        claimed,
        vec![RepositoryChange {
            organisation: "acme".parse().unwrap(),
            name: "app".parse().unwrap(),
            repository: repo.id,
            kind: ChangeKind::Created,
            sequence: 1,
        }]
    );
}

/// A creation that fails leaves no entry; the one that succeeded keeps its own.
pub async fn a_failed_creation_leaves_no_entry<H: Deployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let acme: OrganisationName = "acme".parse().unwrap();
    let app: RepositoryName = "app".parse().unwrap();
    let elsewhere: OrganisationName = "elsewhere".parse().unwrap();

    let duplicate = harness
        .repositories()
        .create_repository(&ada, &acme, &app)
        .await;
    let missing = harness
        .repositories()
        .create_repository(&ada, &elsewhere, &app)
        .await;

    assert!(duplicate.is_err());
    assert!(missing.is_err());
    let claimed = harness.outbox().claim(None, 100, LEASE).await.unwrap();
    assert_eq!(
        claimed
            .iter()
            .map(|change| (change.repository, change.sequence))
            .collect::<Vec<_>>(),
        vec![(repo.id, 1)]
    );
}

/// A claim returns one repository's entries in sequence order and no other repository's; without
/// a named repository it is the one whose oldest entry was written first.
pub async fn entries_are_claimed_in_sequence_order_per_repository<H: Deployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let first = harness.repository(&ada, "acme").await;
    let second = harness.repository(&ada, "other").await;
    let deleted = harness.write_change(first.id, ChangeKind::Deleted).await;
    harness.write_change(second.id, ChangeKind::Deleted).await;
    assert_eq!(deleted.sequence, 2);

    let claimed = harness.outbox().claim(None, 100, LEASE).await.unwrap();

    assert!(claimed.iter().all(|change| change.repository == first.id));
    assert_eq!(sequences(&claimed), vec![1, 2]);
    assert_eq!(claimed.last(), Some(&deleted));

    // The other repository's entries are still there, in order.
    let other = harness
        .outbox()
        .claim(Some(second.id), 100, LEASE)
        .await
        .unwrap();
    assert!(other.iter().all(|change| change.repository == second.id));
    assert_eq!(sequences(&other), vec![1, 2]);
}

/// A repository under a lease is not claimed again until the lease expires, and one repository's
/// lease does not hold another back.
pub async fn a_lease_expires_and_the_entries_are_claimed_again<H: Deployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let first = harness.repository(&ada, "acme").await;
    let second = harness.repository(&ada, "other").await;
    let outbox = harness.outbox();

    let claimed = outbox.claim(Some(first.id), 100, LEASE).await.unwrap();
    let during = outbox.claim(Some(first.id), 100 + LEASE - 1, LEASE).await;
    let elsewhere = claim_any(&harness, 100 + LEASE - 1).await;
    let after = outbox.claim(Some(first.id), 100 + LEASE, LEASE).await;

    assert_eq!(sequences(&claimed), vec![1]);
    assert_eq!(during.unwrap(), vec![]);
    assert_eq!(
        elsewhere
            .iter()
            .map(|change| change.repository)
            .collect::<Vec<_>>(),
        vec![second.id]
    );
    assert_eq!(after.unwrap(), claimed);
}

/// Releasing a claim makes the entries claimable from the time given: at once for a time not
/// after now, and not before it for a later one.
pub async fn a_released_claim_is_claimable_from_the_time_given<H: Deployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let outbox = harness.outbox();

    outbox.claim(Some(repo.id), 100, LEASE).await.unwrap();
    outbox.release(repo.id, 100).await.unwrap();
    let at_once = outbox.claim(Some(repo.id), 101, LEASE).await.unwrap();
    outbox.release(repo.id, 200).await.unwrap();
    let held_back = outbox.claim(Some(repo.id), 199, LEASE).await.unwrap();
    let from_then = outbox.claim(Some(repo.id), 200, LEASE).await.unwrap();

    assert_eq!(sequences(&at_once), vec![1]);
    assert_eq!(held_back, vec![]);
    assert_eq!(sequences(&from_then), vec![1]);
}

/// Deleting an entry removes it and no other; deleting one that is not there is not an error.
pub async fn a_deleted_entry_is_gone_and_the_others_stay<H: Deployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo: Repo<'_, H> = harness.repository(&ada, "acme").await;
    harness.write_change(repo.id, ChangeKind::Deleted).await;
    let outbox = harness.outbox();

    outbox.delete(repo.id, 1).await.unwrap();
    outbox.delete(repo.id, 1).await.unwrap();
    let remaining = outbox.claim(Some(repo.id), 100, LEASE).await.unwrap();
    outbox.delete(repo.id, 2).await.unwrap();
    outbox.release(repo.id, 100).await.unwrap();
    let none = outbox.claim(None, 100, LEASE).await.unwrap();

    assert_eq!(sequences(&remaining), vec![2]);
    assert_eq!(none, vec![]);
}
