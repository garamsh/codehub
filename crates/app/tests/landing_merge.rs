//! A landing that merges with what `main` gained keeps what only the landing changed.

pub mod common;

use common::Harness;
use test_support::contract::{Deployment, draft};

#[tokio::test]
async fn a_merged_landing_keeps_the_paths_only_it_changed() {
    let harness = Harness::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let seed = draft("seed");
    repo.stage(&ada, &seed, &[("f", "1\n2\n3\n"), ("kept", "x\n")])
        .await;
    repo.land(&ada, &seed, "seed").await.unwrap();
    let (first, second) = (draft("first"), draft("second"));
    repo.stage(&ada, &first, &[("f", "1\n2\nthree\n")]).await;
    repo.stage(&ada, &second, &[("f", "one\n2\n3\n"), ("new", "n\n")])
        .await;
    repo.land(&ada, &first, "first").await.unwrap();

    repo.land(&ada, &second, "second").await.unwrap();

    assert_eq!(repo.read("f").await.as_deref(), Some("one\n2\nthree\n"));
    assert_eq!(repo.read("new").await.as_deref(), Some("n\n"));
    // Control: a path neither side touched stays.
    assert_eq!(repo.read("kept").await.as_deref(), Some("x\n"));
}
