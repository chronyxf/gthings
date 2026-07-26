//! `gthings search` — search Google via CDP browser.

use crate::commands::{connect, on_cdp_error, print_error};
use gthings_search::search;

/// Search: detect → connect → create tab → search → close tab → disconnect.
pub(crate) async fn cmd_search(query: &str, count: usize, json: bool) -> i32 {
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

    let results = match search(&session, &tab, query, count).await {
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

    if json {
        let output = serde_json::to_string(&results).unwrap_or_else(|e| {
            tracing::error!("serialize output failed: {e}");
            String::new()
        });
        println!("{}", output);
    } else {
        for r in &results {
            println!("#{} {} — {}", r.position, r.title, r.url);
            if !r.snippet.is_empty() {
                println!("  {}", r.snippet);
            }
        }
    }
    0
}
