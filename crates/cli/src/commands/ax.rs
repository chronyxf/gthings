//! `gthings ax <url>` — fetch and display the compressed accessibility tree (AX Tree).
//!
//! Creates a browser tab, navigates to the URL, fetches the full AX tree via
//! `Accessibility.getFullAXTree`, compresses it to a compact text format with
//! `[N]` refs mapped to `backendDOMNodeId`, prints the result, and closes the tab.
//!
//! The compressed output is suitable for AI agents to find page elements
//! semantically by role+name instead of relying on brittle DOM selectors.
//!
//! Use `--max-nodes N` to limit the number of compressed nodes (default 500, 0 = unlimited).

use gthings_common::taxonomy::ErrorCode;

use crate::util::{UniversalFlags, connect, emit_error, emit_success};

/// Fetch and display the compressed accessibility tree for a URL.
pub(crate) async fn cmd_ax(flags: &UniversalFlags, url: &str, max_nodes: Option<usize>) -> i32 {
    let session = match connect(flags).await {
        Ok(s) => s,
        Err(c) => return c,
    };

    match gthings_cdp::ax_tree::ax_tree(&session, url, max_nodes).await {
        Ok(result) => {
            // Guard against an empty/errored tree — report failure (exit 1)
            // instead of silently succeeding with `data.tree=""`.
            if result.tree.is_empty() {
                emit_error(
                    flags,
                    ErrorCode::ExtractFailed,
                    "AX_TREE_EMPTY: compressed AX tree is empty",
                    "Check URL and browser connection",
                );
                return 1;
            }
            // stdout carries only envelopes; the compressed tree rides in
            // `data.tree` for every output format (including text).
            let value = serde_json::json!({
                "tree": result.tree,
                "total_nodes": result.total_nodes,
                "truncated": result.truncated,
                "url": url,
                "command": "ax",
            });
            emit_success(flags, value);
            0
        }
        Err(e) => {
            emit_error(
                flags,
                ErrorCode::ExtractFailed,
                &e.to_string(),
                "Check URL and browser connection",
            );
            1
        }
    }
}
