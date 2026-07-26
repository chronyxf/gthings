use std::fs;
use std::io;
use std::time::Duration;

use tokio::task;

use crate::error::GthingsError;

/// A SHA-256-keyed disk cache with TTL-based expiry.
///
/// Entries are stored as raw content strings, not JSON-wrapped. Expiry uses
/// file mtime. Multiple processes may safely share the same cache directory.
pub struct Sha256DiskCache {
    dir: std::path::PathBuf,
    ttl: Duration,
}

impl Sha256DiskCache {
    /// Create a new cache rooted at `dir` with the given TTL.
    ///
    /// The directory is **not** created until the first [`set()`](Sha256DiskCache::set) call.
    pub fn new(dir: impl Into<std::path::PathBuf>, ttl_secs: u64) -> Self {
        Self {
            dir: dir.into(),
            ttl: Duration::from_secs(ttl_secs),
        }
    }

    /// Return cached content for `key`, or `None` if missing or expired.
    /// Expired entries are deleted lazily before returning `None`.
    pub async fn get(&self, key: &str) -> Result<Option<String>, GthingsError> {
        let path = self.dir.join(format!("{key}.json"));
        let ttl = self.ttl;

        task::spawn_blocking(move || {
            let data = match fs::read_to_string(&path) {
                Ok(data) => data,
                Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
                Err(e) => return Err(GthingsError::Io(e)),
            };

            if is_expired(&path, ttl) {
                let _ = fs::remove_file(&path);
                return Ok(None);
            }

            Ok(Some(data))
        })
        .await
        .map_err(|e| GthingsError::Io(std::io::Error::other(e)))?
    }

    /// Store `data` in the cache under `key`.
    ///
    /// Errors are silently ignored — the cache is a performance optimisation,
    /// not a correctness requirement.
    pub async fn set(&self, key: &str, data: &str) {
        let final_path = self.dir.join(format!("{key}.json"));
        let tmp_path = self
            .dir
            .join(format!("{key}.tmp.{}.json", std::process::id()));
        let data = data.to_string();
        let dir = self.dir.clone();

        task::spawn_blocking(move || {
            if !dir.exists() {
                if let Err(e) = fs::create_dir_all(&dir) {
                    tracing::debug!("cache: failed to create cache dir: {e}");
                    // Not fatal — cache is an optimisation.
                }
            }

            match fs::write(&tmp_path, &data) {
                Ok(()) => {}
                Err(e) => {
                    tracing::debug!("cache: failed to write temp file: {e}");
                    let _ = fs::remove_file(&tmp_path);
                    return;
                }
            }

            if let Err(e) = fs::rename(&tmp_path, &final_path) {
                tracing::debug!("cache: failed to rename temp file: {e}");
                let _ = fs::remove_file(&tmp_path);
            }
        })
        .await
        .ok();
    }
}

/// Check if a cache file is older than the TTL via file mtime.
///
/// Missing files are treated as expired. Files with inaccessible metadata
/// are conservatively not expired (do not delete what we cannot verify).
fn is_expired(path: &std::path::Path, ttl: Duration) -> bool {
    match std::fs::metadata(path) {
        Ok(meta) => {
            if let Ok(modified) = meta.modified() {
                if let Ok(elapsed) = modified.elapsed() {
                    return elapsed > ttl;
                }
            }
            false
        }
        Err(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn set_get_raw_content() {
        let dir = std::env::temp_dir().join("_cache_test_set_get_raw");
        let cache = Sha256DiskCache::new(&dir, 3600);
        let key = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let content = "raw content, not JSON wrapped";
        cache.set(key, content).await;
        let retrieved = cache.get(key).await.unwrap().expect("should be present");
        assert_eq!(retrieved, content);
        // Also verify the file on disk is exactly the raw content
        let file_path = dir.join(format!("{key}.json"));
        let on_disk = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(on_disk, content);
        // Cleanup
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn get_expired_by_mtime() {
        let dir = std::env::temp_dir().join("_cache_test_get_expired");
        let cache = Sha256DiskCache::new(&dir, 0); // 0-second TTL — immediately expired
        let key = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        cache.set(key, "will expire").await;
        // The set just happened, but with TTL=0 it's already expired
        let retrieved = cache.get(key).await.unwrap();
        assert!(retrieved.is_none());
        // File should have been removed
        let file_path = dir.join(format!("{key}.json"));
        assert!(!file_path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
