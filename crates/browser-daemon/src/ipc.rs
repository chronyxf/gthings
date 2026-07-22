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

/// Self-describing context from the daemon — captures browser identity
/// and connection state. Used to enrich CLI trace records so every
/// trace line is self-describing.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DaemonContext {
    /// PID of the browser-daemon process itself.
    pub daemon_pid: u32,
    /// PID of the Chromium-based browser process (None if not tracked).
    pub browser_pid: Option<u32>,
    /// Browser type identifier: "chrome", "dia", "chromium", "edge",
    /// "brave", "opera", "vivaldi", "arc", or "unknown".
    pub browser_type: String,
    /// Browser version string from CDP Browser.getVersion product field.
    pub browser_version: String,
    /// CDP debug port.
    pub cdp_port: u16,
    /// How the browser connection was established: "discovered" or "launched".
    pub connection_method: String,
    /// Daemon uptime in seconds.
    pub uptime_secs: u64,
    /// Whether the daemon is connected to Chrome CDP.
    pub chrome_connected: bool,
}
