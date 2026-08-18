//! Domain reputation cache — first-pass bot-wall / pay-wall filter.
//!
//! Stores known-bad domains on disk so subsequent requests can short-circuit
//! before opening a CDP tab. Uses the same atomic temp+rename write pattern
//! as [`crate::util::fs::atomic_write`].
//!
//! # Cache key format
//!
//! Files are stored as `{reputation_dir}/{domain}.json`.
//!
//! # TTL
//!
//! Default TTL is 24 hours. Configurable via [`DomainReputation::new`].
//!
//! # Decay rule
//!
//! One clean extraction (no BotWall/Paywall flags) clears those flags from
//! the domain record and resets the hit count, allowing the domain to be
//! re-tried immediately.
//!
//! # Design note: intentional `ok()` silencing
//!
//! Several methods use `.ok()?` or `.ok().flatten()` to silently skip errors
//! from disk I/O or deserialisation. This is intentional — the reputation
//! cache is a performance optimisation, not a correctness requirement. A
//! missing or corrupt file simply means "no reputation data", which is the
//! safe default (allow extraction, don't block).

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

use tokio::sync::RwLock;
use tokio::task;

mod model;
mod policy;
mod store;

pub(crate) use model::DomainRecord;
pub use model::QualityFlag;
use store::load_record;

/// Returns `true` for quality flags that indicate a persistent bot/paywall
/// wall warranting a hard block: `BotWall` or `Paywall`.
///
/// `Captcha` is intentionally excluded — captchas are often transient
/// (rate-limiting) and should not permanently block a domain.
#[must_use]
pub(crate) const fn is_blocking_flag(flag: &QualityFlag) -> bool {
    matches!(flag, QualityFlag::BotWall | QualityFlag::Paywall)
}

/// A single quality flag that, on its own, is a hard (blocking) failure.
///
/// Returns `true` for flags that should stop extraction immediately:
/// `BotWall` or `Paywall` (see [`is_blocking_flag`]).
#[must_use]
pub const fn quality_flag_is_blocking(flag: &QualityFlag) -> bool {
    is_blocking_flag(flag)
}

/// Run a fallible blocking closure on the blocking thread-pool, returning
/// its `Option` result.
///
/// Spawn/cancellation errors are silently ignored (`.ok().flatten()`) — the
/// reputation cache is a performance optimisation, not a correctness
/// requirement.
async fn run_blocking<F, T>(f: F) -> Option<T>
where
    F: FnOnce() -> Option<T> + Send + 'static,
    T: Send + 'static,
{
    task::spawn_blocking(f).await.ok().flatten()
}

/// On-disk domain reputation cache.
///
/// Each domain has a single JSON file. The cache is a performance
/// optimisation — errors are silently ignored.
pub struct DomainReputation {
    dir: PathBuf,
    ttl: Duration,
    cache: RwLock<HashMap<String, (DomainRecord, Instant)>>,
}

impl DomainReputation {
    /// Create a new reputation cache rooted at `dir` with the given TTL
    /// in seconds.
    ///
    /// The directory is created lazily on the first write.
    pub fn new(dir: impl Into<PathBuf>, ttl_secs: u64) -> Self {
        Self {
            dir: dir.into(),
            ttl: Duration::from_secs(ttl_secs),
            cache: RwLock::new(HashMap::new()),
        }
    }

    /// Path of the on-disk record file for a domain.
    fn path_for(&self, domain: &str) -> PathBuf {
        self.dir.join(format!("{domain}.json"))
    }

    /// Look up the reputation record for a domain.
    ///
    /// Returns `None` if no record exists, the file cannot be read, or the
    /// record is outside the TTL window (expired records are deleted lazily).
    ///
    /// An in-memory cache avoids redundant disk I/O for frequently-checked
    /// domains. Memory entries expire at the same TTL as the file-level cache.
    pub(crate) async fn lookup(&self, domain: &str) -> Option<DomainRecord> {
        // Check memory cache first, evicting expired entries.
        {
            let mut cache = self.cache.write().await;
            if let Some((record, cached_at)) = cache.get(domain) {
                if cached_at.elapsed() < self.ttl {
                    return Some(record.clone());
                }
                // Expired — evict from the memory cache.
                cache.remove(domain);
            }
        }

        let path = self.path_for(domain);
        let ttl = self.ttl;

        let result = run_blocking(move || load_record(&path, ttl)).await;

        // Populate memory cache from successful disk read
        if let Some(ref record) = result {
            self.update_memory_cache(domain, record.clone()).await;
        }

        result
    }

    /// Write quality flags for a domain.
    ///
    /// If a record already exists, the flags are appended (deduplicated),
    /// `hit_count` is incremented, and `last_seen` is updated. If no record
    /// exists, a fresh one is created with `hit_count` = 1.
    ///
    /// Uses atomic temp+rename to avoid partial writes.
    /// Also updates the in-memory cache so subsequent lookups skip disk I/O.
    pub async fn write(&self, domain: &str, flags: &[QualityFlag]) {
        let final_path = self.path_for(domain);
        let dir = self.dir.clone();
        let ttl = self.ttl;
        let flags_vec = flags.to_vec();

        let record =
            run_blocking(move || Self::merge_and_write_record(dir, final_path, ttl, &flags_vec))
                .await;

        // Update memory cache with the written record
        if let Some(record) = record {
            self.update_memory_cache(domain, record).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_rep() -> (DomainReputation, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("temp dir");
        let rep = DomainReputation::new(dir.path(), 3600); // 1 hour TTL
        (rep, dir)
    }

    #[tokio::test]
    async fn test_lookup_missing() {
        let (rep, _dir) = make_rep();
        assert!(rep.lookup("example.com").await.is_none());
    }

    #[tokio::test]
    async fn test_write_and_lookup() {
        let (rep, _dir) = make_rep();
        rep.write("example.com", &[QualityFlag::BotWall]).await;

        let rec = rep.lookup("example.com").await.expect("should exist");
        assert_eq!(rec.hit_count, 1);
        assert!(rec.last_flags.contains(&QualityFlag::BotWall));
    }

    #[tokio::test]
    async fn test_write_increments_hit_count() {
        let (rep, _dir) = make_rep();
        rep.write("example.com", &[QualityFlag::BotWall]).await;
        rep.write("example.com", &[QualityFlag::BotWall]).await;

        let rec = rep.lookup("example.com").await.expect("should exist");
        assert_eq!(rec.hit_count, 2);
    }

    #[tokio::test]
    async fn test_ttl_expiry_causes_cold_cache() {
        let dir = tempfile::tempdir().expect("temp dir");
        let rep = DomainReputation::new(dir.path(), 0); // 0-second TTL

        rep.write("example.com", &[QualityFlag::BotWall]).await;
        rep.write("example.com", &[QualityFlag::BotWall]).await;

        // TTL is 0, so the record should be treated as expired
        assert!(!rep.is_blocked("example.com").await);
        assert!(rep.lookup("example.com").await.is_none());
    }
}
