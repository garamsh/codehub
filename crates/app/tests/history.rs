//! Reading a path's history through the in-memory adapters.

#[cfg(test)]
mod common;

use app::domain::address::Address;
use app::port::driving::source_control::SourceControl;
use common::{Harness, draft_id};

#[tokio::test]
async fn a_path_history_query_returns_its_version_chain_newest_first() {
    let harness = Harness::default();
    let repo = harness.repository("acme").await;
    let mut landed = Vec::new();
    for (message, files) in [
        ("1", &[("a", "a1"), ("b", "b1")][..]),
        ("2", &[("b", "b2")]),
        ("3", &[("a", "a3")]),
        ("4", &[("b", "b4")]),
        ("5", &[("a", "a5")]),
    ] {
        let draft = draft_id("ada", message);
        repo.stage(&draft, files).await;
        landed.push(repo.land(&draft, message).await.unwrap());
    }
    let main = repo.main().await.unwrap();
    assert_eq!(landed.last(), Some(&main));
    assert_eq!(repo.read("a").await.as_deref(), Some("a5"));

    let a = harness
        .source_control()
        .history(repo.id, main, &"a".parse().unwrap())
        .await
        .unwrap();
    assert_eq!(
        a.iter().map(|node| node.changeset).collect::<Vec<_>>(),
        [landed[4], landed[2], landed[0]]
    );
    assert_eq!(
        a.iter().map(|node| node.file.content).collect::<Vec<_>>(),
        [Address::of(b"a5"), Address::of(b"a3"), Address::of(b"a1")]
    );

    let b = harness
        .source_control()
        .history(repo.id, main, &"b".parse().unwrap())
        .await
        .unwrap();
    assert_eq!(
        b.iter().map(|node| node.changeset).collect::<Vec<_>>(),
        [landed[3], landed[1], landed[0]]
    );
}
