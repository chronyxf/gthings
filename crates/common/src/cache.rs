use std::fs;
use std::io;
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::error::GthingsError;

/// A SHA-256-keyed disk cache with TTL-based expiry.
///
/// Keys are deterministically derived from a (URL, offset, max) triple using
/// SHA-256 of `"{url}|{offset}|{max}"`. Entries are stored as raw content
/// strings on disk (not JSON-wrapped). Expiry uses file mtime.
///
/// # Concurrency
///
/// Multiple processes may safely read and write the same cache directory.
/// Writes use an atomic rename pattern to avoid partial-file reads.
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

    /// Generate a deterministic SHA-256 hex key matching the TypeScript original.
    ///
    /// The hash input is `"{url}|{offset}|{max}"` — identical to
    /// `createHash('sha256').update(\`${url}|${offset}|${max}\`).digest('hex')`.
    ///
    /// This key doubles as the cache file name (with a `.json` extension).
    pub fn key(&self, url: &str, offset: usize, max: usize) -> String {
        let input = format!("{}|{}|{}", url, offset, max);
        let mut hasher = Sha256::new();
        hasher.update(input.as_bytes());
        hex_encode(hasher.finalize())
    }

    /// Return the cached content for `key`, or `None` if it does not exist or has expired.
    ///
    /// Expired entries are deleted before `None` is returned (lazy eviction).
    /// TTL is checked against the file's mtime, matching the TypeScript original.
    pub fn get(&self, key: &str) -> Result<Option<String>, GthingsError> {
        let path = self.dir.join(format!("{key}.json"));

        let data = match fs::read_to_string(&path) {
            Ok(data) => data,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(GthingsError::Io(e)),
        };

        // Check TTL via file mtime (matches TypeScript's statSync().mtimeMs)
        if is_expired(&path, self.ttl) {
            let _ = fs::remove_file(&path);
            return Ok(None);
        }

        Ok(Some(data))
    }

    /// Store `data` in the cache under the given `key`.
    ///
    /// Writes raw content (no JSON wrapping), matching the TypeScript original.
    /// This operation is best-effort: all errors are silently ignored because
    /// the cache is a performance optimisation, not a correctness requirement.
    ///
    /// Uses an atomic write pattern (temp file + rename) to avoid partial-file
    /// reads by concurrent processes.
    pub fn set(&self, key: &str, data: &str) {
        // Lazily create the cache directory.
        if !self.dir.exists() {
            if let Err(e) = fs::create_dir_all(&self.dir) {
                tracing::debug!("cache: failed to create cache dir: {e}");
                // Not fatal — cache is an optimisation.
            }
        }

        // Atomic write: write to a temp file, then rename.
        let final_path = self.dir.join(format!("{key}.json"));
        let tmp_path = self
            .dir
            .join(format!("{key}.tmp.{}.json", std::process::id()));

        match fs::write(&tmp_path, data) {
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
    }

    /// Scan the cache directory and remove every entry whose age exceeds the TTL.
    ///
    /// Returns the number of entries that were evicted.
    /// Uses file mtime for expiry checks, matching the TypeScript original.
    pub fn evict_expired(&self) -> Result<usize, GthingsError> {
        let read_dir = match fs::read_dir(&self.dir) {
            Ok(r) => r,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(0),
            Err(e) => return Err(GthingsError::Io(e)),
        };

        let mut evicted = 0usize;

        for entry in read_dir {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };

            let path = entry.path();

            // Only process .json files that match our key pattern (not .tmp files).
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let file_stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            if file_stem.len() != 64 {
                // Not a SHA-256 hex key; skip.
                continue;
            }

            if is_expired(&path, self.ttl) && fs::remove_file(&path).is_ok() {
                evicted += 1;
            }
        }

        Ok(evicted)
    }
}

// Internal helpers

/// Encode a byte slice as a lowercase hex string (no external crate dependency).
fn hex_encode(bytes: impl AsRef<[u8]>) -> String {
    const HEX_CHARS: &[u8] = b"0123456789abcdef";
    let bytes = bytes.as_ref();
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX_CHARS[(b >> 4) as usize] as char);
        out.push(HEX_CHARS[(b & 0x0f) as usize] as char);
    }
    out
}

/// Check whether a cache file is older than the TTL using the file's mtime.
///
/// Missing files are treated as expired. Files with inaccessible metadata
/// are conservatively treated as not expired (we do not delete what we
/// cannot verify).
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

    #[test]
    fn hex_encode_sha256() {
        use sha2::Digest;
        let hash = Sha256::digest(b"hello");
        let hex = hex_encode(hash);
        assert_eq!(
            hex,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn key_matches_typescript() {
        // TypeScript: createHash('sha256').update(`https://example.com|0|100`).digest('hex')
        // Expected: echo -n "https://example.com|0|100" | shasum -a 256
        let cache = Sha256DiskCache::new("/tmp/_test_cache", 3600);
        let key = cache.key("https://example.com", 0, 100);
        assert_eq!(
            key,
            "8257e20c62110aed0f61540c89ad50214dc4556b8285d0482d8ffb01d81f0a4d"
        );
    }

    #[test]
    fn key_deterministic() {
        let cache = Sha256DiskCache::new("/tmp/_test_cache", 3600);
        let k1 = cache.key("https://example.com", 0, 100);
        let k2 = cache.key("https://example.com", 0, 100);
        assert_eq!(k1, k2);
    }

    #[test]
    fn key_different_inputs() {
        let cache = Sha256DiskCache::new("/tmp/_test_cache", 3600);
        let k1 = cache.key("https://example.com", 0, 100);
        let k2 = cache.key("https://example.com", 10, 100);
        assert_ne!(k1, k2);
    }

    #[test]
    fn set_get_raw_content() {
        let dir = std::env::temp_dir().join(format!("_cache_test_{}", std::process::id()));
        let cache = Sha256DiskCache::new(&dir, 3600);
        let key = cache.key("https://set-get-test", 0, 100);
        let content = "raw content, not JSON wrapped";
        cache.set(&key, content);
        let retrieved = cache.get(&key).unwrap().expect("should be present");
        assert_eq!(retrieved, content);
        // Also verify the file on disk is exactly the raw content
        let file_path = dir.join(format!("{key}.json"));
        let on_disk = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(on_disk, content);
        // Cleanup
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn get_expired_by_mtime() {
        let dir = std::env::temp_dir().join(format!("_cache_test_{}", std::process::id()));
        let cache = Sha256DiskCache::new(&dir, 0); // 0-second TTL — immediately expired
        let key = cache.key("https://expired-test", 0, 100);
        cache.set(&key, "will expire");
        // The set just happened, but with TTL=0 it's already expired
        let retrieved = cache.get(&key).unwrap();
        assert!(retrieved.is_none());
        // File should have been removed
        let file_path = dir.join(format!("{key}.json"));
        assert!(!file_path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
