//! Accessibility Tree (AX Tree) support for AI agent semantic element finding.
//!
//! Compresses Chrome's `Accessibility.getFullAXTree` output by ~96% (886KB → 22KB tokens
//! on Wikipedia) so AI agents can find page elements by role+name instead of brittle DOM
//! selectors. Clones the approach from `browser-harness-js/skills/cdp/sdk/axview.ts`.
//!
//! # Functions
//! - [`ax_tree`] — navigate to a URL, fetch the full AX tree, return compressed text
//! - [`compress_ax_tree`] — compress raw `getFullAXTree` JSON into a compact text format

mod compress;
mod node;
mod roles;

use crate::error::{CdpError, Result};
use crate::session::Session;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::time::Instant;

use self::compress::{
    EmitContext, build_final_output, compute_text_suppression, emit_tree, extract_nodes_array,
    post_traverse,
};
use self::node::AxNode;
use self::roles::{AX_TREE_MIN_NODES, AX_TREE_POLL_INTERVAL, AX_TREE_POLL_TIMEOUT, AxTreeConfig};

/// Result of compressing an AX tree, including truncation metadata.
pub struct AxTreeResult {
    /// The compressed tree text (potentially truncated).
    pub tree: String,
    /// Total number of compressed nodes before truncation.
    pub total_nodes: usize,
    /// Whether the tree was truncated.
    pub truncated: bool,
}

/// Navigate to `url`, fetch the full AX tree, compress it, and return the result.
///
/// Creates a tab, navigates, enables the Accessibility domain, calls
/// `Accessibility.getFullAXTree`, compresses the result via [`compress_ax_tree`],
/// disables the Accessibility domain, and closes the tab.
///
/// `max_nodes` controls the maximum number of compressed tree nodes to include.
/// - `None` → default 500
/// - `Some(0)` → unlimited
/// - `Some(n)` → limit to n nodes
pub async fn ax_tree(
    session: &Session,
    url: &str,
    max_nodes: Option<usize>,
) -> Result<AxTreeResult> {
    let tab = session.create_background_tab().await?;
    tab.navigate(session, url).await?;
    let sid = tab.session_id.as_deref();

    // Enable the Accessibility domain before querying
    session
        .connection()
        .call("Accessibility.enable", json!({}), sid)
        .await?;

    // Chrome builds the renderer AX tree lazily: the first snapshot right
    // after `Accessibility.enable` is frequently empty, all-ignored, or a
    // mid-render shell (an unnamed `RootWebArea` with no body content yet).
    // Poll until the tree is usable, then require it to SETTLE — two
    // consecutive identical snapshots prove Chrome finished building the
    // renderer AX tree instead of handing us an in-progress snapshot.
    let mut result = session
        .connection()
        .call("Accessibility.getFullAXTree", json!({}), sid)
        .await?;
    let deadline = Instant::now() + AX_TREE_POLL_TIMEOUT;
    let mut prev: Option<Value> = None;
    loop {
        if ax_tree_settled(prev.as_ref(), &result) {
            break;
        }
        if Instant::now() >= deadline {
            break;
        }
        prev = Some(result);
        tokio::time::sleep(AX_TREE_POLL_INTERVAL).await;
        result = session
            .connection()
            .call("Accessibility.getFullAXTree", json!({}), sid)
            .await?;
    }

    // Cleanup
    let _ = session
        .connection()
        .call("Accessibility.disable", json!({}), sid)
        .await;

    session.close_tab(tab).await?;

    if !ax_tree_ready(&result) {
        return Err(CdpError::CdpCallFailed {
            method: "Accessibility.getFullAXTree".into(),
            detail: "AX_TREE_EMPTY: getFullAXTree returned no nodes".into(),
        });
    }

    Ok(compress_ax_tree(&result, max_nodes))
}

/// Whether a `getFullAXTree` response holds a usable tree: at least one node
/// AND a non-ignored `RootWebArea`. Chrome's lazy renderer AX tree can return
/// `nodes: []` or an all-ignored root on the first snapshot — this predicate
/// lets the poll loop tell an in-progress tree from a genuinely empty one.
fn ax_tree_ready(value: &Value) -> bool {
    let nodes = extract_nodes_array(value);
    if nodes.is_empty() {
        return false;
    }
    nodes
        .iter()
        .any(|n| AxNode::new(n).role() == "RootWebArea" && !AxNode::new(n).ignored())
}

/// Whether the current snapshot counts as *settled*: the tree is usable
/// (`RootWebArea` present), holds at least [`AX_TREE_MIN_NODES`] nodes (root
/// plus body content — a lone root is a mid-render shell), and is byte-identical
/// to the previously observed snapshot. Mid-render trees keep growing between
/// polls, so two consecutive equal snapshots mean rendering settled.
fn ax_tree_settled(prev: Option<&Value>, current: &Value) -> bool {
    if ax_tree_ready(current) && extract_nodes_array(current).len() >= AX_TREE_MIN_NODES {
        return prev == Some(current);
    }
    false
}

/// An empty compression result — no nodes were found to compress.
fn empty_result() -> AxTreeResult {
    AxTreeResult {
        tree: String::new(),
        total_nodes: 0,
        truncated: false,
    }
}

/// Compress the raw `Accessibility.getFullAXTree` JSON result into compact text.
///
/// The raw JSON contains thousands of nodes with structural noise (InlineTextBox,
/// ignored nodes, generic containers). This function:
///
/// 1. Drops ignored nodes and structural roles (InlineTextBox, generic, paragraph, etc.)
/// 2. Keeps interactive nodes (button, link, textbox, etc.), landmarks, headings
/// 3. Assigns sequential `[N]` refs mapped to `backendDOMNodeId`
/// 4. Formats tree with indentation
/// 5. Truncates to `max_nodes` lines (if set, default 500)
///
/// # Output format
///
/// ```text
/// [1] RootWebArea "Example"
///   [2] heading "Welcome" (h1)
///   [3] navigation
///     [4] link "Home"
///     [5] link "About"
///   [6] button "Submit" <disabled>
///
/// # refs -> backendDOMNodeId
/// [1]=42 [2]=47 [3]=53 [4]=55 [5]=58 [6]=72
/// ```
///
/// `max_nodes` controls the maximum number of compressed tree nodes to include.
/// - `None` → default 500
/// - `Some(0)` → unlimited
/// - `Some(n)` → limit to n nodes
pub fn compress_ax_tree(value: &Value, max_nodes: Option<usize>) -> AxTreeResult {
    let nodes = extract_nodes_array(value);

    if nodes.is_empty() {
        return empty_result();
    }

    // Build nodeId → node map
    let by_id: HashMap<&str, &Value> = nodes
        .iter()
        .filter_map(|n| n.get("nodeId").and_then(|id| id.as_str()).map(|id| (id, n)))
        .collect();

    let config = AxTreeConfig::new(max_nodes);

    // Find root — prefer RootWebArea, else first non-ignored
    let root = by_id
        .values()
        .find(|n| AxNode::new(n).role() == "RootWebArea")
        .or_else(|| by_id.values().find(|n| !AxNode::new(n).ignored()))
        .copied();

    let root = match root {
        Some(r) => r,
        None => {
            return empty_result();
        }
    };

    // Phase 1: determine surviving nodes via bottom-up traversal
    let mut survive: HashSet<&str> = HashSet::new();
    post_traverse(root, &by_id, &mut survive, &config);

    // Phase 1b: coalesce redundant text children
    let suppress = compute_text_suppression(&by_id, &survive);

    // Phase 2: emit tree
    let mut ctx = EmitContext {
        out: Vec::new(),
        ref_by_id: HashMap::new(),
        ref_map: Vec::new(),
        next_ref: 1,
        by_id: &by_id,
        survive: &survive,
        suppress: &suppress,
        config: &config,
    };

    emit_tree(&mut ctx, root, 0);

    let (final_lines, total_nodes, truncated) = build_final_output(ctx, &config);

    AxTreeResult {
        tree: final_lines.join("\n"),
        total_nodes,
        truncated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Build a minimal AX node for testing.
    pub(super) fn make_node(
        node_id: &str,
        role: &str,
        name: &str,
        ignored: bool,
        child_ids: Vec<&str>,
        backend_dom_id: Option<i64>,
    ) -> Value {
        let mut node = json!({
            "nodeId": node_id,
            "ignored": ignored,
            "role": {"value": role, "type": "string"},
        });
        if !name.is_empty() {
            node["name"] = json!({"value": name, "type": "string"});
        }
        if !child_ids.is_empty() {
            let ids: Vec<Value> = child_ids.iter().map(|s| json!(s)).collect();
            node["childIds"] = json!(ids);
        }
        if let Some(dom_id) = backend_dom_id {
            node["backendDOMNodeId"] = json!(dom_id);
        }
        node
    }

    // ── ax_tree_ready (poll guard) ────────────────────────────────────────

    #[test]
    fn test_ax_tree_ready_accepts_usable_tree() {
        let nodes = json!([
            make_node("1", "RootWebArea", "Example", false, vec![], Some(1)),
            make_node("2", "button", "Click", false, vec![], Some(2)),
        ]);
        assert!(ax_tree_ready(&json!({"result": {"nodes": nodes}})));
        // Top-level `nodes` (and a bare array) are accepted shapes too.
        assert!(ax_tree_ready(&json!({"nodes": nodes})));
        assert!(ax_tree_ready(&nodes));
    }

    #[test]
    fn test_ax_tree_ready_rejects_empty_nodes() {
        assert!(!ax_tree_ready(&json!({"nodes": []})));
        assert!(!ax_tree_ready(&json!({"result": {"nodes": []}})));
        assert!(!ax_tree_ready(&json!({"result": {}})));
        assert!(!ax_tree_ready(&json!({})));
    }

    #[test]
    fn test_ax_tree_ready_rejects_all_ignored_root() {
        let nodes = json!([
            make_node("1", "RootWebArea", "Example", true, vec![], Some(1)),
            make_node("2", "button", "Click", true, vec![], Some(2)),
        ]);
        assert!(!ax_tree_ready(&json!({"result": {"nodes": nodes}})));
    }

    #[test]
    fn test_ax_tree_ready_requires_root_web_area() {
        // Non-empty nodes, but no RootWebArea present — not a usable tree.
        let nodes = json!([
            make_node("1", "generic", "", false, vec![], None),
            make_node("2", "button", "Click", false, vec![], Some(2)),
        ]);
        assert!(!ax_tree_ready(&json!({"result": {"nodes": nodes}})));
    }

    // ── ax_tree_settled (stability guard) ────────────────────────────────

    /// A two-node snapshot: non-ignored `RootWebArea` + one child.
    fn two_node_snapshot() -> Value {
        json!({"result": {"nodes": [
            make_node("1", "RootWebArea", "Example", false, vec!["2"], Some(1)),
            make_node("2", "button", "Click", false, vec![], Some(2)),
        ]}})
    }

    #[test]
    fn test_ax_tree_settled_accepts_two_identical_snapshots() {
        let snapshot = two_node_snapshot();
        // A second, identical capture means rendering settled.
        assert!(ax_tree_settled(Some(&snapshot), &snapshot));
    }

    #[test]
    fn test_ax_tree_settled_rejects_first_snapshot() {
        // No previous observation yet — a single snapshot can never be
        // "two consecutive equal snapshots".
        let snapshot = two_node_snapshot();
        assert!(!ax_tree_settled(None, &snapshot));
    }

    #[test]
    fn test_ax_tree_settled_rejects_growing_mid_render_shell() {
        let shell = json!({"result": {"nodes": [
            make_node("1", "RootWebArea", "", false, vec![], Some(1)),
        ]}});
        // First capture: lone unnamed root — the mid-render shell from the bug.
        assert!(!ax_tree_settled(None, &shell));
        // Second capture: the shell GREW (body content arrived) — not stable,
        // must keep polling.
        let grown = two_node_snapshot();
        assert!(!ax_tree_settled(Some(&shell), &grown));
        // The grown tree must also clear the minimum node count before it can
        // settle; a lone root held stable across polls is still a shell.
        assert!(!ax_tree_settled(Some(&shell), &shell));
    }

    #[test]
    fn test_ax_tree_settled_rejects_empty_or_ignored() {
        let empty = json!({"result": {"nodes": []}});
        let prev = two_node_snapshot();
        assert!(!ax_tree_settled(Some(&prev), &empty));
        let all_ignored = json!({"result": {"nodes": [
            make_node("1", "RootWebArea", "Example", true, vec![], Some(1)),
            make_node("2", "button", "Click", true, vec![], Some(2)),
        ]}});
        assert!(!ax_tree_settled(Some(&all_ignored), &all_ignored));
    }
}
