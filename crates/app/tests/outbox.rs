//! The directory outbox: written with the creation, ordered per repository, and claimed under a lease.

#[cfg(test)]
pub mod common;

use common::Harness;
use test_support::contract::outbox;

#[tokio::test]
async fn creating_a_repository_writes_its_creation_entry() {
    outbox::creating_a_repository_writes_its_creation_entry::<Harness>().await;
}

#[tokio::test]
async fn a_failed_creation_leaves_no_entry() {
    outbox::a_failed_creation_leaves_no_entry::<Harness>().await;
}

#[tokio::test]
async fn entries_are_claimed_in_sequence_order_per_repository() {
    outbox::entries_are_claimed_in_sequence_order_per_repository::<Harness>().await;
}

#[tokio::test]
async fn a_lease_expires_and_the_entries_are_claimed_again() {
    outbox::a_lease_expires_and_the_entries_are_claimed_again::<Harness>().await;
}

#[tokio::test]
async fn a_released_claim_is_claimable_from_the_time_given() {
    outbox::a_released_claim_is_claimable_from_the_time_given::<Harness>().await;
}

#[tokio::test]
async fn a_deleted_entry_is_gone_and_the_others_stay() {
    outbox::a_deleted_entry_is_gone_and_the_others_stay::<Harness>().await;
}
