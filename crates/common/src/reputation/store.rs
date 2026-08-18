use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

use chrono::Utc;

use super::DomainReputation;
use super::model::{DomainRecord, QualityFlag};

/// Load a domain record from disk, returning `None` if the file is missing,
/// corrupt, or outside the TTL window.
///
/// Expired files are removed lazily before returning `None`.
pub(super) fn load_record(path: &Path, ttl: Duration) -> Option<DomainRecord> {
    let data = fs::read_to_string(path).ok()?;
    if crate::util::fs::is_file_expired(path, ttl.as_secs()) {
        let _ = fs::remove_file(path);
        return None;
    }
    serde_json::from_str::<DomainRecord>(&data).ok()
}

/// Load a record (or start fresh), apply `modify`, then serialize and
/// atomically write it to disk.
///
/// Returns the final record on success, or `None` on I/O error (silently
/// skipped — the cache is an optimisation).
pub(super) fn persist_record<F>(path: &Path, ttl: Duration, modify: F) -> Option<DomainRecord>
where
    F: FnOnce(&mut DomainRecord),
{
    let mut record = load_record(path, ttl).unwrap_or_else(DomainRecord::fresh);
    modify(&mut record);
    let json = serde_json::to_string(&record).ok()?;
    crate::util::fs::atomic_write(path, &json).ok()?;
    Some(record)
}

impl DomainReputation {
    /// Merge new flags into an existing record (or create one), serialize,
    /// and atomically write to disk.
    ///
    /// Returns the final record on success, or `None` on I/O error (silently
    /// skipped — the cache is an optimisation).
    pub(super) fn merge_and_write_record(
        dir: PathBuf,
        final_path: PathBuf,
        ttl: Duration,
        flags: &[QualityFlag],
    ) -> Option<DomainRecord> {
        if let Err(e) = fs::create_dir_all(&dir) {
            tracing::debug!("domain_reputation: failed to create dir: {e}");
        }

        persist_record(&final_path, ttl, |record| {
            // Merge new flags (append unique)
            for f in flags {
                if !record.last_flags.contains(f) {
                    record.last_flags.push(f.clone());
                }
            }
            record.hit_count += 1;
            record.last_seen = Utc::now();
        })
    }

    /// Update the in-memory cache with a fresh record, so subsequent lookups
    /// skip disk I/O.
    pub(super) async fn update_memory_cache(&self, domain: &str, record: DomainRecord) {
        let mut cache = self.cache.write().await;
        cache.insert(domain.to_string(), (record, Instant::now()));
    }
}
