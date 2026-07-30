//! Accessibility Tree (AX Tree) support for AI agent semantic element finding.
//!
//! Compresses Chrome's `Accessibility.getFullAXTree` output by ~96% (886KB → 22KB tokens
//! on Wikipedia) so AI agents can find page elements by role+name instead of brittle DOM
//! selectors. Clones the approach from `browser-harness-js/skills/cdp/sdk/axview.ts`.
//!
//! # Functions
//! - [`ax_tree`] — navigate to a URL, fetch the full AX tree, return compressed text
//! - [`compress_ax_tree`] — compress raw `getFullAXTree` JSON into a compact text format
//! - [`ax_diff`] — LCS-based structural diff of two compressed AX tree strings

use crate::error::Result;
use crate::session::Session;
use serde_json::{Value, json};
use similar::{Algorithm, ChangeTag, TextDiff};
use std::collections::{HashMap, HashSet};

/// Roles that are interactive/actionable — the agent can click, type, or focus these.
const INTERACTIVE_ROLES: &[&str] = &[
    "button",
    "checkbox",
    "combobox",
    "grid",
    "gridcell",
    "link",
    "listbox",
    "menu",
    "menubar",
    "menuitem",
    "menuitemcheckbox",
    "menuitemradio",
    "option",
    "radio",
    "scrollbar",
    "searchbox",
    "slider",
    "spinbutton",
    "switch",
    "tab",
    "tablist",
    "textbox",
    "tree",
    "treegrid",
    "treeitem",
    "rowheader",
    "columnheader",
    "canvas",
];

/// Roles that are landmarks / semantic containers.
const LANDMARK_ROLES: &[&str] = &[
    "application",
    "article",
    "banner",
    "complementary",
    "contentinfo",
    "form",
    "main",
    "navigation",
    "region",
    "search",
];

/// Roles that are purely structural and should be dropped.
const DROP_ROLES: &[&str] = &[
    "Abbr",
    "cell",
    "deletion",
    "div",
    "figcaption",
    "figure",
    "generic",
    "group",
    "insertion",
    "list",
    "listitem",
    "none",
    "paragraph",
    "presentation",
    "row",
    "rowgroup",
    "section",
    "separator",
    "subscript",
    "superscript",
    "table",
];

/// Leaf AX roles that wrap raw text — no semantic value, always dropped.
const LEAF_ROLES: &[&str] = &["InlineTextBox", "LineBreak", "ListMarker"];

/// Configuration for AX tree compression — bundles role sets and max_nodes limit.
struct AxTreeConfig {
    interactive: HashSet<&'static str>,
    landmark: HashSet<&'static str>,
    drop: HashSet<&'static str>,
    leaf: HashSet<&'static str>,
    max_nodes: usize,
}

impl AxTreeConfig {
    fn new(max_nodes: Option<usize>) -> Self {
        Self {
            interactive: HashSet::from_iter(INTERACTIVE_ROLES.iter().copied()),
            landmark: HashSet::from_iter(LANDMARK_ROLES.iter().copied()),
            drop: HashSet::from_iter(DROP_ROLES.iter().copied()),
            leaf: HashSet::from_iter(LEAF_ROLES.iter().copied()),
            max_nodes: max_nodes.unwrap_or(500),
        }
    }
}

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

    let result = session
        .connection()
        .call("Accessibility.getFullAXTree", json!({}), sid)
        .await?;

    // Cleanup
    let _ = session
        .connection()
        .call("Accessibility.disable", json!({}), sid)
        .await;

    session.close_tab(tab).await?;

    Ok(compress_ax_tree(&result, max_nodes))
}

// ---------------------------------------------------------------------------
// AxNode wrapper — bundles role/name/ignored/props queries into methods
// ---------------------------------------------------------------------------

/// A lightweight wrapper around a single AX node `&Value` that provides
/// convenient accessors for common Chrome AX tree fields.
struct AxNode<'a> {
    node: &'a Value,
}

impl<'a> AxNode<'a> {
    fn new(node: &'a Value) -> Self {
        Self { node }
    }

    /// Extract role string from a node.
    fn role(&self) -> &str {
        self.node
            .get("role")
            .and_then(|r| r.get("value"))
            .and_then(|v| v.as_str())
            .unwrap_or_else(|| if self.ignored() { "IGNORED" } else { "NONE" })
    }

    /// Extract name from a node, normalizing whitespace (no intermediate Vec).
    fn name(&self) -> String {
        let text = self
            .node
            .get("name")
            .and_then(|r| r.get("value"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let mut result = String::with_capacity(text.len());
        let mut first = true;
        for word in text.split_whitespace() {
            if !first {
                result.push(' ');
            }
            result.push_str(word);
            first = false;
        }
        result
    }

    /// Check if a node is ignored.
    fn ignored(&self) -> bool {
        self.node
            .get("ignored")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    }

    /// Extract properties map from a node.
    fn props(&self) -> HashMap<String, Value> {
        let mut p = HashMap::new();
        if let Some(properties) = self.node.get("properties").and_then(|v| v.as_array()) {
            for prop in properties {
                if let (Some(name), Some(val)) =
                    (prop.get("name").and_then(|v| v.as_str()), prop.get("value"))
                {
                    if let Some(inner_val) = val.get("value") {
                        p.insert(name.to_string(), inner_val.clone());
                    }
                }
            }
        }
        p
    }
}

/// Traverse all StaticText children recursively and call `f` on each.
fn for_each_child_static_text<'a>(
    node: &'a Value,
    by_id: &HashMap<&'a str, &'a Value>,
    f: &mut dyn FnMut(&'a Value),
) {
    if let Some(child_ids) = node.get("childIds").and_then(|v| v.as_array()) {
        for cid in child_ids {
            if let Some(cid_str) = cid.as_str() {
                if let Some(child) = by_id.get(cid_str) {
                    if AxNode::new(child).role() == "StaticText" {
                        f(child);
                    }
                    for_each_child_static_text(child, by_id, f);
                }
            }
        }
    }
}

/// Collect text from StaticText children recursively.
fn collect_text_children<'a>(
    node: &'a Value,
    by_id: &HashMap<&'a str, &'a Value>,
    parts: &mut Vec<String>,
) {
    for_each_child_static_text(node, by_id, &mut |child| {
        if let Some(name) = child
            .get("name")
            .and_then(|r| r.get("value"))
            .and_then(|v| v.as_str())
        {
            parts.push(name.to_string());
        }
    });
}

/// Mark StaticText children for suppression.
fn suppress_static_text<'a>(
    node: &'a Value,
    by_id: &HashMap<&'a str, &'a Value>,
    suppress: &mut HashSet<&'a str>,
) {
    for_each_child_static_text(node, by_id, &mut |child| {
        if let Some(nid) = child.get("nodeId").and_then(|v| v.as_str()) {
            suppress.insert(nid);
        }
    });
}

/// Check if a node should be kept for its own sake (not just as ancestor).
fn keep_self(node: &Value, config: &AxTreeConfig) -> bool {
    let n = AxNode::new(node);
    let r = n.role();
    let nm = n.name();

    if config.leaf.contains(r) {
        return false;
    }
    // Always keep interactive and landmark roles
    if config.interactive.contains(r) || config.landmark.contains(r) {
        return true;
    }
    // Keep non-empty headings
    if r == "heading" {
        return true;
    }
    // Keep non-empty StaticText
    if r == "StaticText" && !nm.is_empty() {
        return true;
    }
    // Keep named nodes that aren't in the drop set
    if !nm.is_empty() && !config.drop.contains(r) {
        return true;
    }
    false
}

/// Bottom-up post traversal: determine which nodes survive the compression.
/// Returns true if the node (or any descendant) should survive.
fn post_traverse<'a>(
    node: &'a Value,
    by_id: &HashMap<&'a str, &'a Value>,
    survive: &mut HashSet<&'a str>,
    config: &AxTreeConfig,
) -> bool {
    if !node.is_object() {
        return false;
    }
    let n = AxNode::new(node);
    let r = n.role();
    if config.leaf.contains(r) {
        return false;
    }
    if n.ignored() {
        let mut any_child = false;
        if let Some(child_ids) = node.get("childIds").and_then(|v| v.as_array()) {
            for cid in child_ids {
                if let Some(cid_str) = cid.as_str() {
                    if let Some(child) = by_id.get(cid_str) {
                        if post_traverse(child, by_id, survive, config) {
                            any_child = true;
                        }
                    }
                }
            }
        }
        return any_child;
    }

    let mut keep = keep_self(node, config);
    let mut children_survive = false;

    if let Some(child_ids) = node.get("childIds").and_then(|v| v.as_array()) {
        for cid in child_ids {
            if let Some(cid_str) = cid.as_str() {
                if let Some(child) = by_id.get(cid_str) {
                    if post_traverse(child, by_id, survive, config) {
                        children_survive = true;
                    }
                }
            }
        }
    }

    if children_survive {
        keep = true;
    }

    if keep {
        survive.insert(node.get("nodeId").and_then(|v| v.as_str()).unwrap_or(""));
    }

    keep
}

/// Mutable context for [`emit_tree`] — bundles all state that is threaded through
/// the recursive traversal so the function signature is minimal.
struct EmitContext<'a> {
    out: Vec<String>,
    ref_by_id: HashMap<&'a str, usize>,
    ref_map: Vec<(usize, i64)>,
    next_ref: usize,
    by_id: &'a HashMap<&'a str, &'a Value>,
    survive: &'a HashSet<&'a str>,
    suppress: &'a HashSet<&'a str>,
    config: &'a AxTreeConfig,
}

/// Collect flags from a property map into a `Vec<&'static str>`.
fn emit_flags_vec(p: &HashMap<String, Value>) -> Vec<&'static str> {
    let mut flags: Vec<&'static str> = Vec::new();
    if p.get("focused").and_then(|v| v.as_bool()) == Some(true) {
        flags.push("focused");
    }
    match p.get("checked").and_then(|v| v.as_str()) {
        Some("true") => flags.push("checked"),
        Some("mixed") => flags.push("mixed"),
        _ => {}
    }
    if p.get("selected").and_then(|v| v.as_bool()) == Some(true) {
        flags.push("selected");
    }
    if p.get("expanded").and_then(|v| v.as_bool()) == Some(true) {
        flags.push("expanded");
    }
    if p.get("disabled").and_then(|v| v.as_bool()) == Some(true) {
        flags.push("disabled");
    }
    if p.get("required").and_then(|v| v.as_bool()) == Some(true) {
        flags.push("required");
    }
    if p.get("pressed").and_then(|v| v.as_str()) == Some("true") {
        flags.push("pressed");
    }
    flags
}

/// Build a `[N] ` reference string for an actionable/named node.
fn emit_ref_str<'a>(
    ctx: &mut EmitContext<'a>,
    nid: &'a str,
    dom_id: Option<i64>,
    has_ref: bool,
) -> String {
    if !has_ref {
        return String::new();
    }
    let ref_num = if let Some(existing) = ctx.ref_by_id.get(nid) {
        *existing
    } else {
        let n = ctx.next_ref;
        ctx.ref_by_id.insert(nid, n);
        if let Some(bdom) = dom_id {
            ctx.ref_map.push((n, bdom));
        }
        ctx.next_ref += 1;
        n
    };
    format!("[{}] ", ref_num)
}

/// Build the `="value"` suffix for a property map, with truncation at 40 chars.
#[allow(clippy::incompatible_msrv)]
fn emit_value_part(p: &HashMap<String, Value>) -> String {
    match p.get("value") {
        Some(val) if let Some(s) = val.as_str() => {
            if s.is_empty() {
                String::new()
            } else {
                let end = s.floor_char_boundary(s.len().min(40));
                format!(" =\"{}\"", &s[..end])
            }
        }
        Some(val) if let Some(n) = val.as_f64() => {
            format!(" =\"{}\"", n)
        }
        _ => String::new(),
    }
}

/// Recursively emit the compressed tree.
fn emit_tree<'a>(ctx: &mut EmitContext<'a>, node: &'a Value, depth: usize) {
    if !node.is_object() {
        return;
    }
    let nid = match node.get("nodeId").and_then(|v| v.as_str()) {
        Some(id) => id,
        None => return,
    };
    if ctx.suppress.contains(nid) {
        return;
    }

    let ax = AxNode::new(node);
    let r = ax.role();
    let nm = ax.name();

    // Handle ignored / dropped nodes by descending into children
    if ax.ignored()
        || (ctx.config.drop.contains(r)
            && r != "heading"
            && !ctx.config.interactive.contains(r)
            && !ctx.config.landmark.contains(r))
    {
        if let Some(child_ids) = node.get("childIds").and_then(|v| v.as_array()) {
            for cid in child_ids {
                if let Some(cid_str) = cid.as_str() {
                    if let Some(child) = ctx.by_id.get(cid_str) {
                        emit_tree(ctx, child, depth);
                    }
                }
            }
        }
        return;
    }

    if !ctx.survive.contains(nid) || ctx.config.leaf.contains(r) {
        return;
    }

    // Build ref for actionable/named nodes
    let dom_id = node.get("backendDOMNodeId").and_then(|v| v.as_i64());
    let has_ref = dom_id.is_some()
        && (ctx.config.interactive.contains(r)
            || r == "heading"
            || (r != "StaticText" && !nm.is_empty()));

    let ref_str = emit_ref_str(ctx, nid, dom_id, has_ref);

    // Build flags string
    let p = ax.props();
    let flags = emit_flags_vec(&p);

    let indent = "  ".repeat(depth);
    let name_part = if nm.is_empty() {
        String::new()
    } else {
        format!(" \"{}\"", nm)
    };
    let flags_part = if flags.is_empty() {
        String::new()
    } else {
        format!(" <{}>", flags.join(" "))
    };

    // Add heading level
    let level_part = if r == "heading" {
        if let Some(level) = p.get("level").and_then(|v| v.as_i64()) {
            format!(" (h{})", level)
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    // Add value
    let value_part = emit_value_part(&p);

    ctx.out.push(format!(
        "{}{}{}{}{}{}{}",
        indent, ref_str, r, name_part, flags_part, level_part, value_part
    ));

    // Emit children
    if let Some(child_ids) = node.get("childIds").and_then(|v| v.as_array()) {
        for cid in child_ids {
            if let Some(cid_str) = cid.as_str() {
                if let Some(child) = ctx.by_id.get(cid_str) {
                    emit_tree(ctx, child, depth + 1);
                }
            }
        }
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
///
/// Extract the nodes slice from a `getFullAXTree` result.
fn extract_nodes_array(value: &Value) -> &[Value] {
    value
        .get("result")
        .and_then(|r| r.get("nodes"))
        .or_else(|| value.get("nodes"))
        .or_else(|| if value.is_array() { Some(value) } else { None })
        .and_then(|v| v.as_array())
        .map(std::vec::Vec::as_slice)
        .unwrap_or_default()
}

/// Compute the set of suppressed node IDs by coalescing redundant text children.
fn compute_text_suppression<'a>(
    by_id: &HashMap<&'a str, &'a Value>,
    survive: &HashSet<&'a str>,
) -> HashSet<&'a str> {
    let mut suppress: HashSet<&str> = HashSet::new();
    for node in by_id.values() {
        let nid = match node.get("nodeId").and_then(|v| v.as_str()) {
            Some(id) => id,
            None => continue,
        };
        if !survive.contains(nid) {
            continue;
        }
        let ax_node = AxNode::new(node);
        let r = ax_node.role();
        if r == "StaticText" {
            continue;
        }
        let nm = ax_node.name();
        if nm.is_empty() {
            continue;
        }
        let mut text_parts: Vec<String> = Vec::new();
        collect_text_children(node, by_id, &mut text_parts);
        let joined: String = text_parts.join(" ");
        if joined.split_whitespace().eq(nm.split_whitespace()) {
            suppress_static_text(node, by_id, &mut suppress);
        }
    }
    suppress
}

/// Build the final output lines from the emitted context, including truncation
/// and the refs map.
fn build_final_output<'a>(
    mut ctx: EmitContext<'a>,
    config: &AxTreeConfig,
) -> (Vec<String>, usize, bool) {
    let total_nodes = ctx.out.len();
    let truncated = config.max_nodes > 0 && total_nodes > config.max_nodes;

    let mut final_lines: Vec<String> = if truncated {
        let mut lines: Vec<String> = ctx.out.drain(..config.max_nodes).collect();
        lines.push(String::new());
        lines.push(format!(
            "... truncated {} nodes (max {}) ...",
            total_nodes - config.max_nodes,
            config.max_nodes
        ));
        lines
    } else {
        ctx.out
    };

    if !ctx.ref_map.is_empty() {
        final_lines.push(String::new());
        final_lines.push("# refs -> backendDOMNodeId".to_string());
        let refs_line = ctx
            .ref_map
            .iter()
            .map(|(r, b)| format!("[{}]={}", r, b))
            .collect::<Vec<_>>()
            .join(" ");
        final_lines.push(refs_line);
    }

    (final_lines, total_nodes, truncated)
}

pub fn compress_ax_tree(value: &Value, max_nodes: Option<usize>) -> AxTreeResult {
    let nodes = extract_nodes_array(value);

    if nodes.is_empty() {
        return AxTreeResult {
            tree: String::new(),
            total_nodes: 0,
            truncated: false,
        };
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
            return AxTreeResult {
                tree: String::new(),
                total_nodes: 0,
                truncated: false,
            };
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

/// Normalize a compressed tree line for diffing: strip `[N]` refs (they renumber
/// every snapshot) and trailing whitespace.
fn normalize_line(line: &str) -> String {
    // Strip `[N] ` refs at the start or `[N]=M` ref map entries
    let without_refs = strip_ref_pattern(line);
    without_refs.trim_end().to_string()
}

/// Strip `[N]` reference patterns from a line.
fn strip_ref_pattern(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut i = 0;
    while i < n {
        if chars[i] == '[' {
            // Try to find a closing ]
            let mut j = i + 1;
            let mut has_digits = false;
            while j < n && chars[j].is_ascii_digit() {
                has_digits = true;
                j += 1;
            }
            if has_digits && j < n && chars[j] == ']' {
                // Consume the ref, then skip optional whitespace
                let mut k = j + 1;
                while k < n && chars[k] == ' ' {
                    k += 1;
                }
                i = k;
                continue;
            }
        }
        result.push(chars[i]);
        i += 1;
    }
    result
}

/// Compute the structural diff of two compressed AX tree strings using Myers'
/// algorithm (O(N) space via the `similar` crate).
///
/// Refs are stripped before comparison (they renumber every snapshot).
/// Lines present only in `prev` are prefixed with `-`, lines only in `next`
/// with `+`. Unchanged lines are omitted.
///
/// # Output format
///
/// ```text
/// - button "Log in"
/// + button "Log out"
/// ```
pub fn ax_diff(prev: &str, next: &str) -> String {
    // Collect meaningful lines, stripping empty / ref-metadata lines.
    let lines = |s: &str| -> Vec<String> {
        s.split('\n')
            .map(|l| l.trim_end().to_string())
            .filter(|l| {
                let trimmed = l.trim();
                if trimmed.is_empty() {
                    return false;
                }
                if trimmed.starts_with("# refs") {
                    return false;
                }
                // ref map lines: [N]=M [N]=M ...
                if trimmed
                    .split_whitespace()
                    .all(|tok| tok.starts_with('[') && tok.contains("]="))
                {
                    return false;
                }
                true
            })
            .collect()
    };

    let a = lines(prev);
    let b = lines(next);

    // Build comparison sequences with refs stripped
    let norm_a: Vec<String> = a.iter().map(|l| normalize_line(l)).collect();
    let norm_b: Vec<String> = b.iter().map(|l| normalize_line(l)).collect();
    let norm_a_refs: Vec<&str> = norm_a.iter().map(|s| s.as_str()).collect();
    let norm_b_refs: Vec<&str> = norm_b.iter().map(|s| s.as_str()).collect();

    // Myers diff (O(N) space)
    let diff = TextDiff::configure()
        .algorithm(Algorithm::Myers)
        .diff_slices(&norm_a_refs, &norm_b_refs);

    let mut out: Vec<String> = Vec::new();
    let mut i = 0; // index into `a`
    let mut j = 0; // index into `b`

    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Equal => {
                i += 1;
                j += 1;
            }
            ChangeTag::Delete => {
                let stripped = strip_ref_pattern(a[i].trim_start());
                out.push(format!("- {}", stripped.trim()));
                i += 1;
            }
            ChangeTag::Insert => {
                let stripped = strip_ref_pattern(b[j].trim_start());
                out.push(format!("+ {}", stripped.trim()));
                j += 1;
            }
        }
    }

    if out.is_empty() {
        "(no changes)".to_string()
    } else {
        out.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Build a minimal AX node for testing.
    fn make_node(
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

    #[test]
    fn test_compress_ax_tree_empty() {
        let result = compress_ax_tree(&json!({}), None);
        assert_eq!(result.tree, "");

        let result = compress_ax_tree(&json!({"result": {}}), None);
        assert_eq!(result.tree, "");
    }

    #[test]
    fn test_compress_ax_tree_reduces_size() {
        // Build a realistic tree
        let nodes = json!([
            make_node(
                "1",
                "RootWebArea",
                "Test Page",
                false,
                vec!["2", "3"],
                Some(1)
            ),
            make_node("2", "heading", "Welcome", false, vec![], Some(10)),
            make_node("3", "button", "Click Me", false, vec![], Some(20)),
        ]);

        let raw = json!({"result": {"nodes": nodes}});
        let raw_str = serde_json::to_string(&raw).unwrap();
        let result = compress_ax_tree(&raw, None);

        assert!(
            !result.tree.is_empty(),
            "compressed output should not be empty"
        );
        assert!(
            result.tree.len() < raw_str.len(),
            "compressed ({}) should be smaller than raw ({})",
            result.tree.len(),
            raw_str.len()
        );
        assert!(result.tree.contains("[1]"), "should contain ref [1]");
        assert!(result.tree.contains("[2]"), "should contain ref [2]");
        assert!(
            result.tree.contains("RootWebArea"),
            "should contain RootWebArea"
        );
        assert!(result.tree.contains("heading"), "should contain heading");
        assert!(result.tree.contains("button"), "should contain button");
        assert!(
            result.tree.contains("Click Me"),
            "should contain button name"
        );
        assert!(
            result.tree.contains("Welcome"),
            "should contain heading name"
        );
        assert!(result.tree.contains("# refs"), "should contain refs map");
    }

    #[test]
    fn test_compress_ax_tree_drops_ignored() {
        let nodes = json!([
            make_node("1", "RootWebArea", "Page", false, vec!["2", "3"], Some(1)),
            make_node("2", "button", "Visible", false, vec![], Some(10)),
            make_node("3", "button", "Hidden", true, vec![], Some(11)),
        ]);

        let result = json!({"result": {"nodes": nodes}});
        let compressed = compress_ax_tree(&result, None);

        assert!(
            compressed.tree.contains("Visible"),
            "should contain visible button"
        );
        assert!(
            !compressed.tree.contains("Hidden"),
            "should NOT contain hidden button"
        );
    }

    #[test]
    fn test_compress_ax_tree_drops_inline_text_box() {
        let nodes = json!([
            make_node("1", "RootWebArea", "Page", false, vec!["2"], Some(1)),
            make_node("2", "InlineTextBox", "some text", false, vec![], None),
        ]);

        let result = json!({"result": {"nodes": nodes}});
        let compressed = compress_ax_tree(&result, None);

        assert!(
            !compressed.tree.contains("some text"),
            "should NOT contain InlineTextBox text"
        );
        assert!(
            !compressed.tree.contains("InlineTextBox"),
            "should NOT mention InlineTextBox"
        );
    }

    #[test]
    fn test_ax_diff_additions_and_deletions() {
        let prev = r#"[1] RootWebArea "Page"
  [2] button "Log in"
  [3] link "About"
"#;
        let next = r#"[1] RootWebArea "Page"
  [2] button "Log out"
  [4] link "Contact"
"#;

        let diff = ax_diff(prev, next);

        assert!(diff.contains("- button"), "should show deletion: - button");
        assert!(diff.contains("Log in"), "should mention removed name");
        assert!(diff.contains("+ button"), "should show addition: + button");
        assert!(diff.contains("Log out"), "should mention new name");
        // "About" was removed, "Contact" was added
        assert!(diff.contains("- link"), "should show link removal");
        assert!(diff.contains("About"), "should mention removed link name");
        assert!(diff.contains("+ link"), "should show link addition");
        assert!(diff.contains("Contact"), "should mention new link name");
    }

    #[test]
    fn test_ax_diff_no_changes() {
        let tree = r#"[1] RootWebArea "Page"
  [2] button "Click"
"#;
        let diff = ax_diff(tree, tree);
        assert!(
            diff.contains("no changes"),
            "identical trees should show no changes: got: {diff}"
        );
    }

    #[test]
    fn test_ax_diff_strips_refs_before_compare() {
        // Same structure, different ref numbers — should show no changes
        let prev = r#"[5] RootWebArea "Page"
  [7] button "Go"
"#;
        let next = r#"[1] RootWebArea "Page"
  [2] button "Go"
"#;
        let diff = ax_diff(prev, next);
        assert!(
            diff.contains("no changes"),
            "same structure with different refs should show no changes: got: {diff}"
        );
    }

    // ── Explicitly-named tests for compress_ax_tree ───────────────────────

    #[test]
    fn test_compress_ax_tree_ignores_inline_text() {
        // InlineTextBox and other leaf AX roles should be dropped from output.
        let nodes = json!([
            make_node("1", "RootWebArea", "Page", false, vec!["2"], Some(1)),
            make_node("2", "InlineTextBox", "some text", false, vec![], None),
            make_node("3", "LineBreak", "", false, vec![], None),
            make_node("4", "ListMarker", "•", false, vec![], None),
        ]);

        let result = json!({"result": {"nodes": nodes}});
        let compressed = compress_ax_tree(&result, None);

        assert!(
            !compressed.tree.contains("InlineTextBox"),
            "should not mention InlineTextBox"
        );
        assert!(
            !compressed.tree.contains("LineBreak"),
            "should not mention LineBreak"
        );
        assert!(
            !compressed.tree.contains("ListMarker"),
            "should not mention ListMarker"
        );
        assert!(
            !compressed.tree.contains("some text"),
            "should not contain InlineTextBox text"
        );
    }

    #[test]
    fn test_compress_ax_tree_ignores_generic_roles() {
        // Structural roles like generic, paragraph, div should be dropped.
        let nodes = json!([
            make_node(
                "1",
                "RootWebArea",
                "Page",
                false,
                vec!["2", "3", "4"],
                Some(1)
            ),
            make_node("2", "generic", "", false, vec!["5"], None),
            make_node("3", "paragraph", "", false, vec![], None),
            make_node("4", "div", "", false, vec![], None),
            // A button is a child of the generic container — it should survive
            make_node("5", "button", "Click", false, vec![], Some(10)),
        ]);

        let result = json!({"result": {"nodes": nodes}});
        let compressed = compress_ax_tree(&result, None);

        // Generic/p/div should be dropped
        assert!(
            !compressed.tree.contains("generic"),
            "should not contain generic role"
        );
        assert!(
            !compressed.tree.contains("paragraph"),
            "should not contain paragraph role"
        );
        assert!(
            !compressed.tree.contains("div"),
            "should not contain div role"
        );
        // But children of generic containers should survive
        assert!(compressed.tree.contains("button"), "should contain button");
        assert!(
            compressed.tree.contains("Click"),
            "should contain button name"
        );
    }

    #[test]
    fn test_compress_ax_tree_keeps_interactive_roles() {
        // Interactive/actionable roles must always survive compression.
        let nodes = json!([
            make_node(
                "1",
                "RootWebArea",
                "Page",
                false,
                vec!["2", "3", "4", "5"],
                Some(1)
            ),
            make_node("2", "button", "Submit", false, vec![], Some(10)),
            make_node("3", "link", "Home", false, vec![], Some(20)),
            make_node("4", "textbox", "Search", false, vec![], Some(30)),
            make_node("5", "heading", "Welcome", false, vec![], Some(40)),
        ]);

        let result = json!({"result": {"nodes": nodes}});
        let compressed = compress_ax_tree(&result, None);

        assert!(compressed.tree.contains("button"), "should contain button");
        assert!(compressed.tree.contains("link"), "should contain link");
        assert!(
            compressed.tree.contains("textbox"),
            "should contain textbox"
        );
        assert!(
            compressed.tree.contains("heading"),
            "should contain heading"
        );
        assert!(
            compressed.tree.contains("Submit"),
            "should contain button name"
        );
        assert!(compressed.tree.contains("Home"), "should contain link name");
        assert!(
            compressed.tree.contains("Search"),
            "should contain textbox name"
        );
        assert!(
            compressed.tree.contains("Welcome"),
            "should contain heading name"
        );
    }

    // ── Focused ax_diff tests ─────────────────────────────────────────────

    #[test]
    fn test_ax_diff_shows_additions() {
        let prev = r#"[1] RootWebArea "Page"
  [2] button "Log in"
"#;
        let next = r#"[1] RootWebArea "Page"
  [2] button "Log in"
  [3] link "Sign up"
"#;

        let diff = ax_diff(prev, next);

        // Addition should be prefixed with `+`
        assert!(diff.contains("+"), "diff should contain '+' prefix");
        assert!(
            diff.contains("+ link"),
            "diff should show added link: {diff}"
        );
        assert!(
            diff.contains("Sign up"),
            "diff should show new node name: {diff}"
        );
        // No deletions should be present
        assert!(
            !diff.contains("- "),
            "diff should not have deletions: {diff}"
        );
    }

    #[test]
    fn test_ax_diff_shows_deletions() {
        let prev = r#"[1] RootWebArea "Page"
  [2] button "Log in"
  [3] link "Sign up"
"#;
        let next = r#"[1] RootWebArea "Page"
  [2] button "Log in"
"#;

        let diff = ax_diff(prev, next);

        // Deletion should be prefixed with `-`
        assert!(diff.contains("-"), "diff should contain '-' prefix");
        assert!(
            diff.contains("- link"),
            "diff should show removed link: {diff}"
        );
        assert!(
            diff.contains("Sign up"),
            "diff should show removed node name: {diff}"
        );
        // No additions should be present
        assert!(
            !diff.contains("+ "),
            "diff should not have additions: {diff}"
        );
    }

    #[test]
    fn test_ax_diff_strips_refs() {
        // Same content, different ref numbering — should show no changes
        let prev = r#"[10] RootWebArea "Diff"
  [20] heading "Title" (h1)
  [30] button "Go"
"#;
        let next = r#"[1] RootWebArea "Diff"
  [2] heading "Title" (h1)
  [3] button "Go"
"#;

        let diff = ax_diff(prev, next);
        assert!(
            diff.contains("no changes"),
            "same content with different ref numbering should show no changes: got: {diff}"
        );
    }
}
