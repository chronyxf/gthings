use chrono::Utc;

use super::DomainReputation;
use super::store::persist_record;

/// Consecutive blocking-flag hits required before a domain is blocked.
const BLOCK_THRESHOLD_HITS: u32 = 2;

impl DomainReputation {
    /// Returns `true` if the domain should be blocked without opening a CDP tab.
    ///
    /// A domain is considered blocked when:
    /// - A reputation record exists
    /// - The record is within the TTL window
    /// - The domain has been flagged with a blocking flag on [`BLOCK_THRESHOLD_HITS`]+
    ///   consecutive extraction attempts (`hit_count >= BLOCK_THRESHOLD_HITS`)
    pub async fn is_blocked(&self, domain: &str) -> bool {
        let Some(rec) = self.lookup(domain).await else {
            return false;
        };

        if rec.hit_count < BLOCK_THRESHOLD_HITS {
            return false;
        }

        rec.last_flags.iter().any(super::is_blocking_flag)
    }

    /// Clear `BotWall` and `Paywall` flags from a domain's record.
    ///
    /// Called after a clean extraction (no bot/paywall flags detected).
    /// Resets the hit count to 0 so the domain starts fresh.
    /// Also updates the in-memory cache to reflect the decayed state.
    pub async fn decay(&self, domain: &str) {
        let final_path = self.path_for(domain);
        let ttl = self.ttl;

        let result = super::run_blocking(move || {
            persist_record(&final_path, ttl, |record| {
                // Remove blocking flags (BotWall/Paywall)
                record.last_flags.retain(|f| !super::is_blocking_flag(f));
                record.hit_count = 0;
                record.last_seen = Utc::now();
            })
        })
        .await;

        // Update memory cache after decay
        if let Some(record) = result {
            self.update_memory_cache(domain, record).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::QualityFlag;
    use super::*;

    fn make_rep() -> (DomainReputation, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("temp dir");
        let rep = DomainReputation::new(dir.path(), 3600); // 1 hour TTL
        (rep, dir)
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
