use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tracing;

use crate::daemon::CdpDaemon;
use crate::ipc::{DaemonRequest, DaemonResponse};

/// Unix Domain Socket server that accepts NDJSON commands from a CLI and
/// routes them to the [`CdpDaemon`].
pub struct DaemonServer {
    socket_path: std::path::PathBuf,
    daemon: Arc<CdpDaemon>,
}

impl DaemonServer {
    pub fn new(socket_path: impl Into<std::path::PathBuf>, daemon: Arc<CdpDaemon>) -> Self {
        Self {
            socket_path: socket_path.into(),
            daemon,
        }
    }

    /// Start listening for UDS connections.
    ///
    /// Removes any stale socket file, binds the listener, and accepts
    /// connections in a loop. Each connection is handled in a separate
    /// Tokio task.
    pub async fn run(&self) -> Result<(), common::GthingsError> {
        // Remove old socket file if it exists.
        let _ = tokio::fs::remove_file(&self.socket_path).await;

        let listener = tokio::net::UnixListener::bind(&self.socket_path)?;

        // Restrict permissions to owner-only.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perm = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(&self.socket_path, perm)?;
        }

        tracing::info!("UDS server listening on {}", self.socket_path.display());

        loop {
            let (stream, _addr) = listener.accept().await?;
            let daemon = self.daemon.clone();

            tokio::spawn(async move {
                if let Err(e) = Self::handle_client(stream, daemon).await {
                    tracing::error!("Client handler error: {e}");
                }
            });
        }
    }

    /// Handle a single UDS client connection with NDJSON framing.
    async fn handle_client(
        stream: tokio::net::UnixStream,
        daemon: Arc<CdpDaemon>,
    ) -> Result<(), common::GthingsError> {
        let (reader, writer) = stream.into_split();
        let mut reader = tokio::io::BufReader::new(reader);
        let mut writer = tokio::io::BufWriter::new(writer);
        let mut line = String::new();

        loop {
            line.clear();
            let n = reader.read_line(&mut line).await?;
            if n == 0 {
                // EOF – client disconnected.
                break;
            }

            let req: DaemonRequest = match serde_json::from_str(line.trim()) {
                Ok(r) => r,
                Err(e) => {
                    let resp = DaemonResponse {
                        id: 0,
                        ok: false,
                        result: None,
                        error: Some(format!("Invalid JSON: {e}")),
                    };
                    Self::write_response(&mut writer, &resp).await?;
                    continue;
                }
            };

            let resp = daemon.handle_request(req).await;
            Self::write_response(&mut writer, &resp).await?;
        }

        Ok(())
    }

    /// Serialize a [`DaemonResponse`] and write it as a single NDJSON line.
    async fn write_response(
        writer: &mut tokio::io::BufWriter<tokio::net::unix::OwnedWriteHalf>,
        resp: &DaemonResponse,
    ) -> Result<(), common::GthingsError> {
        let json = serde_json::to_string(resp)
            .map_err(|e| common::GthingsError::Other(format!("JSON serialization error: {e}")))?;
        writer.write_all(json.as_bytes()).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;
        Ok(())
    }
}
