/// Application configuration loaded from environment variables and defaults.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GthingsConfig {
    /// Port for the CDP (Chrome DevTools Protocol) endpoint.
    pub cdp_port: u16,
    /// Optional custom path to the browser executable.
    pub browser_path: Option<std::path::PathBuf>,
    /// Optional custom browser profile directory.
    pub profile_dir: Option<std::path::PathBuf>,
    /// Directory used for persistent disk cache.
    pub cache_dir: std::path::PathBuf,
    /// Cache TTL in seconds.
    pub cache_ttl_secs: u64,
    /// Log level filter string (e.g. "info", "debug", "warn").
    pub log_level: String,
    /// Request timeout in milliseconds.
    pub request_timeout_ms: u64,
}

impl Default for GthingsConfig {
    fn default() -> Self {
        Self {
            cdp_port: 9222,
            browser_path: None,
            profile_dir: None,
            cache_dir: std::path::PathBuf::from("/tmp/nyx-search-cache"),
            cache_ttl_secs: 3600,
            log_level: "info".to_string(),
            request_timeout_ms: 30_000,
        }
    }
}

impl GthingsConfig {
    /// Build a [`GthingsConfig`] by reading environment variables.
    ///
    /// Each field can be overridden by its corresponding environment variable;
    /// values not set will fall back to [`GthingsConfig::default()`].
    ///
    /// | Variable                  | Field              |
    /// |---------------------------|--------------------|
    /// | `GTHINGS_CDP_PORT`        | `cdp_port`         |
    /// | `GTHINGS_BROWSER_PATH`    | `browser_path`     |
    /// | `GTHINGS_PROFILE_DIR`     | `profile_dir`      |
    /// | `GTHINGS_CACHE_DIR`       | `cache_dir`        |
    /// | `GTHINGS_CACHE_TTL_SECS`  | `cache_ttl_secs`   |
    /// | `GTHINGS_LOG_LEVEL`       | `log_level`        |
    /// | `GTHINGS_REQUEST_TIMEOUT` | `request_timeout_ms` |
    pub fn from_env() -> Self {
        let defaults = Self::default();

        Self {
            cdp_port: std::env::var("GTHINGS_CDP_PORT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(defaults.cdp_port),
            browser_path: std::env::var("GTHINGS_BROWSER_PATH")
                .ok()
                .map(std::path::PathBuf::from),
            profile_dir: std::env::var("GTHINGS_PROFILE_DIR")
                .ok()
                .map(std::path::PathBuf::from),
            cache_dir: std::env::var("GTHINGS_CACHE_DIR")
                .ok()
                .map(std::path::PathBuf::from)
                .unwrap_or(defaults.cache_dir),
            cache_ttl_secs: std::env::var("GTHINGS_CACHE_TTL_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(defaults.cache_ttl_secs),
            log_level: std::env::var("GTHINGS_LOG_LEVEL")
                .ok()
                .unwrap_or(defaults.log_level),
            request_timeout_ms: std::env::var("GTHINGS_REQUEST_TIMEOUT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(defaults.request_timeout_ms),
        }
    }
}
