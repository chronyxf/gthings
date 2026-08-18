//! `gthings serve` — run the HTTP :9080 daemon (blocking).
//!
//! Thin wrapper over `gthings_serve::run`: load the resolved configuration
//! (env overrides + canonical defaults) and hand it to the daemon composition
//! root. This blocks until a SIGTERM/SIGINT triggers the daemon's
//! drain-on-shutdown sequence, which also closes every live browser tab.

use gthings_common::config::Config;

/// Run the serve daemon until a termination signal drains it.
///
/// Returns the conventional `128 + signum` exit code produced by
/// [`gthings_serve::ServeHandle::shutdown`].
pub(crate) async fn cmd_serve() -> i32 {
    let config = Config::load();
    gthings_serve::run(config).await.shutdown().await
}
