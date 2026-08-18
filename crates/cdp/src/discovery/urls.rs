use std::net::SocketAddr;

use gthings_common::config::env::CDP_HOST;

use super::DEFAULT_CDP_HOST;
use crate::env_or;

/// Resolve the remote CDP host from a `GTHINGS_CDP_HOST` value, defaulting to
/// `127.0.0.1` (a local browser) when unset or empty.
pub(crate) fn cdp_host_from(env_value: Option<&str>) -> String {
    env_or(env_value, DEFAULT_CDP_HOST)
}

/// Resolve the remote CDP host from the `GTHINGS_CDP_HOST` environment
/// variable (default `127.0.0.1`).
pub(crate) fn cdp_host() -> String {
    cdp_host_from(std::env::var(CDP_HOST).ok().as_deref())
}

/// Build an `http://` probe URL for the given CDP host.
pub(crate) fn http_probe_url(host: &str, port: u16, path: &str) -> String {
    format!("http://{host}:{port}{path}")
}

/// Build a `ws://` probe URL for the given CDP host.
pub(crate) fn ws_probe_url(host: &str, port: u16, ws_path: &str) -> String {
    format!("ws://{host}:{port}{ws_path}")
}

/// Resolve a CDP host + port to a `SocketAddr`, handling both literal IPs
/// (the default `127.0.0.1`) and DNS hostnames.
pub(crate) fn cdp_socket_addr(host: &str, port: u16) -> Option<SocketAddr> {
    if let Ok(addr) = format!("{host}:{port}").parse() {
        return Some(addr);
    }
    use std::net::ToSocketAddrs;
    (host, port).to_socket_addrs().ok()?.next()
}

/// Rewrite a browser-returned `ws://localhost[:port]/path` URL so the daemon
/// connects to the actual CDP host (`GTHINGS_CDP_HOST`, default `127.0.0.1`)
/// instead of resolving `localhost` inside its own container.
///
/// A remote Chrome reached via `Host: localhost` advertises its endpoint as
/// `ws://localhost:9222/...` (from the browser's own perspective), which must
/// be rewritten to the address the daemon can actually reach. Non-`localhost`
/// URLs (e.g. `ws://127.0.0.1:9222/...`) pass through unchanged.
pub fn rewrite_ws_host(ws_url: &str, host: &str) -> String {
    const PREFIX: &str = "ws://localhost";
    match ws_url.strip_prefix(PREFIX) {
        Some(rest) if rest.is_empty() || rest.starts_with(':') || rest.starts_with('/') => {
            format!("ws://{host}{rest}")
        }
        _ => ws_url.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rewrite_ws_host_localhost_to_remote() {
        assert_eq!(
            rewrite_ws_host("ws://localhost:9222/devtools/browser/abc", "10.0.0.7"),
            "ws://10.0.0.7:9222/devtools/browser/abc"
        );
    }

    #[test]
    fn test_rewrite_ws_host_localhost_preserves_port() {
        // The daemon sends `Host: localhost:<port>`, so Chrome advertises the
        // ws endpoint with the same port; the rewrite must preserve it.
        assert_eq!(
            rewrite_ws_host("ws://localhost:9222/devtools/browser/abc", "10.0.0.7"),
            "ws://10.0.0.7:9222/devtools/browser/abc"
        );
    }

    #[test]
    fn test_rewrite_ws_host_bare_localhost() {
        assert_eq!(
            rewrite_ws_host("ws://localhost", "10.0.0.7"),
            "ws://10.0.0.7"
        );
    }

    #[test]
    fn test_rewrite_ws_host_default_host() {
        // Default GTHINGS_CDP_HOST (127.0.0.1) still normalizes the URL.
        assert_eq!(
            rewrite_ws_host("ws://localhost:9222/devtools/browser/abc", "127.0.0.1"),
            "ws://127.0.0.1:9222/devtools/browser/abc"
        );
    }

    #[test]
    fn test_rewrite_ws_host_non_localhost_passthrough() {
        // Non-localhost ws URLs are already routable and must not be touched.
        assert_eq!(
            rewrite_ws_host("ws://127.0.0.1:9222/devtools/browser/abc", "10.0.0.7"),
            "ws://127.0.0.1:9222/devtools/browser/abc"
        );
        assert_eq!(
            rewrite_ws_host("wss://localhost:9222/devtools/browser/abc", "10.0.0.7"),
            "wss://localhost:9222/devtools/browser/abc"
        );
    }

    #[test]
    fn test_rewrite_ws_host_no_confusable_prefix() {
        // Must not rewrite a hostname that merely starts with "localhost".
        assert_eq!(
            rewrite_ws_host(
                "ws://localhost-prefix:9222/devtools/browser/abc",
                "10.0.0.7"
            ),
            "ws://localhost-prefix:9222/devtools/browser/abc"
        );
    }
}
