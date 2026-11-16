//! Tests of the object cache on a real directory.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use super::*;

static NEXT: AtomicU64 = AtomicU64::new(1);

/// A new empty directory to work in.
fn scratch() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "runner-cache-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn address_of(bytes: &[u8]) -> Address {
    Address::new(Sha256::digest(bytes).into())
}

/// Puts `bytes` in the cache and returns their address.
async fn keep(cache: &ObjectCache, work: &Path, bytes: &[u8]) -> Address {
    let from = work.join("source");
    std::fs::write(&from, bytes).unwrap();
    let address = address_of(bytes);
    cache.keep(address, &from).await.unwrap();
    address
}

/// Makes the object last used `ago` before now.
fn used(cache: &ObjectCache, address: Address, ago: Duration) {
    std::fs::File::options()
        .write(true)
        .open(cache.path_of(address))
        .unwrap()
        .set_modified(SystemTime::now() - ago)
        .unwrap();
}

#[tokio::test]
async fn a_kept_object_is_copied_out_and_an_absent_one_is_a_miss() {
    let work = scratch();
    let cache = ObjectCache::new(work.join("objects"), u64::MAX);
    let address = keep(&cache, &work, b"hello").await;
    let target = work.join("target");
    assert!(cache.take(address, &target).await.unwrap());
    assert_eq!(std::fs::read(&target).unwrap(), b"hello");
    // Control: an address the cache never held is a miss and writes nothing.
    let other = work.join("other");
    assert!(!cache.take(address_of(b"absent"), &other).await.unwrap());
    assert!(!other.exists());
    std::fs::remove_dir_all(&work).unwrap();
}

#[tokio::test]
async fn an_object_altered_on_disk_is_removed_and_not_served() {
    let work = scratch();
    let cache = ObjectCache::new(work.join("objects"), u64::MAX);
    let address = keep(&cache, &work, b"genuine").await;
    let target = work.join("target");
    // Control: before the alteration the object is served.
    assert!(cache.take(address, &target).await.unwrap());
    std::fs::write(cache.path_of(address), b"tampered").unwrap();
    std::fs::remove_file(&target).unwrap();
    assert!(!cache.take(address, &target).await.unwrap());
    assert!(!target.exists(), "the altered bytes were served");
    assert!(!cache.path_of(address).exists(), "the altered object stays");
    std::fs::remove_dir_all(&work).unwrap();
}

#[tokio::test]
async fn eviction_removes_the_least_recently_used_objects_until_within_the_limit() {
    let work = scratch();
    let cache = ObjectCache::new(work.join("objects"), 10);
    let oldest = keep(&cache, &work, b"1111").await;
    let middle = keep(&cache, &work, b"2222").await;
    let newest = keep(&cache, &work, b"3333").await;
    used(&cache, oldest, Duration::from_secs(300));
    used(&cache, middle, Duration::from_secs(200));
    used(&cache, newest, Duration::from_secs(100));
    cache.evict().await.unwrap();
    assert!(!cache.path_of(oldest).exists());
    assert!(cache.path_of(middle).exists());
    assert!(cache.path_of(newest).exists());
    // Control: using the oldest-listed object makes it survive the next eviction.
    let again = ObjectCache::new(work.join("objects"), 4);
    let kept = work.join("kept");
    assert!(again.take(middle, &kept).await.unwrap());
    again.evict().await.unwrap();
    assert!(again.path_of(middle).exists());
    assert!(!again.path_of(newest).exists());
    std::fs::remove_dir_all(&work).unwrap();
}

#[tokio::test]
async fn evicting_a_cache_that_was_never_written_is_not_an_error() {
    let work = scratch();
    ObjectCache::new(work.join("objects"), 0)
        .evict()
        .await
        .unwrap();
    std::fs::remove_dir_all(&work).unwrap();
}
