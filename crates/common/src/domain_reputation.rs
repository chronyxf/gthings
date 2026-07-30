//! Domain reputation cache — first-pass bot-wall / pay-wall filter.
//!
//! Stores known-bad domains on disk so subsequent requests can short-circuit
//! before opening a CDP tab. Uses the same atomic temp+rename write pattern
//! as [`DiskCache`](crate::cache::DiskCache).
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
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tokio::task;

/// Quality flags that can be associated with a domain.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QualityFlag {
    /// Bot challenge / "checking your browser" page.
    BotWall,
    /// Paywall / subscription prompt.
    Paywall,
    /// CAPTCHA challenge page.
    Captcha,
    /// Empty or JS-required page shell.
    EmptyShell,
    /// Garbled / unparseable content.
    Garbled,
    /// Very thin content (< 80 chars or < 10 words).
    ThinContent,
    /// Content was truncated by the extractor.
    Truncated,
}

/// A single domain reputation record persisted to disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainRecord {
    /// Quality flags observed in recent extractions.
    pub last_flags: Vec<QualityFlag>,
    /// Number of times this domain has been extracted (with flags).
    pub hit_count: u32,
    /// When this record was last updated.
    pub last_seen: DateTime<Utc>,
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

/// Load a domain record from disk, returning `None` if the file is missing,
/// corrupt, or outside the TTL window.
///
/// Expired files are removed lazily before returning `None`.
fn load_record(path: &Path, ttl: Duration) -> Option<DomainRecord> {
    let data = fs::read_to_string(path).ok()?;
    if crate::is_file_expired(path, ttl.as_secs()) {
        let _ = fs::remove_file(path);
        return None;
    }
    serde_json::from_str::<DomainRecord>(&data).ok()
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

    /// Look up the reputation record for a domain.
    ///
    /// Returns `None` if no record exists, the file cannot be read, or the
    /// record is outside the TTL window (expired records are deleted lazily).
    ///
    /// An in-memory cache avoids redundant disk I/O for frequently-checked
    /// domains. Memory entries expire at the same TTL as the file-level cache.
    ///
    /// The `.ok()?` pattern used internally is intentional — the cache is a
    /// performance optimisation, and I/O errors should not block extraction.
    pub async fn lookup(&self, domain: &str) -> Option<DomainRecord> {
        // Check memory cache first
        {
            let cache = self.cache.read().await;
            if let Some((record, cached_at)) = cache.get(domain) {
                if cached_at.elapsed() < self.ttl {
                    return Some(record.clone());
                }
            }
        }

        let path = self.dir.join(format!("{domain}.json"));
        let ttl = self.ttl;

        let result = task::spawn_blocking(move || load_record(&path, ttl))
            .await
            .ok()
            .flatten();

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
    ///
    /// The `.ok()` pattern used internally is intentional — the cache is a
    /// performance optimisation, and I/O errors should not block extraction.
    /// Merge new flags into an existing record (or create one), serialize,
    /// and atomically write to disk.
    ///
    /// Returns the final record on success, or `None` on I/O error (silently
    /// skipped — the cache is an optimisation).
    fn merge_and_write_record(
        dir: PathBuf,
        final_path: PathBuf,
        ttl: Duration,
        flags: &[QualityFlag],
    ) -> Option<DomainRecord> {
        if !dir.exists() {
            if let Err(e) = fs::create_dir_all(&dir) {
                tracing::debug!("domain_reputation: failed to create dir: {e}");
            }
        }

        let mut record = load_record(&final_path, ttl).unwrap_or(DomainRecord {
            last_flags: Vec::new(),
            hit_count: 0,
            last_seen: Utc::now(),
        });

        // Merge new flags (append unique)
        for f in flags {
            if !record.last_flags.contains(f) {
                record.last_flags.push(f.clone());
            }
        }
        record.hit_count += 1;
        record.last_seen = Utc::now();

        let json = match serde_json::to_string(&record) {
            Ok(j) => j,
            Err(e) => {
                tracing::debug!("domain_reputation: serialization failed: {e}");
                return None;
            }
        };

        if let Err(e) = crate::atomic_write(&final_path, &json) {
            tracing::debug!("domain_reputation: atomic write failed: {e}");
            return None;
        }

        Some(record)
    }

    /// Update the in-memory cache with a fresh record, so subsequent lookups
    /// skip disk I/O.
    async fn update_memory_cache(&self, domain: &str, record: DomainRecord) {
        let mut cache = self.cache.write().await;
        cache.insert(domain.to_string(), (record, Instant::now()));
    }

    pub async fn write(&self, domain: &str, flags: &[QualityFlag]) {
        let final_path = self.dir.join(format!("{domain}.json"));
        let dir = self.dir.clone();
        let ttl = self.ttl;
        let flags_vec = flags.to_vec();

        let record = task::spawn_blocking(move || {
            Self::merge_and_write_record(dir, final_path, ttl, &flags_vec)
        })
        .await
        .ok()
        .flatten();

        // Update memory cache with the written record
        if let Some(record) = record {
            self.update_memory_cache(domain, record).await;
        }
    }

    /// Returns `true` if the domain should be blocked without opening a CDP tab.
    ///
    /// A domain is considered blocked when:
    /// - A reputation record exists
    /// - The record is within the TTL window
    /// - The domain has been flagged with `BotWall` or `Paywall` on **2+ consecutive**
    ///   extraction attempts (`hit_count >= 2`)
    pub async fn is_blocked(&self, domain: &str) -> bool {
        let Some(rec) = self.lookup(domain).await else {
            return false;
        };

        if rec.hit_count < 2 {
            return false;
        }

        rec.last_flags
            .iter()
            .any(|f| matches!(f, QualityFlag::BotWall | QualityFlag::Paywall))
    }

    /// Clear `BotWall` and `Paywall` flags from a domain's record.
    ///
    /// Called after a clean extraction (no bot/paywall flags detected).
    /// Resets the hit count to 0 so the domain starts fresh.
    /// Also updates the in-memory cache to reflect the decayed state.
    ///
    /// The `.ok()?` pattern used internally is intentional — the cache is a
    /// performance optimisation, and I/O errors should not block extraction.
    pub async fn decay(&self, domain: &str) {
        let final_path = self.dir.join(format!("{domain}.json"));
        let ttl = self.ttl;

        let result = task::spawn_blocking(move || -> Option<DomainRecord> {
            let mut record: DomainRecord = load_record(&final_path, ttl)?;

            // Remove BotWall and Paywall flags
            record
                .last_flags
                .retain(|f| !matches!(f, QualityFlag::BotWall | QualityFlag::Paywall));
            record.hit_count = 0;
            record.last_seen = Utc::now();

            let json = serde_json::to_string(&record).ok()?;
            crate::atomic_write(&final_path, &json).ok()?;
            Some(record)
        })
        .await
        .ok()
        .flatten();

        // Update memory cache after decay
        if let Some(record) = result {
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
    async fn test_is_blocked_after_two_botwall_hits() {
        let (rep, _dir) = make_rep();
        assert!(!rep.is_blocked("example.com").await, "no record yet");

        rep.write("example.com", &[QualityFlag::BotWall]).await;
        assert!(!rep.is_blocked("example.com").await, "need 2+ hits");

        rep.write("example.com", &[QualityFlag::BotWall]).await;
        assert!(rep.is_blocked("example.com").await, "should be blocked");
    }

    #[tokio::test]
    async fn test_is_blocked_two_paywall_hits() {
        let (rep, _dir) = make_rep();
        rep.write("example.com", &[QualityFlag::Paywall]).await;
        rep.write("example.com", &[QualityFlag::Paywall]).await;
        assert!(rep.is_blocked("example.com").await);
    }

    #[tokio::test]
    async fn test_not_blocked_for_other_flags() {
        let (rep, _dir) = make_rep();
        rep.write("example.com", &[QualityFlag::Captcha]).await;
        rep.write("example.com", &[QualityFlag::Captcha]).await;
        // Captcha alone does NOT trigger block (only BotWall/Paywall)
        assert!(!rep.is_blocked("example.com").await);
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

    #[tokio::test]
    async fn test_decay_clears_botwall_flags() {
        let (rep, _dir) = make_rep();
        rep.write("example.com", &[QualityFlag::BotWall]).await;
        rep.write("example.com", &[QualityFlag::Paywall]).await;
        assert!(rep.is_blocked("example.com").await);

        // Clean extraction — decay
        rep.decay("example.com").await;

        let rec = rep.lookup("example.com").await.expect("should still exist");
        assert_eq!(rec.hit_count, 0);
        assert!(!rec.last_flags.contains(&QualityFlag::BotWall));
        assert!(!rec.last_flags.contains(&QualityFlag::Paywall));
        assert!(!rep.is_blocked("example.com").await);
    }

    #[tokio::test]
    async fn test_decay_preserves_other_flags() {
        let (rep, _dir) = make_rep();
        rep.write("example.com", &[QualityFlag::BotWall, QualityFlag::Captcha])
            .await;

        rep.decay("example.com").await;

        let rec = rep.lookup("example.com").await.expect("should exist");
        assert!(!rec.last_flags.contains(&QualityFlag::BotWall));
        // Captcha should survive decay
        assert!(rec.last_flags.contains(&QualityFlag::Captcha));
    }
}
