//! CDP browser discovery: CDP host resolution, probe URL construction, and
//! daemon-side DevTools HTTP probes (`/json/version`, `/json/list`,
//! `/json/close`, health check).
//!
//! [`urls`] owns host/URL helpers and [`probe`] owns the HTTP probing logic.
//! All daemon-side probes present `Host: localhost` so a daemon can reach
//! remote Chrome via `GTHINGS_CDP_HOST` (PROPOSAL.md §7).

mod probe;
mod urls;

pub use gthings_common::config::DEFAULT_CDP_HOST;

/// Host header value sent on all daemon-side `/json/*` probes.
///
/// Chrome's DevTools HTTP endpoints only serve requests that look local; when
/// the daemon reaches a remote Chrome via `GTHINGS_CDP_HOST`, it must present
/// `Host: localhost:<port>` for `/json/version` and `/json/close/<id>` to
/// succeed (PROPOSAL.md §7). The port is required so Chrome builds its
/// `webSocketDebuggerUrl` with the same port the daemon can reach.
fn host_header(port: u16) -> String {
    format!("localhost:{port}")
}

pub use probe::check_alive;
pub(crate) use probe::{probe_list, probe_version};
pub use urls::rewrite_ws_host;
pub(crate) use urls::{cdp_host, cdp_socket_addr, ws_probe_url};
// Re-exported only for the crate's own test suite (browser/mod.rs's
// `#[cfg(test)]` modules reference these paths); compiled solely in test builds.
#[cfg(test)]
pub(crate) use urls::{cdp_host_from, http_probe_url};
