use std::collections::HashMap;

use futures_util::StreamExt;
use tokio::sync::mpsc;

use crate::browser;
use crate::error::Result;

use super::Connection;
use super::codec::CHANNEL_CAPACITY;
use super::connect_ws;
use super::io::{InternalMessage, PendingMap, RunState};

impl Connection {
    /// Re-detect the browser WebSocket endpoint and establish a new CDP
    /// connection, replacing the background I/O loop.
    ///
    /// Uses the same 5-strategy detection cascade as [`browser::detect`]:
    /// `GTHINGS_CDP_WS_URL` env var → `/json/version` → `/json` → `/json/list`
    /// → `DevToolsActivePort`. Falls back to the stored WebSocket URL if no
    /// port can be parsed.
    pub(crate) async fn reconnect(&self) -> Result<()> {
        let ws_url = self.resolve_ws_url().await?;

        let ws_stream = connect_ws(&ws_url, "WebSocket reconnect").await?;

        // Bounded channel — same capacity as the initial connection.
        let (write_tx, write_rx) = mpsc::channel::<InternalMessage>(CHANNEL_CAPACITY);
        let pending: PendingMap = HashMap::new();
        let (ws_writer, ws_reader) = ws_stream.split();
        let events = self.events.clone();

        let handle = RunState {
            write_rx,
            ws_writer,
            ws_reader,
            pending,
            events,
        }
        .spawn();

        // Atomically replace the inner state (write channel + handle).
        let mut inner = self.lock_inner();
        inner.write = write_tx;
        inner.handle = handle;

        Ok(())
    }

    /// Resolve the WebSocket URL to use for reconnection, using the
    /// 5-strategy cascade from [`browser::detect`].
    async fn resolve_ws_url(&self) -> Result<String> {
        // 1. Environment variable bypass.
        if let Some(url) = browser::ws_url_from_env() {
            tracing::info!("reconnect: using GTHINGS_CDP_WS_URL env var");
            return Ok(url);
        }

        // 2–5. Try browser detection cascade via port.
        if let Some(port) = parse_port_from_ws_url(&self.ws_url) {
            if let Ok(browser) = browser::detect(port).await {
                tracing::info!("reconnect: detected browser via cascade");
                return Ok(browser.ws_url);
            }
        }

        // Fallback: reconnect to the same URL.
        tracing::info!("reconnect: falling back to stored ws_url");
        Ok(self.ws_url.clone())
    }
}

/// Extract the port from a `ws://host:port/path` URL.
pub(crate) fn parse_port_from_ws_url(ws_url: &str) -> Option<u16> {
    let without_scheme = ws_url.strip_prefix("ws://")?;
    let host_port = without_scheme.split('/').next()?;
    let port_str = host_port.split(':').nth(1)?;
    port_str.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::CdpError;
    use futures_util::{SinkExt, StreamExt};
    use serde_json::{Value, json};
    use std::time::Duration;
    use tokio_tungstenite::tungstenite::Message;

    // -----------------------------------------------------------------------
    // reconnect / self-healing tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_reconnect_after_ws_drop() {
        // Start a local WebSocket echo server.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let ws_url = format!("ws://{}/devtools/browser/test", addr);

        let server = spawn_echo_server(listener).await;

        // Small delay to ensure the server is listening.
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Connect.
        let conn = Connection::connect(&ws_url, None)
            .await
            .expect("connect should succeed");

        // Make a successful call.
        let result = conn.call("Test.echo", json!({"msg": "hello"}), None).await;
        assert!(result.is_ok(), "first call should succeed");

        // Kill the echo server — the TCP connection drops, the run loop
        // detects the WS error and exits.
        server.abort();
        tokio::time::sleep(Duration::from_millis(400)).await;

        // The next call should see a dead write channel → ConnectionFailed.
        let result = conn.call("Test.echo", json!({"msg": "world"}), None).await;
        assert!(
            result.is_err(),
            "call after WS drop should fail: {result:?}"
        );

        // Start a *new* server on the same address.
        let listener2 = tokio::net::TcpListener::bind(addr).await.unwrap();
        let server2 = spawn_echo_server(listener2).await;

        // Give the new server time to start.
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Reconnect (re-detection falls back to the stored ws_url).
        let reconnect_result = conn.reconnect().await;
        assert!(
            reconnect_result.is_ok(),
            "reconnect should succeed: {reconnect_result:?}"
        );

        // Make a call on the fresh connection.
        let result = conn
            .call("Test.echo", json!({"msg": "reconnected"}), None)
            .await;
        assert!(
            result.is_ok(),
            "call after reconnect should succeed: {result:?}"
        );

        // Clean up.
        server2.abort();
    }

    /// Spawn a CDP echo server that accepts connections.
    /// Handles **one connection at a time** inline (no per-connection spawn).
    /// When the returned handle is aborted, the currently in-flight connection
    /// is dropped immediately — this is how the reconnection tests simulate a
    /// clean WebSocket disconnect.
    async fn spawn_echo_server(listener: tokio::net::TcpListener) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                // NOTE: intentionally *not* spawning the handler so that
                // aborting this task drops the active WS connection.
                if let Ok(ws_stream) = tokio_tungstenite::accept_async(stream).await {
                    let (mut writer, mut reader) = ws_stream.split();
                    while let Some(Ok(msg)) = reader.next().await {
                        if let Message::Text(text) = msg {
                            if let Ok(value) = serde_json::from_str::<Value>(&text) {
                                if let Some(id) = value.get("id").and_then(|v| v.as_u64()) {
                                    let resp = json!({"id": id, "result": {"ok": true}});
                                    let _ = writer.send(Message::Text(resp.to_string())).await;
                                }
                            }
                        }
                    }
                }
            }
        })
    }

    #[tokio::test]
    async fn test_reconnect_preserves_pending_calls() {
        // Start first CDP echo server
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let ws_url = format!("ws://{}/devtools/browser/test", addr);

        let server = spawn_echo_server(listener).await;
        tokio::time::sleep(Duration::from_millis(50)).await;

        let conn = Connection::connect(&ws_url, None)
            .await
            .expect("connect should succeed");

        // Verify the connection works initially
        let result = conn.call("Test.echo", json!({"msg": "first"}), None).await;
        assert!(result.is_ok(), "first call should succeed: {result:?}");

        // Kill the echo server — the run loop detects the WS close and exits,
        // draining pending calls with ConnectionFailed errors.
        server.abort();
        tokio::time::sleep(Duration::from_millis(300)).await;

        // Start a new echo server on the same address
        let listener2 = tokio::net::TcpListener::bind(addr).await.unwrap();
        let server2 = spawn_echo_server(listener2).await;
        tokio::time::sleep(Duration::from_millis(50)).await;

        // The call() method should detect the dead connection (try_send fails or
        // the oneshot returns ConnectionFailed), reconnect via
        // `Connection::reconnect`, and retry the CDP command (bounded by
        // MAX_CALL_RETRIES) — all transparently.
        let result = conn
            .call("Test.echo", json!({"msg": "after-reconnect"}), None)
            .await;
        assert!(
            result.is_ok(),
            "call after auto-reconnect should succeed: {result:?}"
        );

        server2.abort();
    }

    #[tokio::test]
    async fn test_reconnect_fails_after_max_attempts() {
        // Use a single-connection server so abort() closes the WS immediately.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let ws_url = format!("ws://{}/devtools/browser/test", addr);

        let server = spawn_echo_server(listener).await;

        tokio::time::sleep(Duration::from_millis(50)).await;

        let conn = Connection::connect(&ws_url, None)
            .await
            .expect("connect should succeed");

        // Verify initial connection works
        let result = conn.call("Test.echo", json!({"msg": "hello"}), None).await;
        assert!(result.is_ok(), "first call should succeed: {result:?}");

        // Abort the server — the TCP connection drops, the run loop detects
        // the WS close event, drains pending calls with ConnectionFailed, and
        // exits.
        server.abort();
        tokio::time::sleep(Duration::from_millis(300)).await;

        // The call should detect the dead connection, attempt a reconnect
        // (fails — no server available), and return a ConnectionFailed error.
        // Retries are bounded by MAX_CALL_RETRIES = 3 (see codec.rs), so a
        // dead server cannot produce an infinite retry loop.
        let result = conn.call("Test.echo", json!({"msg": "fail"}), None).await;
        match result {
            Err(CdpError::ConnectionFailed { detail }) => {
                assert!(
                    detail.contains("reconnect")
                        || detail.contains("WebSocket")
                        || detail.contains("I/O loop terminated"),
                    "error should mention reconnect failure, got: {detail}"
                );
            }
            other => panic!("expected ConnectionFailed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_reconnect_doesnt_loop_infinitely() {
        // Single-connection server, aborted to force WS disconnect.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let ws_url = format!("ws://{}/devtools/browser/test", addr);

        let server = spawn_echo_server(listener).await;

        tokio::time::sleep(Duration::from_millis(50)).await;

        let conn = Connection::connect(&ws_url, None)
            .await
            .expect("connect should succeed");

        let result = conn.call("Test.echo", json!({"msg": "hello"}), None).await;
        assert!(result.is_ok(), "first call should succeed: {result:?}");

        // Abort the server — WS connection drops, run loop exits.
        server.abort();
        tokio::time::sleep(Duration::from_millis(300)).await;

        // First post-disconnect call — reconnect attempt fails (no server),
        // returns error.  If there were an infinite loop, this would hang.
        let result1 = conn
            .call("Test.echo", json!({"msg": "attempt1"}), None)
            .await;
        assert!(
            result1.is_err(),
            "first post-disconnect call should fail: {result1:?}"
        );

        // Second post-disconnect call — should also fail cleanly.  If the
        // first attempt left the connection in a looping state, this would
        // hang or panic.
        let result2 = conn
            .call("Test.echo", json!({"msg": "attempt2"}), None)
            .await;
        assert!(
            result2.is_err(),
            "second post-disconnect call should fail: {result2:?}"
        );

        // Both errors must be ConnectionFailed (not panic or timeout).
        assert!(
            matches!(&result1, Err(CdpError::ConnectionFailed { .. })),
            "error should be ConnectionFailed: {result1:?}"
        );
        assert!(
            matches!(&result2, Err(CdpError::ConnectionFailed { .. })),
            "error should be ConnectionFailed: {result2:?}"
        );
    }
}
