//! The runner's object cache: files named by address under a directory, bounded in size, from
//! which every job's files are copied.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::job_source::Address;

/// How many of a job's objects the runner downloaded and how many it took from the cache.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct ObjectCounts {
    /// Objects fetched from the server.
    pub downloaded: usize,
    /// Objects copied from the cache.
    pub cached: usize,
}

/// A directory of objects, each a file named by the hex digest of its bytes.
pub struct ObjectCache {
    dir: PathBuf,
    limit: u64,
}

impl ObjectCache {
    /// A cache in `dir` that [`evict`](Self::evict) keeps within `limit` bytes.
    #[must_use]
    pub const fn new(dir: PathBuf, limit: u64) -> Self {
        Self { dir, limit }
    }

    fn path_of(&self, address: Address) -> PathBuf {
        self.dir.join(name_of(address))
    }

    /// Copies the object at `address` to `target` and counts it as used. Returns whether the
    /// cache held it. An object whose bytes no longer hash to its address is removed, not
    /// served: the call reports a miss and `target` is left absent.
    ///
    /// # Errors
    ///
    /// An I/O error other than the object being absent.
    pub async fn take(&self, address: Address, target: &Path) -> std::io::Result<bool> {
        let mut source = match tokio::fs::File::open(self.path_of(address)).await {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error),
        };
        let mut out = tokio::fs::File::create(target).await?;
        let mut hash = Sha256::new();
        let mut buffer = vec![0_u8; 64 * 1024];
        loop {
            let count = source.read(&mut buffer).await?;
            if count == 0 {
                break;
            }
            let chunk = buffer.get(..count).unwrap_or_default();
            hash.update(chunk);
            out.write_all(chunk).await?;
        }
        out.flush().await?;
        drop(out);
        if Address::new(hash.finalize().into()) != address {
            drop(source);
            tokio::fs::remove_file(target).await?;
            remove_if_present(&self.path_of(address)).await?;
            return Ok(false);
        }
        source.into_std().await.set_modified(SystemTime::now())?;
        Ok(true)
    }

    /// Adds the file at `from`, whose bytes the caller has checked against `address`.
    ///
    /// # Errors
    ///
    /// An I/O error while copying.
    pub async fn keep(&self, address: Address, from: &Path) -> std::io::Result<()> {
        self.store(address, async |partial| {
            tokio::fs::copy(from, partial).await.map(drop)
        })
        .await
    }

    /// Adds `bytes` if they hash to `address`. Returns whether they did: bytes that do not are
    /// not stored.
    ///
    /// # Errors
    ///
    /// An I/O error while writing.
    pub async fn keep_bytes(&self, address: Address, bytes: &[u8]) -> std::io::Result<bool> {
        if Address::new(Sha256::digest(bytes).into()) != address {
            return Ok(false);
        }
        self.store(address, async |partial| {
            tokio::fs::write(partial, bytes).await
        })
        .await?;
        Ok(true)
    }

    /// Fills a partial file with `fill`, then renames it to the object's name, as last used now.
    async fn store(
        &self,
        address: Address,
        fill: impl AsyncFnOnce(&Path) -> std::io::Result<()>,
    ) -> std::io::Result<()> {
        tokio::fs::create_dir_all(&self.dir).await?;
        let name = name_of(address);
        let partial = self.dir.join(format!("tmp-{}-{name}", std::process::id()));
        let stored = async {
            fill(&partial).await?;
            let file = std::fs::File::options().write(true).open(&partial)?;
            file.set_modified(SystemTime::now())?;
            tokio::fs::rename(&partial, self.dir.join(&name)).await
        }
        .await;
        if stored.is_err() {
            remove_if_present(&partial).await?;
        }
        stored
    }

    /// Removes the partial files a runner that was killed while keeping an object left behind,
    /// those not changed for an hour. A newer one may belong to a runner that is running.
    ///
    /// # Errors
    ///
    /// An I/O error while listing or removing.
    pub async fn clean(&self) -> std::io::Result<()> {
        let mut entries = match tokio::fs::read_dir(&self.dir).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        while let Some(entry) = entries.next_entry().await? {
            if !entry.file_name().to_string_lossy().starts_with("tmp-") {
                continue;
            }
            let age = entry
                .metadata()
                .await?
                .modified()?
                .elapsed()
                .unwrap_or_default();
            if age >= PARTIAL_AGE {
                remove_if_present(&entry.path()).await?;
            }
        }
        Ok(())
    }

    /// Removes the least recently used objects until the cache holds at most its limit.
    ///
    /// # Errors
    ///
    /// An I/O error while listing or removing.
    pub async fn evict(&self) -> std::io::Result<()> {
        let mut entries = match tokio::fs::read_dir(&self.dir).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        let mut objects = Vec::new();
        let mut total = 0_u64;
        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name().to_string_lossy().into_owned();
            if format!("sha256:{name}").parse::<Address>().is_err() {
                continue;
            }
            let Ok(metadata) = entry.metadata().await else {
                continue;
            };
            total += metadata.len();
            objects.push((metadata.modified()?, name, metadata.len()));
        }
        objects.sort();
        for (_, name, size) in objects {
            if total <= self.limit {
                break;
            }
            remove_if_present(&self.dir.join(name)).await?;
            total -= size;
        }
        Ok(())
    }
}

/// How long a partial file stands unchanged before it is taken for a dead runner's.
const PARTIAL_AGE: std::time::Duration = std::time::Duration::from_secs(60 * 60);

fn name_of(address: Address) -> String {
    address.to_string().replace("sha256:", "")
}

async fn remove_if_present(path: &Path) -> std::io::Result<()> {
    match tokio::fs::remove_file(path).await {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests;
