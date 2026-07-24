use tracing_subscriber::EnvFilter;

use crate::config::GthingsConfig;

/// Initialise the global [`tracing`] subscriber from the provided config.
///
/// The log level comes from [`GthingsConfig::log_level`]; unrecognised values
/// fall back to `info`.
///
/// # Panics
///
/// Panics if a global subscriber has already been registered.
pub fn init_tracing(config: &GthingsConfig) {
    let filter = EnvFilter::try_new(&config.log_level).unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::fmt().with_env_filter(filter).init();
}
