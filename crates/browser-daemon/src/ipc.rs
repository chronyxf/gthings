/// Request from CLI to daemon over the Unix Domain Socket.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DaemonRequest {
    pub id: u64,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

/// Response from daemon to CLI over the Unix Domain Socket.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DaemonResponse {
    pub id: u64,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Health / status check result returned by the daemon.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DaemonStatus {
    pub running: bool,
    pub pid: Option<u32>,
    pub cdp_port: Option<u16>,
    pub chrome_connected: bool,
    pub uptime_secs: Option<u64>,
    pub version: Option<String>,
}
