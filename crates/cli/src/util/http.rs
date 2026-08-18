//! Shared HTTP client (lazily initialized, connection-pooled).

use std::sync::OnceLock;

use crate::util::DEFAULT_TIMEOUT_SECS;

pub(crate) fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(gthings_common::user_agent::gthings_agent())
            .timeout(std::time::Duration::from_secs(DEFAULT_TIMEOUT_SECS))
            .build()
            .unwrap_or_else(|e| {
                tracing::error!(error = %e, "Failed to build HTTP client");
                std::process::exit(1);
            })
    })
}
