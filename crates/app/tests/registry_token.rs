//! The registry token a caller is given, through the registry token port.

#[cfg(test)]
pub mod common;

use common::Harness;
use test_support::contract::registry_token;

#[tokio::test]
async fn a_caller_is_granted_the_image_actions_it_holds() {
    registry_token::a_caller_is_granted_the_image_actions_it_holds::<Harness>().await;
}

#[tokio::test]
async fn a_scope_that_cannot_be_granted_is_answered_like_a_denied_one() {
    registry_token::a_scope_that_cannot_be_granted_is_answered_like_a_denied_one::<Harness>().await;
}

#[tokio::test]
async fn a_request_with_no_scope_is_granted_nothing() {
    registry_token::a_request_with_no_scope_is_granted_nothing::<Harness>().await;
}

#[tokio::test]
async fn a_request_for_another_service_or_with_too_many_scopes_is_refused() {
    registry_token::a_request_for_another_service_or_with_too_many_scopes_is_refused::<Harness>()
        .await;
}

#[tokio::test]
async fn a_request_asks_the_decision_port_twice_at_most() {
    registry_token::a_request_asks_the_decision_port_twice_at_most::<Harness>().await;
}

#[tokio::test]
async fn a_failing_authorisation_backend_issues_no_token() {
    registry_token::a_failing_authorisation_backend_issues_no_token::<Harness>().await;
}
