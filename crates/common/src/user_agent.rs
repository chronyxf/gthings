//! User-agent string resolution.
//!
//! The agent string (e.g. `gthings/0.6.8`) is stamped into provenance records
//! and follow/harvest requests. Resolution order:
//! 1. `GTHINGS_AGENT` env var (runtime override, e.g. the daemon sets it to the
//!    product version it was built against).
//! 2. A compile-time stamp of this crate's version.
//!
//! The value is resolved once per process and cached.

use std::sync::OnceLock;

/// Environment variable consulted for a runtime override.
pub const ENV_AGENT: &str = "GTHINGS_AGENT";

/// Compile-time fallback stamp.
///
/// The per-crate versioning means this crate's `CARGO_PKG_VERSION` may differ
/// from the CLI product version; the runtime override exists precisely to fix
/// that skew. The daemon/CLI sets `GTHINGS_AGENT` to the product version.
pub const FALLBACK_AGENT: &str = concat!("gthings/", env!("CARGO_PKG_VERSION"));

/// Browser-like User-Agent used for web extraction so bot-protected sites
/// don't reject the daemon's requests with HTTP 403.
///
/// Unlike [`gthings_agent`] (which advertises `gthings/<version>` and is
/// trivially blocked), this mimics a real desktop Chrome so generic web
/// fetches pass common bot checks.
pub const BROWSER_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

/// Accept header value sent alongside [`BROWSER_UA`] on web extraction
/// requests, mirroring what a browser sends for HTML navigation.
pub const BROWSER_ACCEPT: &str = "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8";

/// Resolve the process-wide agent string (cached after first call).
#[must_use]
pub fn gthings_agent() -> String {
    static AGENT: OnceLock<String> = OnceLock::new();
    AGENT
        .get_or_init(|| resolve_agent(std::env::var(ENV_AGENT).ok()))
        .clone()
}

/// Resolve the agent string from an injectable env value (testable).
fn resolve_agent(env_value: Option<String>) -> String {
    match env_value {
        Some(override_value) if !override_value.trim().is_empty() => override_value,
        _ => FALLBACK_AGENT.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{ENV_AGENT, FALLBACK_AGENT, resolve_agent};

    #[test]
    fn env_override_wins() {
        assert_eq!(
            resolve_agent(Some("gthings/9.9.9".to_string())),
            "gthings/9.9.9"
        );
    }

    #[test]
    fn empty_override_falls_back() {
        assert_eq!(resolve_agent(Some("   ".to_string())), FALLBACK_AGENT);
    }

    #[test]
    fn missing_env_falls_back_to_compile_time_stamp() {
        let resolved = resolve_agent(None);
        assert!(resolved.starts_with("gthings/"), "got {resolved}");
        assert_eq!(resolved, FALLBACK_AGENT);
    }

    #[test]
    fn canonical_env_var_name() {
        assert_eq!(ENV_AGENT, "GTHINGS_AGENT");
    }
}
