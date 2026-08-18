use std::time::Duration;

use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::error::{CdpError, Result};

#[cfg(target_os = "macos")]
use crate::browser::dismiss_allow_debugging_dialog;

use super::Connection;

/// The concrete WebSocket stream type used for CDP connections.
type WsStream = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// Outcome of a single WebSocket probe against the connect task.
enum ProbeOutcome {
    /// The handshake completed — the connection is established.
    Connected(Box<WsStream>),
    /// The task finished (error or join failure); it must not be re-polled.
    Finished,
    /// The probe timed out; the task is still in flight and may be re-polled.
    TimedOut,
}

/// Poll the connect task once with `timeout`, logging under `label`.
async fn probe_once(
    connect_fut: &mut tokio::task::JoinHandle<std::result::Result<WsStream, CdpError>>,
    timeout: Duration,
    label: &str,
) -> ProbeOutcome {
    match tokio::time::timeout(timeout, &mut *connect_fut).await {
        Ok(Ok(Ok(stream))) => {
            tracing::debug!("{label} connected");
            ProbeOutcome::Connected(Box::new(stream))
        }
        Ok(Ok(Err(e))) => {
            tracing::debug!("{label} failed: {e}");
            ProbeOutcome::Finished
        }
        Ok(Err(_)) => {
            tracing::debug!("{label} timed out after {timeout:?}");
            ProbeOutcome::TimedOut
        }
        Err(_) => {
            tracing::debug!("{label} join error");
            ProbeOutcome::Finished
        }
    }
}

impl Connection {
    /// Connect to the CDP WebSocket endpoint, preferring a socket-first
    /// strategy and only falling back to the macOS dialog dismissal.
    ///
    /// Strategy:
    /// 1. **Socket/WebSocket first (3 × 3s)**: probe the DevTools WebSocket
    ///    with a 3s timeout, up to 3 attempts, with a brief ~500ms sleep
    ///    between probe failures. If any probe succeeds, the connection is
    ///    established without ever running osascript.
    /// 2. **osascript dismiss fallback (1 × 1s)**: only after all 3 probes
    ///    fail, the macOS "Allow remote debugging?" dialog may be blocking
    ///    the handshake — try to dismiss it, then do one more probe.
    /// 3. **Post-osascript probe (1 × 3s)**: one final WebSocket probe in
    ///    case the user clicked "Allow" during the osascript call.
    /// 4. **Descriptive error**: if the final connection still fails, return
    ///    a descriptive error instead of panicking.
    pub(crate) async fn connect_with_dialog(
        mut connect_fut: tokio::task::JoinHandle<std::result::Result<WsStream, CdpError>>,
        _ws_url: &str,
    ) -> Result<WsStream> {
        // Socket-first: probe the DevTools WebSocket up to 3 times with a 3s
        // timeout, sleeping ~500ms between failures. If the handshake
        // completes quickly, no dialog is blocking it and we skip the
        // expensive osascript call entirely.
        const WS_PROBE_ATTEMPTS: u32 = 3;
        const WS_PROBE_TIMEOUT: Duration = Duration::from_secs(3);
        const WS_PROBE_RETRY_SLEEP: Duration = Duration::from_millis(500);

        // A JoinHandle is single-shot: once polled to completion it cannot be
        // polled again (re-polling a completed handle panics). Track whether the
        // task has been consumed so we never re-poll it.
        let mut task_done = false;

        for attempt in 1..=WS_PROBE_ATTEMPTS {
            if task_done {
                break;
            }
            let label = format!("ws probe {attempt}/{WS_PROBE_ATTEMPTS}");
            match probe_once(&mut connect_fut, WS_PROBE_TIMEOUT, &label).await {
                ProbeOutcome::Connected(stream) => return Ok(*stream),
                ProbeOutcome::Finished => task_done = true,
                ProbeOutcome::TimedOut => {}
            }

            if attempt < WS_PROBE_ATTEMPTS {
                tokio::time::sleep(WS_PROBE_RETRY_SLEEP).await;
            }
        }

        // All probes failed; the browser debugging dialog may be blocking the
        // WebSocket handshake. Try to dismiss it via osascript (best-effort).
        #[cfg(target_os = "macos")]
        {
            dismiss_allow_debugging_dialog().await;
        }

        // One more probe in case the user clicked "Allow" during osascript.
        // Skip if the task already completed (its JoinHandle is consumed).
        if !task_done {
            if let ProbeOutcome::Connected(stream) =
                probe_once(&mut connect_fut, WS_PROBE_TIMEOUT, "post-osascript probe").await
            {
                return Ok(*stream);
            }
        }

        // Final attempt — return a descriptive error on failure.
        Err(CdpError::ConnectionFailed {
            detail: format!(
                "WebSocket connection failed after {WS_PROBE_ATTEMPTS} probes \
                 and osascript dismiss attempt"
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_connect_with_dialog_does_not_repoll_completed_handle() {
        // Spawn a task that finishes immediately with an error so the first
        // probe consumes the handle; connect_with_dialog must then return Err
        // instead of re-polling on the next probe.
        let handle: tokio::task::JoinHandle<std::result::Result<WsStream, CdpError>> =
            tokio::spawn(async {
                Err(CdpError::ConnectionFailed {
                    detail: "forced connection failure".into(),
                })
            });

        let result = Connection::connect_with_dialog(handle, "ws://127.0.0.1:1/devtools").await;
        assert!(
            result.is_err(),
            "expected an error from the completed (failed) connect task, got {result:?}"
        );
    }
}
