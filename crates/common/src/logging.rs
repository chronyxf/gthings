use tracing_subscriber::EnvFilter;

use crate::config::GthingsConfig;

/// Initialise the global [`tracing`] subscriber based on the provided config.
///
/// The subscriber writes structured log lines to stderr using
/// [`tracing_subscriber::fmt`]. The log level is sourced from
/// [`GthingsConfig::log_level`]; if the value is not a recognised filter
/// string, `info` is used as a fallback.
///
/// # Panics
///
/// Panics if a global subscriber has already been registered.
pub fn init_tracing(config: &GthingsConfig) {
    let filter = EnvFilter::try_new(&config.log_level).unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::fmt().with_env_filter(filter).init();
}
