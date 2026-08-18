//! Centralized environment-config surface for the serve daemon and CLI.
//!
//! All runtime knobs are `GTHINGS_*` env vars, resolved once by
//! [`Config::load`] (or the injectable [`Config::from_env`] for tests). No
//! clap dependency is needed here: the daemon reads raw env, and the CLI keeps
//! its own clap flags while sharing the same canonical defaults via this
//! module.

use std::collections::HashMap;
use std::path::PathBuf;

/// Default CDP host when `GTHINGS_CDP_HOST` is unset.
pub const DEFAULT_CDP_HOST: &str = "127.0.0.1";
/// Default reputation-cache TTL (24 h).
pub const DEFAULT_REPUTATION_TTL_SECS: u64 = 24 * 60 * 60;
/// Default serve bind host (loopback only).
pub const DEFAULT_SERVE_HOST: &str = "127.0.0.1";
/// Default serve bind port (see PROPOSAL §5: HTTP `:9080`).
pub const DEFAULT_SERVE_PORT: u16 = 9080;

/// Canonical env-var names.
pub mod env {
    /// CDP browser host (`GTHINGS_CDP_HOST`, default [`super::DEFAULT_CDP_HOST`]).
    pub const CDP_HOST: &str = "GTHINGS_CDP_HOST";
    /// Browser/HTTP user-agent override (canonical [`crate::user_agent::ENV_AGENT`]).
    pub const USER_AGENT: &str = crate::user_agent::ENV_AGENT;
    /// Disable `gthings update` (`GTHINGS_UPDATE_DISABLED`).
    pub const UPDATE_DISABLED: &str = "GTHINGS_UPDATE_DISABLED";
    /// Reputation-cache directory (`GTHINGS_REPUTATION_DIR`).
    pub const REPUTATION_DIR: &str = "GTHINGS_REPUTATION_DIR";
    /// Reputation-cache TTL seconds (`GTHINGS_REPUTATION_TTL_SECS`).
    pub const REPUTATION_TTL_SECS: &str = "GTHINGS_REPUTATION_TTL_SECS";
    /// Full `host:port` bind address for `gthings serve` (`GTHINGS_SERVE_BIND`).
    pub const SERVE_BIND: &str = "GTHINGS_SERVE_BIND";
    /// Port-only override for `gthings serve` (`GTHINGS_SERVE_PORT`).
    pub const SERVE_PORT: &str = "GTHINGS_SERVE_PORT";
    /// Prefix for per-command timeouts: `GTHINGS_<CMD>_TIMEOUT` (seconds).
    pub const CMD_TIMEOUT_PREFIX: &str = "GTHINGS_";
    pub const CMD_TIMEOUT_SUFFIX: &str = "_TIMEOUT";
}

/// Fully-resolved environment configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// CDP host, default [`DEFAULT_CDP_HOST`].
    pub cdp_host: String,
    /// Browser/HTTP user-agent override, if set.
    pub user_agent: Option<String>,
    /// Whether `gthings update` should be a no-op.
    pub update_disabled: bool,
    /// Persistent reputation-cache directory, if set.
    pub reputation_dir: Option<PathBuf>,
    /// Reputation-cache TTL in seconds, default [`DEFAULT_REPUTATION_TTL_SECS`].
    pub reputation_ttl_secs: u64,
    /// Serve bind address `host:port`, default `127.0.0.1:9080`.
    pub serve_bind: String,
    /// Per-command timeouts (`GTHINGS_<CMD>_TIMEOUT`), keyed by lowercase
    /// command name, in seconds.
    pub command_timeouts: HashMap<String, u64>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            cdp_host: DEFAULT_CDP_HOST.to_string(),
            user_agent: None,
            update_disabled: false,
            reputation_dir: None,
            reputation_ttl_secs: DEFAULT_REPUTATION_TTL_SECS,
            serve_bind: format!("{DEFAULT_SERVE_HOST}:{DEFAULT_SERVE_PORT}"),
            command_timeouts: HashMap::new(),
        }
    }
}

impl Config {
    /// Resolve configuration from the process environment.
    #[must_use]
    pub fn load() -> Self {
        Self::from_env(std::env::vars())
    }

    /// Resolve configuration from an injectable `(key, value)` iterator.
    ///
    /// Keeps the parse logic testable without mutating process env vars.
    #[must_use]
    pub fn from_env(iter: impl IntoIterator<Item = (String, String)>) -> Self {
        let mut cfg = Self::default();
        let mut serve_port_override: Option<u16> = None;
        let mut serve_bind_seen = false;

        for (key, value) in iter {
            match key.as_str() {
                env::CDP_HOST => cfg.cdp_host = value,
                env::USER_AGENT => cfg.user_agent = Some(value),
                env::UPDATE_DISABLED => cfg.update_disabled = parse_bool(&value),
                env::REPUTATION_DIR => cfg.reputation_dir = Some(PathBuf::from(value)),
                env::REPUTATION_TTL_SECS => {
                    if let Ok(secs) = value.parse() {
                        cfg.reputation_ttl_secs = secs;
                    }
                }
                env::SERVE_BIND => {
                    cfg.serve_bind = value;
                    serve_bind_seen = true;
                }
                env::SERVE_PORT => {
                    if let Ok(port) = value.parse::<u16>() {
                        serve_port_override = Some(port);
                    }
                }
                _ => {
                    if let Some(cmd) = key
                        .strip_prefix(env::CMD_TIMEOUT_PREFIX)
                        .and_then(|c| c.strip_suffix(env::CMD_TIMEOUT_SUFFIX))
                        .filter(|c| !c.is_empty())
                    {
                        if let Ok(secs) = value.parse() {
                            cfg.command_timeouts.insert(cmd.to_lowercase(), secs);
                        }
                    }
                }
            }
        }

        // Port-only override applies when no full bind address was given.
        if let Some(port) = serve_port_override {
            if !serve_bind_seen {
                cfg.serve_bind = format!("{DEFAULT_SERVE_HOST}:{port}");
            }
        }

        cfg
    }

    /// Per-command timeout in seconds, if configured via `GTHINGS_<CMD>_TIMEOUT`.
    #[must_use]
    pub fn command_timeout(&self, cmd: &str) -> Option<u64> {
        self.command_timeouts.get(&cmd.to_lowercase()).copied()
    }
}

/// Parse a `GTHINGS_*` boolean: `1`, `true`, `yes`, `on` (case-insensitive).
fn parse_bool(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{
        Config, DEFAULT_CDP_HOST, DEFAULT_REPUTATION_TTL_SECS, DEFAULT_SERVE_HOST,
        DEFAULT_SERVE_PORT,
    };

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn defaults_when_env_empty() {
        let cfg = Config::from_env(env(&[]));
        assert_eq!(cfg.cdp_host, DEFAULT_CDP_HOST);
        assert_eq!(cfg.user_agent, None);
        assert!(!cfg.update_disabled);
        assert_eq!(cfg.reputation_dir, None);
        assert_eq!(cfg.reputation_ttl_secs, DEFAULT_REPUTATION_TTL_SECS);
        assert_eq!(
            cfg.serve_bind,
            format!("{DEFAULT_SERVE_HOST}:{DEFAULT_SERVE_PORT}")
        );
        assert!(cfg.command_timeouts.is_empty());
        assert_eq!(cfg.command_timeout("search"), None);
    }

    #[test]
    fn parses_scalar_overrides() {
        let cfg = Config::from_env(env(&[
            ("GTHINGS_CDP_HOST", "10.0.0.5"),
            ("GTHINGS_AGENT", "gthings/9.9.9"),
            ("GTHINGS_UPDATE_DISABLED", "true"),
            ("GTHINGS_REPUTATION_DIR", "/data/reputation"),
            ("GTHINGS_REPUTATION_TTL_SECS", "600"),
        ]));
        assert_eq!(cfg.cdp_host, "10.0.0.5");
        assert_eq!(cfg.user_agent.as_deref(), Some("gthings/9.9.9"));
        assert!(cfg.update_disabled);
        assert_eq!(
            cfg.reputation_dir.as_deref(),
            Some(std::path::Path::new("/data/reputation"))
        );
        assert_eq!(cfg.reputation_ttl_secs, 600);
    }

    #[test]
    fn parse_bool_accepts_common_true_spellings() {
        let cfg = Config::from_env(env(&[("GTHINGS_UPDATE_DISABLED", "1")]));
        assert!(cfg.update_disabled);
        let cfg = Config::from_env(env(&[("GTHINGS_UPDATE_DISABLED", "no")]));
        assert!(!cfg.update_disabled);
    }

    #[test]
    fn per_command_timeouts_are_lowercased() {
        let cfg = Config::from_env(env(&[
            ("GTHINGS_SEARCH_TIMEOUT", "120"),
            ("GTHINGS_EXTRACT_TIMEOUT", "60"),
            ("GTHINGS_HARVEST_TIMEOUT", "240"),
        ]));
        assert_eq!(cfg.command_timeout("search"), Some(120));
        assert_eq!(cfg.command_timeout("EXTRACT"), Some(60));
        assert_eq!(cfg.command_timeout("harvest"), Some(240));
        assert_eq!(cfg.command_timeout("status"), None);
    }

    #[test]
    fn serve_bind_respects_port_only_override() {
        let cfg = Config::from_env(env(&[("GTHINGS_SERVE_PORT", "9090")]));
        assert_eq!(cfg.serve_bind, "127.0.0.1:9090");
        let cfg = Config::from_env(env(&[("GTHINGS_SERVE_BIND", "0.0.0.0:8443")]));
        assert_eq!(cfg.serve_bind, "0.0.0.0:8443");
        // A full bind address wins over a port-only override.
        let cfg = Config::from_env(env(&[
            ("GTHINGS_SERVE_BIND", "0.0.0.0:8443"),
            ("GTHINGS_SERVE_PORT", "9090"),
        ]));
        assert_eq!(cfg.serve_bind, "0.0.0.0:8443");
    }
}
