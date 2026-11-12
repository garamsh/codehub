//! What only a shared database shows: concurrent writers, a second connection pool, and the
//! compare-and-swap decided by the server.

#[cfg(test)]
pub mod common;

use std::collections::BTreeMap;
use std::sync::Arc;

use adapter_postgres::{Database, PostgresChangesetLog, PostgresDerivedData, PostgresDraftStore};
use app::domain::address::Address;
use app::domain::changeset::Changeset;
use app::domain::draft::DraftId;
use app::domain::file::{File, FileKind};
use app::domain::file_version::FileVersionNode;
use app::domain::path::Path;
use app::domain::tree::Tree;
use app::port::driven::changeset_log::{ChangesetLog, ChangesetLogError, Ref};
use app::port::driven::derived_data::{DerivedData, DerivedDataError};
use app::port::driven::draft_store::DraftStore;
use app::port::driving::source_control::LandError;
use app::port::driving::source_control::SourceControlError;
use common::Harness;
use test_support::contract::{Deployment, Repo, draft};
use tokio::task::JoinSet;

/// A changeset with no parents and no changes, told apart from its siblings by `n`.
#[cfg(test)]
fn changeset(n: i64) -> Changeset {
    Changeset {
        parents: Vec::new(),
        author: "ada".to_owned(),
        timestamp: n,
        message: format!("changeset {n}"),
        changes: BTreeMap::new(),
    }
}

#[cfg(test)]
const WRITERS: usize = 8;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_drafts_on_disjoint_paths_each_land_exactly_once() {
    let harness = Arc::new(Harness::with_pool(10).await);
    let ada = harness.caller("ada").await;
    let id = harness.repository(&ada, "acme").await.id;

    let mut writers = JoinSet::new();
    for n in 0..WRITERS {
        let harness = Arc::clone(&harness);
        writers.spawn(async move {
            let ada = harness.caller("ada").await;
            let repo = Repo {
                harness: &*harness,
                id,
            };
            let name = draft(&format!("draft-{n}"));
            let path = format!("file-{n}");
            repo.stage(&ada, &name, &[(&path, &format!("content {n}\n"))])
                .await;
            let mut moved = 0_u32;
            loop {
                match repo.land(&ada, &name, &format!("land {n}")).await {
                    Ok(landed) => break (landed, moved),
                    Err(SourceControlError::Land(LandError::MainMoved { .. })) => moved += 1,
                    Err(error) => panic!("landing draft {n} failed: {error}"),
                }
            }
        });
    }
    let mut landed = Vec::new();
    while let Some(outcome) = writers.join_next().await {
        landed.push(outcome.unwrap().0);
    }

    // Every landing is a changeset of its own, and `main` reaches all of them through parents.
    let repo = Repo {
        harness: &*harness,
        id,
    };
    landed.sort();
    landed.dedup();
    assert_eq!(landed.len(), WRITERS);
    let mut at = repo.main().await;
    let mut chain = Vec::new();
    while let Some(current) = at {
        chain.push(current);
        at = harness
            .log
            .get(id, current)
            .await
            .unwrap()
            .parents
            .first()
            .copied();
    }
    chain.sort();
    assert_eq!(chain, landed);
    for n in 0..WRITERS {
        assert_eq!(
            repo.read(&format!("file-{n}")).await,
            Some(format!("content {n}\n"))
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_compare_and_swaps_from_one_value_have_one_winner() {
    let harness = Arc::new(Harness::with_pool(10).await);
    let ada = harness.caller("ada").await;
    let id = harness.repository(&ada, "acme").await.id;
    let first = harness.log.append(id, changeset(0)).await.unwrap();
    harness
        .log
        .compare_and_swap(id, Ref::Main, None, first)
        .await
        .unwrap();

    let mut racers = JoinSet::new();
    for n in 1..=WRITERS {
        let harness = Arc::clone(&harness);
        racers.spawn(async move {
            let new = harness
                .log
                .append(id, changeset(i64::try_from(n).unwrap()))
                .await
                .unwrap();
            let outcome = harness
                .log
                .compare_and_swap(id, Ref::Main, Some(first), new)
                .await;
            (new, outcome)
        });
    }
    let mut winners = Vec::new();
    let mut losers = 0;
    while let Some(raced) = racers.join_next().await {
        match raced.unwrap() {
            (new, Ok(())) => winners.push(new),
            (_, Err(ChangesetLogError::RefMoved { current })) => {
                assert_ne!(current, Some(first));
                losers += 1;
            }
            (_, Err(error)) => panic!("unexpected error: {error}"),
        }
    }

    assert_eq!(winners.len(), 1);
    assert_eq!(losers, WRITERS - 1);
    assert_eq!(
        harness.log.read_ref(id, Ref::Main).await.unwrap(),
        winners.first().copied()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_creations_of_an_unset_ref_have_one_winner() {
    let harness = Arc::new(Harness::with_pool(10).await);
    let ada = harness.caller("ada").await;
    let id = harness.repository(&ada, "acme").await.id;

    let mut racers = JoinSet::new();
    for n in 0..WRITERS {
        let harness = Arc::clone(&harness);
        racers.spawn(async move {
            let new = harness
                .log
                .append(id, changeset(i64::try_from(n).unwrap()))
                .await
                .unwrap();
            let outcome = harness.log.compare_and_swap(id, Ref::Main, None, new).await;
            (new, outcome.is_ok())
        });
    }
    let mut winners = Vec::new();
    while let Some(raced) = racers.join_next().await {
        let (new, won) = raced.unwrap();
        if won {
            winners.push(new);
        }
    }

    assert_eq!(winners.len(), 1);
    assert_eq!(
        harness.log.read_ref(id, Ref::Main).await.unwrap(),
        winners.first().copied()
    );
}

#[tokio::test]
async fn a_second_pool_on_the_same_database_sees_what_the_first_wrote() {
    let harness = Harness::new().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let name = draft("first");
    repo.stage(&ada, &name, &[("a", "a0\n")]).await;
    let landed = repo.land(&ada, &name, "first").await.unwrap();
    let staged = draft("pending");
    repo.stage(&ada, &staged, &[("b", "b0\n")]).await;

    // A restart: a new pool, and new adapters over it, on the database the first one filled.
    let restarted = Database::connect(&harness.url(), 2).await.unwrap();
    restarted.migrate().await.unwrap();
    let log = PostgresChangesetLog::new(&restarted);
    let derived = PostgresDerivedData::new(&restarted);
    let drafts = PostgresDraftStore::new(&restarted);

    assert_eq!(
        log.read_ref(repo.id, Ref::Main).await.unwrap(),
        Some(landed)
    );
    assert_eq!(
        log.get(repo.id, landed).await.unwrap(),
        harness.log.get(repo.id, landed).await.unwrap()
    );
    let root = derived.root(repo.id, landed).await.unwrap();
    assert_eq!(root, harness.derived.root(repo.id, landed).await.unwrap());
    let pending = DraftId {
        principal: ada.principal().clone(),
        name: staged,
    };
    assert_eq!(
        drafts.get(repo.id, &pending).await.unwrap(),
        harness.drafts.get(repo.id, &pending).await.unwrap()
    );
    assert_eq!(
        drafts.get(repo.id, &pending).await.unwrap().changes.len(),
        1
    );
    // The second pool writes what the first reads, and the repository numbering carries on.
    let next = log.append(repo.id, changeset(1)).await.unwrap();
    assert_eq!(harness.log.get(repo.id, next).await.unwrap(), changeset(1));
}

#[tokio::test]
async fn a_repository_reads_nothing_another_repository_wrote() {
    let harness = Harness::new().await;
    let ada = harness.caller("ada").await;
    let a = harness.repository(&ada, "acme").await.id;
    let b = harness.repository(&ada, "bolt").await.id;
    assert_ne!(a, b);

    let id = harness.log.append(a, changeset(0)).await.unwrap();
    let tree = Tree::default();
    let tree_address = Address::of(&tree.encode().unwrap());
    harness
        .derived
        .put_trees(a, id, tree_address, BTreeMap::from([(tree_address, tree)]))
        .await
        .unwrap();
    let path: Path = "f".parse().unwrap();
    let node = FileVersionNode {
        changeset: id,
        path: path.clone(),
        file: File {
            kind: FileKind::Regular,
            content: Address::of(b"content"),
        },
        previous: Vec::new(),
    };
    let node_address = Address::of(&node.encode().unwrap());
    harness
        .derived
        .put_file_versions(
            a,
            id,
            BTreeMap::from([(path, node_address)]),
            BTreeMap::from([(node_address, node)]),
        )
        .await
        .unwrap();

    // Control: repository A reads each of them back.
    assert!(harness.log.get(a, id).await.is_ok());
    assert_eq!(harness.derived.root(a, id).await.unwrap(), tree_address);
    assert!(harness.derived.tree(a, tree_address).await.is_ok());
    assert!(
        harness
            .derived
            .file_versions(a, id)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        harness
            .derived
            .file_version_node(a, node_address)
            .await
            .is_ok()
    );

    assert_eq!(
        harness.log.get(b, id).await,
        Err(ChangesetLogError::NotFound(id))
    );
    assert_eq!(
        harness.derived.root(b, id).await,
        Err(DerivedDataError::NotFound(id))
    );
    assert_eq!(
        harness.derived.tree(b, tree_address).await,
        Err(DerivedDataError::NotFound(tree_address))
    );
    // A set not derived is `None`: the port has no `NotFound` for it.
    assert_eq!(harness.derived.file_versions(b, id).await, Ok(None));
    assert_eq!(
        harness.derived.file_version_node(b, node_address).await,
        Err(DerivedDataError::NotFound(node_address))
    );
}
