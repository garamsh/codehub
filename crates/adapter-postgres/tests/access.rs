//! Authenticating callers and authorising them per path, through the driving ports.

#[cfg(test)]
pub mod common;

use common::Harness;
use test_support::contract::access;

#[tokio::test]
async fn an_invalid_credential_is_rejected() {
    access::an_invalid_credential_is_rejected::<Harness>().await;
}

#[tokio::test]
async fn any_caller_creates_an_organisation_and_becomes_its_admin() {
    access::any_caller_creates_an_organisation_and_becomes_its_admin::<Harness>().await;
}

#[tokio::test]
async fn a_non_admin_cannot_create_a_repository() {
    access::a_non_admin_cannot_create_a_repository::<Harness>().await;
}

#[tokio::test]
async fn a_member_with_write_on_a_prefix_lands_inside_it_and_a_draft_beyond_it_is_rejected() {
    access::a_member_with_write_on_a_prefix_lands_inside_it_and_a_draft_beyond_it_is_rejected::<
        Harness,
    >()
    .await;
}

#[tokio::test]
async fn a_read_only_member_cannot_land() {
    access::a_read_only_member_cannot_land::<Harness>().await;
}

#[tokio::test]
async fn a_non_member_reads_and_asks_for_history_as_of_a_missing_repository() {
    access::a_non_member_reads_and_asks_for_history_as_of_a_missing_repository::<Harness>().await;
}

#[tokio::test]
async fn a_removed_member_loses_access() {
    access::a_removed_member_loses_access::<Harness>().await;
}

#[tokio::test]
async fn a_failing_authorisation_backend_refuses_the_call_with_its_description() {
    access::a_failing_authorisation_backend_refuses_the_call_with_its_description::<Harness>()
        .await;
}
