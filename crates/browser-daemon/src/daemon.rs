use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use cdp_core::CdpConnection;
use common::GthingsError;

use crate::chrome::ChromeInstance;
use crate::ipc::{DaemonRequest, DaemonResponse};

#[cfg(target_os = "macos")]
use std::process::Command as StdCommand;

/// Dismiss Dia Browser's "Allow debugging connection?" dialog via osascript.
/// This must be called ~600ms after WebSocket connection attempt starts.
#[cfg(target_os = "macos")]
fn dismiss_dia_allow_dialog() {
    // Bring Dia to front then press Return (which clicks "Allow")
    let _ = StdCommand::new("osascript")
        .args([
            "-e",
            r#"tell application "System Events" to set frontmost of process "Dia" to true"#,
        ])
        .output();
    let _ = StdCommand::new("osascript")
        .args([
            "-e",
            r#"tell application "System Events" to tell process "Dia" to keystroke return"#,
        ])
        .output();
}

#[cfg(not(target_os = "macos"))]
fn dismiss_dia_allow_dialog() {}

/// Extension trait to convert CDP errors into our domain error type.
trait CdpResultExt<T> {
    fn gthings(self) -> Result<T, GthingsError>;
}

impl<T> CdpResultExt<T> for Result<T, cdp_core::CdpError> {
    fn gthings(self) -> Result<T, GthingsError> {
        self.map_err(|e| GthingsError::Other(e.to_string()))
    }
}

// ── JS code constants (ported from templates.ts) ────────────────────────────

/// DOM extraction JS — returns JSON: {content, total_length, returned_length,
/// offset, truncated, sections}
const EXTRACTION_JS: &str = r#"(offset, maxLen, sel) => {
    const root = document.querySelector(sel) || document.querySelector('article,main,[role=main]') || document.body;
    if (!root) return JSON.stringify({content:'',total_length:0,returned_length:0,offset,truncated:false,sections:[]});
    const text = root.innerText || '';
    const sliced = text.slice(offset, offset + maxLen);
    const sections = [];
    const headings = document.querySelectorAll('h1,h2,h3');
    for (var i=0; i<headings.length; i++) {
        let h = headings[i];
        let sectionText = '';
        let el = h.nextElementSibling;
        while (el && !/^H[1-3]$/i.test(el.tagName)) {
            sectionText += (el.innerText || el.textContent || '') + '\n';
            el = el.nextElementSibling;
        }
        sections.push({heading: h.innerText.trim(), content: sectionText.trim()});
    }
    return JSON.stringify({content:sliced,total_length:text.length,returned_length:sliced.length,offset,truncated:sliced.length < text.length - offset,sections});
}"#;

/// Google SERP extraction JS — returns JSON array: [{title, url, snippet}]
const SEARCH_JS: &str = r#"(count) => {
    const items = Array.from(document.querySelectorAll('a[href]'))
        .filter(a => {
            const href = a.href || '';
            try {
                const u = new URL(href);
                return u.hostname !== location.hostname && u.pathname !== '/search' && a.innerText.trim().length > 0;
            } catch(e) { return false; }
        })
        .slice(0, count)
        .map(a => ({title: a.innerText.trim(), url: a.href, snippet: ''}));
    return JSON.stringify(items);
}"#;

/// Configuration for the CDP daemon.
#[derive(Debug, Clone)]
pub struct DaemonConfig {
    pub cdp_port: u16,
    pub chrome_path: Option<PathBuf>,
    pub profile_dir: Option<PathBuf>,
    pub socket_path: PathBuf,
    pub pid_path: PathBuf,
    pub log_path: PathBuf,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            cdp_port: 9222,
            chrome_path: None,
            profile_dir: None,
            socket_path: PathBuf::from("/tmp/gthings-daemon.sock"),
            pid_path: PathBuf::from("/tmp/gthings-daemon.pid"),
            log_path: PathBuf::from("/tmp/gthings-daemon.log"),
        }
    }
}

/// Core daemon that orchestrates Chrome discovery, CDP communication, and
/// the UDS server for CLI interaction.
pub struct CdpDaemon {
    pub config: DaemonConfig,
    pub conn: Option<Arc<CdpConnection>>,
    pub browser: Option<cdp_core::Browser>,
    pub started_at: Option<std::time::Instant>,
    pub chrome_pid: Option<u32>,
}

impl CdpDaemon {
    pub fn new(config: DaemonConfig) -> Self {
        Self {
            config,
            conn: None,
            browser: None,
            started_at: None,
            chrome_pid: None,
        }
    }

    /// Main run loop.
    ///
    /// 1. Writes the PID file.
    /// 2. Discovers or launches Chrome.
    /// 3. Creates the CDP connection.
    /// 4. Starts the UDS server.
    /// 5. Waits for SIGTERM / SIGINT.
    /// 6. Performs graceful shutdown.
    pub async fn run(mut self) -> Result<(), GthingsError> {
        // ── PID file ──────────────────────────────────────────────────
        self.write_pid()?;

        // ── Discover or launch Chrome ─────────────────────────────────
        let ws_url = match ChromeInstance::discover(self.config.cdp_port).await {
            Ok(url) => {
                tracing::info!(
                    "Discovered existing Chrome on port {}",
                    self.config.cdp_port
                );
                url
            }
            Err(_) => {
                tracing::info!(
                    "No Chrome found on port {}, launching…",
                    self.config.cdp_port
                );
                let (mut child, url) = ChromeInstance::launch(
                    self.config.cdp_port,
                    self.config.chrome_path.as_deref(),
                    self.config.profile_dir.as_deref(),
                )
                .await?;

                let chrome_pid = child
                    .id()
                    .ok_or_else(|| GthingsError::Other("no child PID".into()))?;
                tracing::info!("Chrome launched (PID {chrome_pid}, WS {url})");

                // Reap the child in the background so we don't orphan it.
                tokio::spawn(async move {
                    let _ = child.wait().await;
                });

                self.chrome_pid = Some(chrome_pid);
                url
            }
        };

        // ── Auto-allow dialog handler (Dia Browser on macOS) ─────────
        // On macOS, Dia shows "Allow debugging connection?" dialog when
        // a WebSocket connects. We dismiss it via osascript after 600ms.
        let cancel_allow = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let cancel_clone = cancel_allow.clone();

        #[cfg(target_os = "macos")]
        {
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(600)).await;
                if !cancel_clone.load(std::sync::atomic::Ordering::Relaxed) {
                    dismiss_dia_allow_dialog();
                }
            });
        }

        // ── Connect to CDP ───────────────────────────────────────────────
        let conn = CdpConnection::connect(&ws_url)
            .await
            .map_err(|e| GthingsError::Other(format!("Failed to connect: {e}")))?;

        // Connection succeeded — cancel the auto-allow timer
        cancel_allow.store(true, std::sync::atomic::Ordering::Relaxed);

        self.conn = Some(conn);
        self.started_at = Some(std::time::Instant::now());

        // ── Start UDS server ──────────────────────────────────────────
        let daemon = Arc::new(self);
        let server =
            crate::server::DaemonServer::new(daemon.config.socket_path.clone(), daemon.clone());

        let server_handle = tokio::spawn(async move {
            if let Err(e) = server.run().await {
                tracing::error!("Daemon server exited with error: {e}");
            }
        });

        // ── Signal handling ───────────────────────────────────────────
        let mut sigterm =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;

        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("Received SIGINT, shutting down…");
            }
            _ = sigterm.recv() => {
                tracing::info!("Received SIGTERM, shutting down…");
            }
        }

        // ── Graceful shutdown ─────────────────────────────────────────
        daemon.shutdown().await?;
        let _ = server_handle.await;

        tracing::info!("Daemon stopped");
        Ok(())
    }

    // ── Request dispatching ────────────────────────────────────────────

    /// Dispatch a single [`DaemonRequest`] to the appropriate handler.
    pub async fn handle_request(&self, req: DaemonRequest) -> DaemonResponse {
        match req.method.as_str() {
            "status" => self.handle_status(req.id).await,
            "call" => self.handle_call(req.id, req.params).await,
            "eval" => self.handle_eval(req.id, req.params).await,
            "navigate" => self.handle_navigate(req.id, req.params).await,
            "wait" => self.handle_wait(req.id, req.params).await,
            "list_targets" => self.handle_list_targets(req.id).await,
            "create_tab" => self.handle_create_tab(req.id, req.params).await,
            "close_tab" => self.handle_close_tab(req.id, req.params).await,
            "search" => self.handle_search_exec(req.id, req.params).await,
            "follow" => self.handle_follow_exec(req.id, req.params).await,
            "screenshot" => self.handle_screenshot_exec(req.id, req.params).await,
            "scrape" => self.handle_scrape_exec(req.id, req.params).await,
            _ => DaemonResponse {
                id: req.id,
                ok: false,
                result: None,
                error: Some(format!("Unknown method: {}", req.method)),
            },
        }
    }

    // ── Handler implementations ────────────────────────────────────────

    async fn handle_status(&self, id: u64) -> DaemonResponse {
        let running = self.conn.is_some();
        let pid = std::process::id();
        let cdp_port = Some(self.config.cdp_port);
        let chrome_connected = self.conn.is_some();
        let uptime_secs = self.started_at.map(|t| t.elapsed().as_secs());

        let version = if let Some(ref conn) = self.conn {
            match conn.call("Browser.getVersion", None).await {
                Ok(v) => v
                    .get("product")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                Err(_) => None,
            }
        } else {
            None
        };

        DaemonResponse {
            id,
            ok: true,
            result: Some(serde_json::json!({
                "running": running,
                "pid": pid,
                "cdp_port": cdp_port,
                "chrome_connected": chrome_connected,
                "uptime_secs": uptime_secs,
                "version": version,
            })),
            error: None,
        }
    }

    async fn handle_call(&self, id: u64, params: Option<serde_json::Value>) -> DaemonResponse {
        let p = match params {
            Some(v) => v,
            None => {
                return DaemonResponse {
                    id,
                    ok: false,
                    result: None,
                    error: Some("Missing params".into()),
                };
            }
        };

        let method = match p["method"].as_str() {
            Some(m) => m.to_string(),
            None => {
                return DaemonResponse {
                    id,
                    ok: false,
                    result: None,
                    error: Some("Missing 'method' in params".into()),
                };
            }
        };

        let call_params = p.get("params").cloned();
        let session_id = p["session_id"].as_str().map(|s| s.to_string());

        let conn = match &self.conn {
            Some(c) => c.clone(),
            None => {
                return DaemonResponse {
                    id,
                    ok: false,
                    result: None,
                    error: Some("Not connected to Chrome".into()),
                };
            }
        };

        let result = if let Some(ref sid) = session_id {
            conn.call_with_session(sid, &method, call_params).await
        } else {
            conn.call(&method, call_params).await
        };

        match result {
            Ok(value) => DaemonResponse {
                id,
                ok: true,
                result: Some(value),
                error: None,
            },
            Err(e) => DaemonResponse {
                id,
                ok: false,
                result: None,
                error: Some(e.to_string()),
            },
        }
    }

    async fn handle_eval(&self, id: u64, params: Option<serde_json::Value>) -> DaemonResponse {
        let p = match params {
            Some(v) => v,
            None => {
                return DaemonResponse {
                    id,
                    ok: false,
                    result: None,
                    error: Some("Missing params".into()),
                };
            }
        };

        let expression = match p["expression"].as_str() {
            Some(e) => e.to_string(),
            None => {
                return DaemonResponse {
                    id,
                    ok: false,
                    result: None,
                    error: Some("Missing 'expression' in params".into()),
                };
            }
        };

        let return_by_value = p["return_by_value"].as_bool().unwrap_or(true);

        let conn = match &self.conn {
            Some(c) => c.clone(),
            None => {
                return DaemonResponse {
                    id,
                    ok: false,
                    result: None,
                    error: Some("Not connected to Chrome".into()),
                };
            }
        };

        let (session_id, created_target_id) = match p["session_id"].as_str() {
            Some(s) => (s.to_string(), None),
            None => {
                // Auto-create a tab so eval works without a pre-existing session
                let create_result = match conn
                    .call(
                        "Target.createTarget",
                        Some(serde_json::json!({"url": "about:blank"})),
                    )
                    .await
                {
                    Ok(v) => v,
                    Err(e) => {
                        return DaemonResponse {
                            id,
                            ok: false,
                            result: None,
                            error: Some(format!("Failed to create target: {e}")),
                        };
                    }
                };
                let target_id = match create_result["targetId"].as_str() {
                    Some(id) => id.to_string(),
                    None => {
                        return DaemonResponse {
                            id,
                            ok: false,
                            result: None,
                            error: Some("No targetId in create response".into()),
                        };
                    }
                };
                let attach_result = match conn
                    .call(
                        "Target.attachToTarget",
                        Some(serde_json::json!({
                            "targetId": target_id,
                            "flatten": true,
                        })),
                    )
                    .await
                {
                    Ok(v) => v,
                    Err(e) => {
                        return DaemonResponse {
                            id,
                            ok: false,
                            result: None,
                            error: Some(format!("Failed to attach to target: {e}")),
                        };
                    }
                };
                let sid = match attach_result["sessionId"].as_str() {
                    Some(id) => id.to_string(),
                    None => {
                        return DaemonResponse {
                            id,
                            ok: false,
                            result: None,
                            error: Some("No sessionId in attach response".into()),
                        };
                    }
                };
                (sid, Some(target_id))
            }
        };

        let eval_params = serde_json::json!({
            "expression": expression,
            "returnByValue": return_by_value,
        });

        let result = conn
            .call_with_session(&session_id, "Runtime.evaluate", Some(eval_params))
            .await;

        // If we created the tab, close it
        if let Some(target_id) = created_target_id {
            let _ = conn
                .call(
                    "Target.closeTarget",
                    Some(serde_json::json!({"targetId": target_id})),
                )
                .await;
        }

        match result {
            Ok(value) => DaemonResponse {
                id,
                ok: true,
                result: Some(value),
                error: None,
            },
            Err(e) => DaemonResponse {
                id,
                ok: false,
                result: None,
                error: Some(e.to_string()),
            },
        }
    }

    async fn handle_navigate(&self, id: u64, params: Option<serde_json::Value>) -> DaemonResponse {
        let p = match params {
            Some(v) => v,
            None => {
                return DaemonResponse {
                    id,
                    ok: false,
                    result: None,
                    error: Some("Missing params".into()),
                };
            }
        };

        let url = match p["url"].as_str() {
            Some(u) => u.to_string(),
            None => {
                return DaemonResponse {
                    id,
                    ok: false,
                    result: None,
                    error: Some("Missing 'url' in params".into()),
                };
            }
        };

        let session_id = match p["session_id"].as_str() {
            Some(s) => s.to_string(),
            None => {
                return DaemonResponse {
                    id,
                    ok: false,
                    result: None,
                    error: Some("Missing 'session_id' in params".into()),
                };
            }
        };

        let conn = match &self.conn {
            Some(c) => c.clone(),
            None => {
                return DaemonResponse {
                    id,
                    ok: false,
                    result: None,
                    error: Some("Not connected to Chrome".into()),
                };
            }
        };

        let nav_params = serde_json::json!({ "url": url });

        match conn
            .call_with_session(&session_id, "Page.navigate", Some(nav_params))
            .await
        {
            Ok(value) => DaemonResponse {
                id,
                ok: true,
                result: Some(value),
                error: None,
            },
            Err(e) => DaemonResponse {
                id,
                ok: false,
                result: None,
                error: Some(e.to_string()),
            },
        }
    }

    async fn handle_wait(&self, id: u64, params: Option<serde_json::Value>) -> DaemonResponse {
        let p = match params {
            Some(v) => v,
            None => {
                return DaemonResponse {
                    id,
                    ok: false,
                    result: None,
                    error: Some("Missing params".into()),
                };
            }
        };

        let method = match p["method"].as_str() {
            Some(m) => m.to_string(),
            None => {
                return DaemonResponse {
                    id,
                    ok: false,
                    result: None,
                    error: Some("Missing 'method' in params".into()),
                };
            }
        };

        let session_id = p["session_id"].as_str().map(|s| s.to_string());
        let timeout_ms = p["timeout_ms"].as_u64().unwrap_or(30_000);

        let conn = match &self.conn {
            Some(c) => c.clone(),
            None => {
                return DaemonResponse {
                    id,
                    ok: false,
                    result: None,
                    error: Some("Not connected to Chrome".into()),
                };
            }
        };

        let mut rx = conn.subscribe();
        let deadline = tokio::time::sleep(Duration::from_millis(timeout_ms));
        tokio::pin!(deadline);

        loop {
            tokio::select! {
                event = rx.recv() => {
                    match event {
                        Ok(evt) => {
                            let session_matches = match &session_id {
                                Some(sid) => evt.session_id.as_deref() == Some(sid.as_str()),
                                None => true,
                            };
                            if evt.method == method && session_matches {
                                return DaemonResponse {
                                    id,
                                    ok: true,
                                    result: Some(serde_json::json!({
                                        "method": evt.method,
                                        "params": evt.params,
                                        "session_id": evt.session_id,
                                    })),
                                    error: None,
                                };
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!("Event channel lagged by {n} messages");
                            continue;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            return DaemonResponse {
                                id,
                                ok: false,
                                result: None,
                                error: Some("CDP connection closed".into()),
                            };
                        }
                    }
                }
                _ = &mut deadline => {
                    return DaemonResponse {
                        id,
                        ok: false,
                        result: None,
                        error: Some("Timeout waiting for event".into()),
                    };
                }
            }
        }
    }

    async fn handle_list_targets(&self, id: u64) -> DaemonResponse {
        let conn = match &self.conn {
            Some(c) => c.clone(),
            None => {
                return DaemonResponse {
                    id,
                    ok: false,
                    result: None,
                    error: Some("Not connected to Chrome".into()),
                };
            }
        };

        match conn.call("Target.getTargets", None).await {
            Ok(value) => {
                let targets = value.get("targetInfos").cloned().unwrap_or(value);
                DaemonResponse {
                    id,
                    ok: true,
                    result: Some(targets),
                    error: None,
                }
            }
            Err(e) => DaemonResponse {
                id,
                ok: false,
                result: None,
                error: Some(e.to_string()),
            },
        }
    }

    async fn handle_create_tab(
        &self,
        id: u64,
        _params: Option<serde_json::Value>,
    ) -> DaemonResponse {
        let conn = match &self.conn {
            Some(c) => c.clone(),
            None => {
                return DaemonResponse {
                    id,
                    ok: false,
                    result: None,
                    error: Some("Not connected to Chrome".into()),
                };
            }
        };

        // Create a new page target.
        let create_result = match conn
            .call(
                "Target.createTarget",
                Some(serde_json::json!({ "url": "about:blank" })),
            )
            .await
        {
            Ok(v) => v,
            Err(e) => {
                return DaemonResponse {
                    id,
                    ok: false,
                    result: None,
                    error: Some(format!("Failed to create target: {e}")),
                };
            }
        };

        let target_id = match create_result["targetId"].as_str() {
            Some(id) => id.to_string(),
            None => {
                return DaemonResponse {
                    id,
                    ok: false,
                    result: None,
                    error: Some("No targetId in create response".into()),
                };
            }
        };

        // Attach a flattened session to the new target.
        let attach_result = match conn
            .call(
                "Target.attachToTarget",
                Some(serde_json::json!({
                    "targetId": target_id,
                    "flatten": true,
                })),
            )
            .await
        {
            Ok(v) => v,
            Err(e) => {
                return DaemonResponse {
                    id,
                    ok: false,
                    result: None,
                    error: Some(format!("Failed to attach to target: {e}")),
                };
            }
        };

        let session_id = match attach_result["sessionId"].as_str() {
            Some(id) => id.to_string(),
            None => {
                return DaemonResponse {
                    id,
                    ok: false,
                    result: None,
                    error: Some("No sessionId in attach response".into()),
                };
            }
        };

        DaemonResponse {
            id,
            ok: true,
            result: Some(serde_json::json!({
                "targetId": target_id,
                "sessionId": session_id,
            })),
            error: None,
        }
    }

    async fn handle_close_tab(&self, id: u64, params: Option<serde_json::Value>) -> DaemonResponse {
        let p = match params {
            Some(v) => v,
            None => {
                return DaemonResponse {
                    id,
                    ok: false,
                    result: None,
                    error: Some("Missing params".into()),
                };
            }
        };

        let target_id = match p["targetId"].as_str() {
            Some(id) => id.to_string(),
            None => {
                return DaemonResponse {
                    id,
                    ok: false,
                    result: None,
                    error: Some("Missing 'targetId' in params".into()),
                };
            }
        };

        let conn = match &self.conn {
            Some(c) => c.clone(),
            None => {
                return DaemonResponse {
                    id,
                    ok: false,
                    result: None,
                    error: Some("Not connected to Chrome".into()),
                };
            }
        };

        match conn
            .call(
                "Target.closeTarget",
                Some(serde_json::json!({ "targetId": target_id })),
            )
            .await
        {
            Ok(_) => DaemonResponse {
                id,
                ok: true,
                result: None,
                error: None,
            },
            Err(e) => DaemonResponse {
                id,
                ok: false,
                result: None,
                error: Some(e.to_string()),
            },
        }
    }

    // ── High-level CDP operation handlers ───────────────────────────────

    /// Execute a Google search via CDP.
    /// 1. Create a tab via Target.createTarget
    /// 2. Navigate to Google search URL
    /// 3. Wait for page load
    /// 4. Extract results via Runtime.evaluate with SEARCH_JS
    /// 5. Close the tab
    /// 6. Return parsed results
    async fn execute_search(
        &self,
        query: &str,
        count: usize,
    ) -> Result<Vec<serde_json::Value>, GthingsError> {
        let conn = self
            .conn
            .as_ref()
            .ok_or_else(|| GthingsError::Other("CDP not connected".into()))?;

        // 1. Create tab
        let create_result = conn
            .call(
                "Target.createTarget",
                Some(serde_json::json!({"url": "about:blank"})),
            )
            .await
            .gthings()?;
        let target_id = create_result["targetId"]
            .as_str()
            .ok_or_else(|| GthingsError::Other("No targetId".into()))?
            .to_string();

        // 2. Attach to tab (flattened session)
        let attach_result = conn
            .call(
                "Target.attachToTarget",
                Some(serde_json::json!({"targetId": target_id, "flatten": true})),
            )
            .await
            .gthings()?;
        let session_id = attach_result["sessionId"]
            .as_str()
            .ok_or_else(|| GthingsError::Other("No sessionId".into()))?
            .to_string();

        // 3. Navigate to Google
        let search_url = format!(
            "https://www.google.com/search?q={}&num={}&hl=en",
            urlencoding::encode(query),
            count.min(100)
        );
        conn.call_with_session(&session_id, "Page.enable", None)
            .await
            .ok();
        conn.call_with_session(
            &session_id,
            "Page.navigate",
            Some(serde_json::json!({"url": search_url})),
        )
        .await
        .gthings()?;

        // 4. Wait for load event (poll readyState)
        for _ in 0..30 {
            tokio::time::sleep(Duration::from_millis(500)).await;
            let ready = conn
                .call_with_session(
                    &session_id,
                    "Runtime.evaluate",
                    Some(serde_json::json!({
                        "expression": "document.readyState",
                        "returnByValue": true
                    })),
                )
                .await
                .gthings()?;
            if ready["result"]["value"].as_str() == Some("complete") {
                break;
            }
        }

        // 5. Extract results
        let js = format!("({})({})", SEARCH_JS, count);
        let eval_result = conn
            .call_with_session(
                &session_id,
                "Runtime.evaluate",
                Some(serde_json::json!({
                    "expression": js,
                    "returnByValue": true,
                    "awaitPromise": true
                })),
            )
            .await
            .gthings()?;

        // 6. Parse results
        let items_str = eval_result["result"]["value"].as_str().unwrap_or("[]");
        let mut items: Vec<serde_json::Value> = serde_json::from_str(items_str).unwrap_or_default();

        // 6b. Retry once with trailing space if empty (bypass Google cache)
        if items.is_empty() {
            tracing::debug!("search: empty results, retrying with trailing space");
            let retry_url = format!(
                "https://www.google.com/search?q={}&num={}&hl=en",
                urlencoding::encode(&format!("{} ", query)),
                count.min(100)
            );
            conn.call_with_session(
                &session_id,
                "Page.navigate",
                Some(serde_json::json!({"url": retry_url})),
            )
            .await
            .gthings()?;

            for _ in 0..30 {
                tokio::time::sleep(Duration::from_millis(500)).await;
                let ready = conn.call_with_session(&session_id, "Runtime.evaluate",
                    Some(serde_json::json!({"expression": "document.readyState", "returnByValue": true})),
                ).await.gthings()?;
                if ready["result"]["value"].as_str() == Some("complete") {
                    break;
                }
            }

            let retry_js = format!("({})({})", SEARCH_JS, count);
            let retry_eval = conn.call_with_session(&session_id, "Runtime.evaluate",
                Some(serde_json::json!({"expression": retry_js, "returnByValue": true, "awaitPromise": true})),
            ).await.gthings()?;
            let retry_str = retry_eval["result"]["value"].as_str().unwrap_or("[]");
            items = serde_json::from_str(retry_str).unwrap_or_default();
        }

        // 7. Close tab (Dia quirk: window.close() before Target.closeTarget)
        close_tab(conn, &session_id, &target_id).await;

        Ok(items)
    }

    /// Follow/extract page content via CDP.
    async fn execute_follow(
        &self,
        url: &str,
        selector: &str,
        offset: usize,
        max_length: usize,
    ) -> Result<serde_json::Value, GthingsError> {
        let conn = self
            .conn
            .as_ref()
            .ok_or_else(|| GthingsError::Other("CDP not connected".into()))?;

        // Create tab
        let create_result = conn
            .call(
                "Target.createTarget",
                Some(serde_json::json!({"url": "about:blank"})),
            )
            .await
            .gthings()?;
        let target_id = create_result["targetId"]
            .as_str()
            .ok_or_else(|| GthingsError::Other("No targetId".into()))?
            .to_string();

        let attach_result = conn
            .call(
                "Target.attachToTarget",
                Some(serde_json::json!({"targetId": target_id, "flatten": true})),
            )
            .await
            .gthings()?;
        let session_id = attach_result["sessionId"]
            .as_str()
            .ok_or_else(|| GthingsError::Other("No sessionId".into()))?
            .to_string();

        // Navigate
        conn.call_with_session(&session_id, "Page.enable", None)
            .await
            .ok();
        conn.call_with_session(
            &session_id,
            "Page.navigate",
            Some(serde_json::json!({"url": url})),
        )
        .await
        .gthings()?;

        // Wait for load
        for _ in 0..60 {
            tokio::time::sleep(Duration::from_millis(500)).await;
            let ready = conn
                .call_with_session(
                    &session_id,
                    "Runtime.evaluate",
                    Some(serde_json::json!({
                        "expression": "document.readyState",
                        "returnByValue": true
                    })),
                )
                .await
                .gthings()?;
            if ready["result"]["value"].as_str() == Some("complete") {
                break;
            }
        }

        // Extract content
        let js = format!(
            "({})({},{},'{}')",
            EXTRACTION_JS, offset, max_length, selector
        );
        let eval_result = conn
            .call_with_session(
                &session_id,
                "Runtime.evaluate",
                Some(serde_json::json!({
                    "expression": js,
                    "returnByValue": true,
                    "awaitPromise": true
                })),
            )
            .await
            .gthings()?;

        let content_str = eval_result["result"]["value"].as_str().unwrap_or("{}");
        let content: serde_json::Value = serde_json::from_str(content_str).unwrap_or_default();

        // Close tab (Dia quirk: window.close() before Target.closeTarget)
        close_tab(conn, &session_id, &target_id).await;

        Ok(content)
    }

    /// Take a screenshot via CDP Page.captureScreenshot.
    async fn execute_screenshot(&self, url: &str) -> Result<String, GthingsError> {
        let conn = self
            .conn
            .as_ref()
            .ok_or_else(|| GthingsError::Other("CDP not connected".into()))?;

        let create_result = conn
            .call(
                "Target.createTarget",
                Some(serde_json::json!({"url": "about:blank"})),
            )
            .await
            .gthings()?;
        let target_id = create_result["targetId"]
            .as_str()
            .ok_or_else(|| GthingsError::Other("No targetId".into()))?
            .to_string();

        let attach_result = conn
            .call(
                "Target.attachToTarget",
                Some(serde_json::json!({"targetId": target_id, "flatten": true})),
            )
            .await
            .gthings()?;
        let session_id = attach_result["sessionId"]
            .as_str()
            .ok_or_else(|| GthingsError::Other("No sessionId".into()))?
            .to_string();

        conn.call_with_session(&session_id, "Page.enable", None)
            .await
            .ok();
        conn.call_with_session(
            &session_id,
            "Page.navigate",
            Some(serde_json::json!({"url": url})),
        )
        .await
        .gthings()?;

        // Wait
        for _ in 0..30 {
            tokio::time::sleep(Duration::from_millis(500)).await;
            let ready = conn
                .call_with_session(
                    &session_id,
                    "Runtime.evaluate",
                    Some(serde_json::json!({
                        "expression": "document.readyState",
                        "returnByValue": true
                    })),
                )
                .await
                .gthings()?;
            if ready["result"]["value"].as_str() == Some("complete") {
                break;
            }
        }

        // Capture screenshot
        let result = conn
            .call_with_session(
                &session_id,
                "Page.captureScreenshot",
                Some(serde_json::json!({
                    "format": "png",
                    "captureBeyondViewport": true
                })),
            )
            .await
            .gthings()?;

        let base64_data = result["data"].as_str().unwrap_or("").to_string();

        // Close tab (Dia quirk: window.close() before Target.closeTarget)
        close_tab(conn, &session_id, &target_id).await;

        Ok(base64_data)
    }

    /// Scrape content via CSS selector using CDP Runtime.evaluate.
    async fn execute_scrape(
        &self,
        url: &str,
        selector: &str,
        attribute: Option<&str>,
    ) -> Result<Vec<String>, GthingsError> {
        let conn = self
            .conn
            .as_ref()
            .ok_or_else(|| GthingsError::Other("CDP not connected".into()))?;

        let create_result = conn
            .call(
                "Target.createTarget",
                Some(serde_json::json!({"url": "about:blank"})),
            )
            .await
            .gthings()?;
        let target_id = create_result["targetId"]
            .as_str()
            .ok_or_else(|| GthingsError::Other("No targetId".into()))?
            .to_string();

        let attach_result = conn
            .call(
                "Target.attachToTarget",
                Some(serde_json::json!({"targetId": target_id, "flatten": true})),
            )
            .await
            .gthings()?;
        let session_id = attach_result["sessionId"]
            .as_str()
            .ok_or_else(|| GthingsError::Other("No sessionId".into()))?
            .to_string();

        conn.call_with_session(&session_id, "Page.enable", None)
            .await
            .ok();
        conn.call_with_session(
            &session_id,
            "Page.navigate",
            Some(serde_json::json!({"url": url})),
        )
        .await
        .gthings()?;

        for _ in 0..30 {
            tokio::time::sleep(Duration::from_millis(500)).await;
            let ready = conn
                .call_with_session(
                    &session_id,
                    "Runtime.evaluate",
                    Some(serde_json::json!({
                        "expression": "document.readyState",
                        "returnByValue": true
                    })),
                )
                .await
                .gthings()?;
            if ready["result"]["value"].as_str() == Some("complete") {
                break;
            }
        }

        // Scrape with selector
        let js = match attribute {
            Some(attr) => format!(
                "JSON.stringify(Array.from(document.querySelectorAll('{}')).map(el => el.getAttribute('{}') || el.innerText || ''))",
                selector, attr
            ),
            None => format!(
                "JSON.stringify(Array.from(document.querySelectorAll('{}')).map(el => el.innerText || ''))",
                selector
            ),
        };

        let eval_result = conn
            .call_with_session(
                &session_id,
                "Runtime.evaluate",
                Some(serde_json::json!({
                    "expression": js,
                    "returnByValue": true
                })),
            )
            .await
            .gthings()?;

        let items_str = eval_result["result"]["value"].as_str().unwrap_or("[]");
        let items: Vec<String> = serde_json::from_str(items_str).unwrap_or_default();

        // Close tab (Dia quirk: window.close() before Target.closeTarget)
        close_tab(conn, &session_id, &target_id).await;

        Ok(items)
    }

    // ── Handler wrappers for the UDS dispatch ──────────────────────────

    async fn handle_search_exec(
        &self,
        id: u64,
        params: Option<serde_json::Value>,
    ) -> DaemonResponse {
        let params = params.unwrap_or_default();
        let query = match params["query"].as_str() {
            Some(q) if !q.is_empty() => q,
            _ => return error_response(id, "Missing or empty 'query' parameter"),
        };
        let count = params["count"].as_u64().unwrap_or(10) as usize;

        match self.execute_search(query, count).await {
            Ok(items) => success_response(
                id,
                serde_json::json!({"results": items, "total": items.len()}),
            ),
            Err(e) => error_response(id, &e.to_string()),
        }
    }

    async fn handle_follow_exec(
        &self,
        id: u64,
        params: Option<serde_json::Value>,
    ) -> DaemonResponse {
        let params = params.unwrap_or_default();
        let url = match params["url"].as_str() {
            Some(u) if !u.is_empty() => u.to_string(),
            _ => return error_response(id, "Missing or empty 'url' parameter"),
        };
        let selector = params["selector"].as_str().unwrap_or("");
        let offset = params["offset"].as_u64().unwrap_or(0) as usize;
        let max_length = params["max_length"].as_u64().unwrap_or(50_000) as usize;

        match self
            .execute_follow(&url, selector, offset, max_length)
            .await
        {
            Ok(content) => success_response(id, content),
            Err(e) => error_response(id, &e.to_string()),
        }
    }

    async fn handle_screenshot_exec(
        &self,
        id: u64,
        params: Option<serde_json::Value>,
    ) -> DaemonResponse {
        let params = params.unwrap_or_default();
        let url = match params["url"].as_str() {
            Some(u) if !u.is_empty() => u.to_string(),
            _ => return error_response(id, "Missing or empty 'url' parameter"),
        };

        match self.execute_screenshot(&url).await {
            Ok(data) => success_response(id, serde_json::json!({"data": data, "format": "png"})),
            Err(e) => error_response(id, &e.to_string()),
        }
    }

    async fn handle_scrape_exec(
        &self,
        id: u64,
        params: Option<serde_json::Value>,
    ) -> DaemonResponse {
        let params = params.unwrap_or_default();
        let url = match params["url"].as_str() {
            Some(u) if !u.is_empty() => u.to_string(),
            _ => return error_response(id, "Missing or empty 'url' parameter"),
        };
        let selector = match params["selector"].as_str() {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => return error_response(id, "Missing or empty 'selector' parameter"),
        };
        let attribute = params["attribute"].as_str();

        match self.execute_scrape(&url, &selector, attribute).await {
            Ok(items) => success_response(
                id,
                serde_json::json!({"items": items, "total": items.len()}),
            ),
            Err(e) => error_response(id, &e.to_string()),
        }
    }

    // ── Lifecycle helpers ──────────────────────────────────────────────

    /// Write our PID to the PID file atomically (tmp + rename).
    fn write_pid(&self) -> Result<(), GthingsError> {
        let pid = std::process::id();
        let tmp = format!("{}.tmp", self.config.pid_path.display());
        std::fs::write(&tmp, pid.to_string())?;
        std::fs::rename(&tmp, &self.config.pid_path)?;
        Ok(())
    }

    /// Graceful shutdown: close CDP, kill Chrome, clean up files.
    async fn shutdown(&self) -> Result<(), GthingsError> {
        // Close CDP connection.
        if let Some(ref conn) = self.conn {
            let _ = conn.close().await;
        }

        // Terminate Chrome if we launched it.
        if let Some(pid) = self.chrome_pid {
            tracing::info!("Sending SIGTERM to Chrome (PID {pid})");
            let _ = std::process::Command::new("kill")
                .arg(pid.to_string())
                .status();
            // Brief grace period.
            tokio::time::sleep(Duration::from_millis(500)).await;
        }

        // Remove PID file.
        let _ = tokio::fs::remove_file(&self.config.pid_path).await;

        // Remove socket file.
        let _ = tokio::fs::remove_file(&self.config.socket_path).await;

        Ok(())
    }
}

// ── Response helpers ────────────────────────────────────────────────────────

/// Build a success [`DaemonResponse`].
fn success_response(id: u64, result: serde_json::Value) -> DaemonResponse {
    DaemonResponse {
        id,
        ok: true,
        result: Some(result),
        error: None,
    }
}

/// Build an error [`DaemonResponse`].
fn error_response(id: u64, error: &str) -> DaemonResponse {
    DaemonResponse {
        id,
        ok: false,
        result: None,
        error: Some(error.to_string()),
    }
}

// ── Dia Browser tab close helper ─────────────────────────────────────────────

/// Close a tab, handling Dia Browser's tab-close quirk.
/// Dia requires `window.close()` via Runtime.evaluate *before*
/// `Target.closeTarget`, otherwise the tab strip entry remains.
async fn close_tab(conn: &CdpConnection, session_id: &str, target_id: &str) {
    // Dia quirk: window.close() first, then Target.closeTarget
    let _ = conn
        .call_with_session(
            session_id,
            "Runtime.evaluate",
            Some(serde_json::json!({
                "expression": "window.close()",
                "returnByValue": true
            })),
        )
        .await;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let _ = conn
        .call(
            "Target.closeTarget",
            Some(serde_json::json!({"targetId": target_id})),
        )
        .await;
}
