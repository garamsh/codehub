//! Registering runners and what a runner may claim, through the PostgreSQL adapters.

#[cfg(test)]
pub mod common;

use common::Harness;
use test_support::contract::runners;

#[tokio::test]
async fn an_administrator_registers_lists_and_removes_a_runner() {
    runners::an_administrator_registers_lists_and_removes_a_runner::<Harness>().await;
}

#[tokio::test]
async fn each_registration_gets_its_own_identifier() {
    runners::each_registration_gets_its_own_identifier::<Harness>().await;
}

#[tokio::test]
async fn a_caller_who_does_not_administer_may_not_manage_runners() {
    runners::a_caller_who_does_not_administer_may_not_manage_runners::<Harness>().await;
}

#[tokio::test]
async fn an_unregistered_principal_is_refused() {
    runners::an_unregistered_principal_is_refused::<Harness>().await;
}

#[tokio::test]
async fn a_runner_claims_only_jobs_whose_labels_it_offers() {
    runners::a_runner_claims_only_jobs_whose_labels_it_offers::<Harness>().await;
}

#[tokio::test]
async fn a_claimed_job_is_not_offered_to_another_runner() {
    runners::a_claimed_job_is_not_offered_to_another_runner::<Harness>().await;
}

#[tokio::test]
async fn a_runner_claims_only_jobs_within_its_scope() {
    runners::a_runner_claims_only_jobs_within_its_scope::<Harness>().await;
}

#[tokio::test]
async fn a_claim_lists_the_files_of_the_changeset() {
    runners::a_claim_lists_the_files_of_the_changeset::<Harness>().await;
}
