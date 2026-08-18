use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
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
pub(crate) struct DomainRecord {
    /// Quality flags observed in recent extractions.
    pub last_flags: Vec<QualityFlag>,
    /// Number of times this domain has been extracted (with flags).
    pub hit_count: u32,
    /// When this record was last updated.
    pub last_seen: DateTime<Utc>,
}

impl DomainRecord {
    /// A fresh, empty record stamped with the current time.
    pub(crate) fn fresh() -> Self {
        Self {
            last_flags: Vec::new(),
            hit_count: 0,
            last_seen: Utc::now(),
        }
    }
}
