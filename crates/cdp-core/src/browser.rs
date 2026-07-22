use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::connection::{CdpConnection, discover_ws_url};
use crate::error::CdpError;
use crate::session::Session;

/// Information about a browser target (page, iframe, worker, etc.).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetInfo {
    pub target_id: String,
    #[serde(rename = "type")]
    pub target_type: String,
    pub title: String,
    pub url: String,
    pub attached: bool,
    pub opener_id: Option<String>,
    pub browser_context_id: Option<String>,
}

/// High-level browser handle providing target management and tab lifecycle.
///
/// Wraps a `CdpConnection` and exposes convenience methods for listing,
/// creating, and closing targets.
pub struct Browser {
    conn: Arc<CdpConnection>,
}

impl Browser {
    /// Connect to a browser instance on the given debugging port.
    ///
    /// Discovers the WebSocket URL via the HTTP endpoint, then establishes
    /// a CDP connection.
    pub async fn connect(port: u16) -> Result<Self, common::GthingsError> {
        let ws_url = discover_ws_url(port).await?;
        let conn = CdpConnection::connect(&ws_url)
            .await
            .map_err(|e| common::GthingsError::Other(format!("CDP connection failed: {e}")))?;
        Ok(Browser { conn })
    }

    /// List all targets (pages, iframes, workers, etc.) attached to the
    /// browser.
    pub async fn list_targets(&self) -> Result<Vec<TargetInfo>, CdpError> {
        let result = self.conn.call("Target.getTargets", None).await?;
        let targets: Vec<TargetInfo> = serde_json::from_value(result["targetInfos"].clone())?;
        Ok(targets)
    }

    /// Create a new page target and attach a session to it.
    pub async fn create_target(&self, url: &str) -> Result<Session, CdpError> {
        let params = serde_json::json!({ "url": url });
        let result = self.conn.call("Target.createTarget", Some(params)).await?;
        let target_id = result["targetId"]
            .as_str()
            .ok_or_else(|| CdpError::Other("No targetId in create response".to_string()))?
            .to_string();
        Session::attach(&self.conn, &target_id).await
    }

    /// Close a browser target by its ID.
    pub async fn close_target(&self, target_id: &str) -> Result<(), CdpError> {
        let params = serde_json::json!({ "targetId": target_id });
        self.conn.call("Target.closeTarget", Some(params)).await?;
        Ok(())
    }

    /// Get a reference to the underlying CDP connection for direct calls.
    pub fn connection(&self) -> &Arc<CdpConnection> {
        &self.conn
    }

    /// Close the browser connection.
    pub async fn close(self) -> Result<(), CdpError> {
        self.conn.close().await
    }
}
