use std::fs;
use std::io;
use std::time::Duration;

use sha2::{Digest, Sha256};
use tokio::task;

use crate::error::GthingsError;

/// A TTL-based disk cache that maps string keys to raw content.
///
/// Keys are SHA-256-hashed before being used as filenames (hex-encoded with a
/// `.json` extension). This prevents special-character issues, path-traversal
/// attacks, and overly long filenames. Expiry uses file mtime. Multiple
/// processes may safely share the same cache directory.
pub struct DiskCache {
    dir: std::path::PathBuf,
    ttl: Duration,
}

impl DiskCache {
    /// Create a new cache rooted at `dir` with the given TTL.
    ///
    /// The directory is **not** created until the first [`set()`](DiskCache::set) call.
    pub fn new(dir: impl Into<std::path::PathBuf>, ttl_secs: u64) -> Self {
        Self {
            dir: dir.into(),
            ttl: Duration::from_secs(ttl_secs),
        }
    }

    /// Hash a key to a hex-encoded filename segment.
    fn hash_key(key: &str) -> String {
        hex::encode(Sha256::digest(key.as_bytes()))
    }

    /// Return cached content for `key`, or `None` if missing or expired.
    /// Expired entries are deleted lazily before returning `None`.
    ///
    /// # Errors
    ///
    /// Returns [`GthingsError::Io`] if the cache file exists but cannot be
    /// read for reasons other than `NotFound`.
    pub async fn get(&self, key: &str) -> Result<Option<String>, GthingsError> {
        let path = self.dir.join(format!("{}.json", Self::hash_key(key)));
        let ttl = self.ttl;

        task::spawn_blocking(move || {
            let data = match fs::read_to_string(&path) {
                Ok(data) => data,
                Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
                Err(e) => return Err(GthingsError::Io(e)),
            };

            if crate::is_file_expired(&path, ttl.as_secs()) {
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
        let final_path = self.dir.join(format!("{}.json", Self::hash_key(key)));
        let data = data.to_string();
        let dir = self.dir.clone();

        task::spawn_blocking(move || {
            if !dir.exists() {
                if let Err(e) = fs::create_dir_all(&dir) {
                    tracing::debug!("cache: failed to create cache dir: {e}");
                    // Not fatal — cache is an optimisation.
                }
            }

            if let Err(e) = crate::atomic_write(&final_path, &data) {
                tracing::debug!("cache: atomic write failed: {e}");
            }
        })
        .await
        .ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn set_get_raw_content() {
        let dir = std::env::temp_dir().join("_cache_test_set_get_raw");
        let cache = DiskCache::new(&dir, 3600);
        let key = "some-unique-key";
        let content = "raw content, not JSON wrapped";
        cache.set(key, content).await;
        let retrieved = cache.get(key).await.unwrap().expect("should be present");
        assert_eq!(retrieved, content);
        // Verify the file on disk uses the SHA-256 hash of the key as filename
        let hashed = DiskCache::hash_key(key);
        let file_path = dir.join(format!("{hashed}.json"));
        let on_disk = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(on_disk, content);
        // Cleanup
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn get_expired_by_mtime() {
        let dir = std::env::temp_dir().join("_cache_test_get_expired");
        let cache = DiskCache::new(&dir, 0); // 0-second TTL — immediately expired
        let key = "will-expire-immediately";
        cache.set(key, "will expire").await;
        // The set just happened, but with TTL=0 it's already expired
        let retrieved = cache.get(key).await.unwrap();
        assert!(retrieved.is_none());
        // File should have been removed
        let hashed = DiskCache::hash_key(key);
        let file_path = dir.join(format!("{hashed}.json"));
        assert!(!file_path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn different_keys_produce_different_hashes() {
        let a = DiskCache::hash_key("hello");
        let b = DiskCache::hash_key("world");
        assert_ne!(a, b);
        assert_eq!(a.len(), 64);
    }
}
