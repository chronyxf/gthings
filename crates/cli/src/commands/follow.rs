//! `gthings follow` — navigate to a URL and extract page content via CDP.

use gthings_common::pagination::ExtractParams;
use gthings_search::follow;

use crate::commands::{connect, on_cdp_error, print_error};

/// Follow: detect → connect → create tab → follow → close tab → disconnect.
pub(crate) async fn cmd_follow(url: &str, max_chars: usize, offset: usize, json: bool) -> i32 {
    let session = match connect().await {
        Ok(s) => s,
        Err(c) => return c,
    };

    let tab = match session.create_tab("about:blank").await {
        Ok(t) => t,
        Err(e) => {
            print_error(
                "TAB_CREATE_FAILED",
                &e.to_string(),
                "Check browser connection",
            );
            if let Err(e) = session.disconnect().await {
                tracing::warn!("disconnect failed: {e}");
            }
            return 1;
        }
    };

    let params = ExtractParams { offset, max_chars };
    let result = match follow(&session, &tab, url, params, None).await {
        Ok(r) => r,
        Err(e) => {
            if let Err(e) = session.close_tab(tab).await {
                tracing::warn!("close_tab failed: {e}");
            }
            if let Err(e) = session.disconnect().await {
                tracing::warn!("disconnect failed: {e}");
            }
            on_cdp_error(&e);
            return 1;
        }
    };

    if let Err(e) = session.close_tab(tab).await {
        tracing::warn!("close_tab failed: {e}");
    }
    if let Err(e) = session.disconnect().await {
        tracing::warn!("disconnect failed: {e}");
    }

    let truncated = result.pagination.as_ref().is_some_and(|p| p.truncated);

    if json {
        let output = serde_json::to_string(&result).unwrap_or_else(|e| {
            tracing::error!("serialize output failed: {e}");
            String::new()
        });
        println!("{}", output);
    } else {
        println!("Title: {}", result.title);
        println!("URL: {}", result.url);
        println!(
            "Provenance: {:?} via {} ({}ms)",
            result.provenance.method, result.provenance.agent, result.provenance.duration_ms,
        );
        if truncated {
            println!("Content (truncated to {max_chars} chars):");
        } else {
            println!("Content:");
        }
        println!("{}", result.content);
    }
    0
}
