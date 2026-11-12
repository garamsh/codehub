//! Reading a path's history through the in-memory adapters.

use crate::contract::{Deployment, draft};
use app::domain::address::Address;
use app::port::driving::source_control::SourceControl;

/// A path history query returns its version chain newest first.
pub async fn a_path_history_query_returns_its_version_chain_newest_first<H: Deployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let mut landed = Vec::new();
    for (message, files) in [
        ("1", &[("a", "a1"), ("b", "b1")][..]),
        ("2", &[("b", "b2")]),
        ("3", &[("a", "a3")]),
        ("4", &[("b", "b4")]),
        ("5", &[("a", "a5")]),
    ] {
        let draft = draft(message);
        repo.stage(&ada, &draft, files).await;
        landed.push(repo.land(&ada, &draft, message).await.unwrap());
    }
    let main = repo.main().await.unwrap();
    assert_eq!(landed.last(), Some(&main));
    assert_eq!(repo.read("a").await.as_deref(), Some("a5"));

    let a = harness
        .source_control()
        .history(&ada, repo.id, Some(main), &"a".parse().unwrap())
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
        .history(&ada, repo.id, Some(main), &"b".parse().unwrap())
        .await
        .unwrap();
    assert_eq!(
        b.iter().map(|node| node.changeset).collect::<Vec<_>>(),
        [landed[3], landed[1], landed[0]]
    );
}

/// A history query without a changeset reads at main.
pub async fn a_history_query_without_a_changeset_reads_at_main<H: Deployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let path = "a".parse().unwrap();
    let source_control = harness.source_control();

    // Before anything lands there is no `main`, and so no version.
    assert_eq!(
        source_control.history(&ada, repo.id, None, &path).await,
        Ok(Vec::new())
    );

    let mut landed = Vec::new();
    for message in ["1", "2"] {
        let draft = draft(message);
        repo.stage(&ada, &draft, &[("a", message)]).await;
        landed.push(repo.land(&ada, &draft, message).await.unwrap());
    }
    let at_main = source_control
        .history(&ada, repo.id, None, &path)
        .await
        .unwrap();
    assert_eq!(
        at_main
            .iter()
            .map(|node| node.changeset)
            .collect::<Vec<_>>(),
        [landed[1], landed[0]]
    );
    // Control: an explicit changeset still reads there, not at `main`.
    let at_first = source_control
        .history(&ada, repo.id, Some(landed[0]), &path)
        .await
        .unwrap();
    assert_eq!(
        at_first
            .iter()
            .map(|node| node.changeset)
            .collect::<Vec<_>>(),
        [landed[0]]
    );
}
