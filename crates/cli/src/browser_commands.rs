use anyhow::Result;
use common::config::GthingsConfig;
use std::io::Write;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

/// Default daemon socket path
const DAEMON_SOCKET_PATH: &str = "/tmp/gthings-daemon.sock";
static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

async fn send_request(
    method: &str,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value> {
    let socket_path =
        std::env::var("GTHINGS_DAEMON_SOCKET").unwrap_or_else(|_| DAEMON_SOCKET_PATH.to_string());

    let stream = UnixStream::connect(&socket_path)
        .await
        .map_err(|e| anyhow::anyhow!("Cannot connect to daemon at {}: {}", socket_path, e))?;

    let (reader, mut writer) = stream.into_split();

    let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let request = serde_json::json!({
        "id": id,
        "method": method,
        "params": params,
    });

    // Write NDJSON request
    let mut buf = serde_json::to_vec(&request)?;
    buf.push(b'\n');
    writer.write_all(&buf).await?;
    writer.shutdown().await?; // half-close for reading

    // Read NDJSON response
    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    reader.read_line(&mut line).await?;

    let response: serde_json::Value = serde_json::from_str(&line)?;
    if response["ok"].as_bool().unwrap_or(false) {
        Ok(response["result"].clone())
    } else {
        Err(anyhow::anyhow!(
            "Daemon error: {}",
            response["error"].as_str().unwrap_or("unknown")
        ))
    }
}

pub async fn handle_browser_status(_config: &GthingsConfig) -> Result<()> {
    let result = send_request("status", None).await?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

pub async fn handle_browser_start(config: &GthingsConfig, port: Option<u16>) -> Result<()> {
    // Launch the daemon as a child process
    let daemon_binary = std::env::current_exe()?
        .parent()
        .map(|p| p.join("browser-daemon"))
        .unwrap_or_else(|| std::path::PathBuf::from("browser-daemon"));

    let actual_port = port.unwrap_or(config.cdp_port);

    // Spawn: browser-daemon start --port {port}
    let child = tokio::process::Command::new(&daemon_binary)
        .arg("start")
        .arg("--port")
        .arg(actual_port.to_string())
        .spawn()
        .map_err(|e| anyhow::anyhow!("Failed to start daemon: {}", e))?;

    // Wait briefly for it to be ready
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    println!(
        "{{\"ok\":true,\"port\":{},\"pid\":{}}}",
        actual_port,
        child.id().unwrap_or(0)
    );
    Ok(())
}

pub async fn handle_browser_stop(_config: &GthingsConfig) -> Result<()> {
    // Send status check first to get PID
    let result = send_request("status", None).await?;
    if let Some(pid) = result["pid"].as_u64() {
        // Kill daemon process
        let _ = tokio::process::Command::new("kill")
            .arg(pid.to_string())
            .output()
            .await;
        println!("{{\"ok\":true}}");
    } else {
        println!("{{\"ok\":false,\"error\":\"Daemon not running\"}}");
    }
    Ok(())
}

pub async fn handle_browser_logs(_config: &GthingsConfig, _follow: bool) -> Result<()> {
    // Read from /tmp/gthings-daemon.log
    let log_path = std::path::Path::new("/tmp/gthings-daemon.log");
    let content = tokio::fs::read_to_string(log_path).await?;
    // Print last 50 lines
    let lines: Vec<&str> = content.lines().collect();
    let start = lines.len().saturating_sub(50);
    for line in &lines[start..] {
        println!("{}", line);
    }
    Ok(())
}

pub async fn handle_browser_call(
    _config: &GthingsConfig,
    method: &str,
    params: &str,
) -> Result<()> {
    let params: serde_json::Value =
        serde_json::from_str(params).map_err(|e| anyhow::anyhow!("Invalid params JSON: {}", e))?;
    let call_params = serde_json::json!({
        "method": method,
        "params": params,
    });
    let result = send_request("call", Some(call_params)).await?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

pub async fn handle_browser_eval(_config: &GthingsConfig, expression: &str) -> Result<()> {
    let params = serde_json::json!({
        "expression": expression,
        "returnByValue": true,
    });
    let result = send_request("eval", Some(params)).await?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

pub async fn handle_browser_navigate(_config: &GthingsConfig, url: &str) -> Result<()> {
    let params = serde_json::json!({ "url": url });
    let result = send_request("navigate", Some(params)).await?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

pub async fn handle_browser_wait(
    _config: &GthingsConfig,
    method: &str,
    session: &str,
    timeout: u64,
) -> Result<()> {
    let params = serde_json::json!({
        "method": method,
        "session_id": session,
        "timeout_ms": timeout,
    });
    let result = send_request("wait", Some(params)).await?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

pub async fn handle_screenshot(
    _config: &GthingsConfig,
    url: &str,
    output: &std::path::Path,
    json: bool,
) -> Result<()> {
    let result = send_request("screenshot", Some(serde_json::json!({"url": url}))).await?;
    let base64_data = result["data"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("No screenshot data in response"))?;

    if json {
        let response =
            serde_json::json!({"data": base64_data, "format": "png", "size": base64_data.len()});
        println!("{}", serde_json::to_string(&response)?);
        return Ok(());
    }

    // Decode base64 and write to file
    let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, base64_data)
        .map_err(|e| anyhow::anyhow!("Failed to decode PNG: {}", e))?;

    let mut file = std::fs::File::create(output)
        .map_err(|e| anyhow::anyhow!("Failed to create output file: {}", e))?;
    file.write_all(&bytes)
        .map_err(|e| anyhow::anyhow!("Failed to write PNG: {}", e))?;

    println!("Screenshot saved to: {}", output.display());
    Ok(())
}

pub async fn handle_scrape(
    _config: &GthingsConfig,
    url: &str,
    selector: &str,
    attribute: Option<&str>,
    json_mode: bool,
) -> Result<()> {
    let mut params = serde_json::json!({
        "url": url,
        "selector": selector,
    });
    if let Some(attr) = attribute {
        params["attribute"] = serde_json::Value::String(attr.to_string());
    }

    let result = send_request("scrape", Some(params)).await?;
    let items: Vec<String> = serde_json::from_value(result["items"].clone())
        .map_err(|e| anyhow::anyhow!("Failed to parse scrape results: {}", e))?;

    if json_mode {
        println!("{}", serde_json::to_string_pretty(&items)?);
    } else {
        for item in &items {
            println!("{}", item);
        }
    }

    Ok(())
}
