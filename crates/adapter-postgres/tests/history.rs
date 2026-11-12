//! Reading a path's history through the PostgreSQL adapters.

#[cfg(test)]
pub mod common;

use common::Harness;
use test_support::contract::history;

#[tokio::test]
async fn a_path_history_query_returns_its_version_chain_newest_first() {
    history::a_path_history_query_returns_its_version_chain_newest_first::<Harness>().await;
}

#[tokio::test]
async fn a_history_query_without_a_changeset_reads_at_main() {
    history::a_history_query_without_a_changeset_reads_at_main::<Harness>().await;
}
