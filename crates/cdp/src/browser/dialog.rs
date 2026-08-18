#[cfg(target_os = "macos")]
use std::time::Duration;

/// Browser process names whose "Allow remote debugging?" sheet may need
/// dismissing, in the order they are probed.
#[cfg(target_os = "macos")]
const PROCESSES: &[&str] = &[
    "Dia",
    "Chromium",
    "Google Chrome",
    "Google Chrome Canary",
    "Microsoft Edge",
    "Microsoft Edge Canary",
    "Brave Browser",
    "Arc",
    "Vivaldi",
    "Opera",
];

/// Build the AppleScript that checks each known browser process for a sheet
/// dialog (attached to window 1) and clicks the "Allow" button if found.
/// Returns the browser name if dismissed, or empty string otherwise. Each
/// `exists` check is wrapped in `try` so that missing processes don't abort
/// the script.
#[cfg(target_os = "macos")]
fn build_dismiss_script() -> String {
    let mut script =
        String::from("tell application \"System Events\"\n    set browserName to \"\"\n");
    for p in PROCESSES {
        script.push_str(&format!(
            "    try\n        if browserName is \"\" and exists (sheet 1 of window 1 of process \"{p}\") then\n            tell process \"{p}\" to click button \"Allow\" of sheet 1 of window 1\n            set browserName to \"{p}\"\n        end if\n    end try\n"
        ));
    }
    script.push_str("    return browserName\nend tell");
    script
}

/// Dismiss the macOS "Allow remote debugging connection?" dialog that appears
/// as a **sheet** in Dia and other Chromium-based browsers when a CDP
/// connection is first attempted.
///
/// Uses `osascript`/System Events to detect the dialog sheet and click the
/// "Allow" button, with a single 1s `osascript` timeout. Logs a warning if
/// the dialog is never found — the WebSocket handshake may still proceed.
#[cfg(target_os = "macos")]
pub(crate) async fn dismiss_allow_debugging_dialog() {
    let script = build_dismiss_script();

    let result = tokio::time::timeout(Duration::from_secs(1), async {
        tokio::process::Command::new("osascript")
            .args(["-e", &script])
            .output()
            .await
    })
    .await;

    match result {
        Ok(Ok(out)) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let browser = stdout.trim();
            if !browser.is_empty() {
                tracing::warn!("Dismissed remote debugging dialog for {browser}");
                return;
            }
        }
        Ok(Err(e)) => {
            tracing::warn!("osascript command failed: {e}");
        }
        Err(_) => {
            tracing::warn!("osascript timed out after 1s");
        }
    }

    tracing::warn!("Remote debugging dialog not found — continuing");
}
