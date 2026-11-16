//! The cache refs of a workspace instance: seeded from `main`'s, moved by compare-and-swap and
//! removed with the instance (ADR 0051).

use crate::contract::Deployment;
use app::domain::address::Address;
use app::domain::draft::Instance;
use app::domain::job::{CacheRef, empty_output_tree};
use app::domain::path::{Path, Prefix};
use app::domain::repository::RepositoryId;
use app::port::driven::cache_refs::CacheRefs;
use app::use_case::cache_refs::{
    CacheOutcome, move_workspace_cache_refs, remove_workspace_cache_refs, seed_workspace_cache_ref,
};

fn path(text: &str) -> Path {
    text.parse().unwrap()
}

/// Sets `main`'s cache ref of `cache` in the whole repository's folder to a tree made from `tree`.
async fn set_main_ref<H: Deployment>(
    harness: &H,
    repository: RepositoryId,
    cache: &Path,
    tree: &[u8],
) -> CacheRef {
    let main = CacheRef {
        tree: Address::of(tree),
        changeset: Address::of(b"landed"),
    };
    let set = harness
        .cache_refs()
        .advance_cache_ref(repository, &Prefix::whole(), cache, None, main)
        .await
        .unwrap();
    assert!(set);
    main
}

/// A first use seeds the ref from `main`'s ref of the same path, or empty where `main` has none,
/// and leaves `main`'s ref where it was.
pub async fn seeding_copies_mains_ref_and_leaves_it<H: Deployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let refs = harness.cache_refs();
    let instance = Instance::new(1);
    let (cached, bare) = (path("target"), path("node_modules"));
    let main = set_main_ref(&harness, repo.id, &cached, b"main tree").await;
    assert_eq!(
        refs.workspace_cache_ref(repo.id, instance, &cached)
            .await
            .unwrap(),
        None
    );

    let seeded = seed_workspace_cache_ref(&**refs, repo.id, instance, &cached)
        .await
        .unwrap();
    let empty = seed_workspace_cache_ref(&**refs, repo.id, instance, &bare)
        .await
        .unwrap();

    assert_eq!(seeded, main.tree);
    assert_eq!(empty, empty_output_tree());
    assert_ne!(seeded, empty);
    assert_eq!(
        refs.workspace_cache_ref(repo.id, instance, &cached)
            .await
            .unwrap(),
        Some(main.tree)
    );
    assert_eq!(
        refs.cache_ref(repo.id, &Prefix::whole(), &cached)
            .await
            .unwrap(),
        Some(main)
    );
    assert_eq!(
        refs.cache_ref(repo.id, &Prefix::whole(), &bare)
            .await
            .unwrap(),
        None
    );
}

/// A ref that exists is not seeded again: a later move of `main`'s ref does not reach it.
pub async fn a_seeded_ref_is_not_seeded_again<H: Deployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let refs = harness.cache_refs();
    let instance = Instance::new(1);
    let cached = path("target");
    let main = set_main_ref(&harness, repo.id, &cached, b"first").await;
    let first = seed_workspace_cache_ref(&**refs, repo.id, instance, &cached)
        .await
        .unwrap();
    let later = CacheRef {
        tree: Address::of(b"second"),
        changeset: Address::of(b"later"),
    };
    let moved = refs
        .advance_cache_ref(
            repo.id,
            &Prefix::whole(),
            &cached,
            Some(main.changeset),
            later,
        )
        .await
        .unwrap();
    // Control: `main`'s ref did move.
    assert!(moved);

    let again = seed_workspace_cache_ref(&**refs, repo.id, instance, &cached)
        .await
        .unwrap();

    assert_eq!(first, main.tree);
    assert_eq!(again, main.tree);
    // The create of the port does not replace a ref either.
    let created = refs
        .create_workspace_cache_ref(repo.id, instance, &cached, later.tree)
        .await
        .unwrap();
    assert!(!created);
}

/// A move from a stale tree does not move the ref, and the newer tree stays; `main`'s ref does
/// not move through any of it.
pub async fn a_move_from_a_stale_tree_leaves_the_newer_tree<H: Deployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let refs = harness.cache_refs();
    let instance = Instance::new(1);
    let cached = path("target");
    let main = set_main_ref(&harness, repo.id, &cached, b"main tree").await;
    let started = seed_workspace_cache_ref(&**refs, repo.id, instance, &cached)
        .await
        .unwrap();
    let (newer, stale) = (Address::of(b"newer"), Address::of(b"stale"));

    // Two runs started from the same tree; the first to end moves the ref.
    let outcome = |left| {
        [CacheOutcome {
            path: cached.clone(),
            started,
            left,
        }]
    };
    let first = move_workspace_cache_refs(&**refs, repo.id, instance, &outcome(newer))
        .await
        .unwrap();
    let second = move_workspace_cache_refs(&**refs, repo.id, instance, &outcome(stale))
        .await
        .unwrap();

    assert_eq!(first, Vec::<Path>::new());
    assert_eq!(second, std::slice::from_ref(&cached));
    assert_eq!(
        refs.workspace_cache_ref(repo.id, instance, &cached)
            .await
            .unwrap(),
        Some(newer)
    );
    assert_eq!(
        refs.cache_ref(repo.id, &Prefix::whole(), &cached)
            .await
            .unwrap(),
        Some(main)
    );
    // A ref that does not exist is not created by a move.
    let absent = path("absent");
    let moved = refs
        .move_workspace_cache_ref(repo.id, instance, &absent, started, newer)
        .await
        .unwrap();
    assert!(!moved);
    assert_eq!(
        refs.workspace_cache_ref(repo.id, instance, &absent)
            .await
            .unwrap(),
        None
    );
}

/// Removing an instance's refs removes those of that instance in that repository, and leaves the
/// refs of another instance, of another repository and of `main`.
pub async fn removal_removes_only_that_instances_refs<H: Deployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let other = harness.repository(&ada, "other").await;
    let refs = harness.cache_refs();
    let (closing, staying) = (Instance::new(1), Instance::new(2));
    let (cached, bare) = (path("target"), path("node_modules"));
    let main = set_main_ref(&harness, repo.id, &cached, b"main tree").await;
    let mut seeded = Vec::new();
    for (repository, instance) in [(repo.id, closing), (repo.id, staying), (other.id, closing)] {
        for cache in [&cached, &bare] {
            seed_workspace_cache_ref(&**refs, repository, instance, cache)
                .await
                .unwrap();
            seeded.push((repository, instance, cache.clone()));
        }
    }
    // Control: every ref was created.
    for (repository, instance, cache) in &seeded {
        let created = refs
            .workspace_cache_ref(*repository, *instance, cache)
            .await
            .unwrap();
        assert!(created.is_some());
    }

    remove_workspace_cache_refs(&**refs, repo.id, closing)
        .await
        .unwrap();

    for (repository, instance, cache) in &seeded {
        let held = refs
            .workspace_cache_ref(*repository, *instance, cache)
            .await
            .unwrap();
        let removed = (*repository, *instance) == (repo.id, closing);
        assert_eq!(held.is_none(), removed, "{repository} {instance}");
    }
    assert_eq!(
        refs.cache_ref(repo.id, &Prefix::whole(), &cached)
            .await
            .unwrap(),
        Some(main)
    );
}
