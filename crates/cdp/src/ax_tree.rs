//! Accessibility Tree (AX Tree) support for AI agent semantic element finding.
//!
//! Compresses Chrome's `Accessibility.getFullAXTree` output by ~96% (886KB → 22KB tokens
//! on Wikipedia) so AI agents can find page elements by role+name instead of brittle DOM
//! selectors. Clones the approach from `browser-harness-js/skills/cdp/sdk/axview.ts`.
//!
//! # Functions
//! - [`ax_tree`] — navigate to a URL, fetch the full AX tree, return compressed text
//! - [`ax_query`] — query the AX tree by role/accessible name, return matching nodes
//! - [`compress_ax_tree`] — compress raw `getFullAXTree` JSON into a compact text format
//! - [`ax_diff`] — LCS-based structural diff of two compressed AX tree strings

use crate::error::Result;
use crate::session::Session;
use serde_json::{Value, json};
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

/// Query the AX tree by role and/or accessible name using `Accessibility.queryAXTree`.
///
/// The `selector` can be:
/// - A role name, e.g. `"button"`
/// - `role:name`, e.g. `"button:Submit"`
/// - `role["name"]`, e.g. `r#"button["Submit"]"#`
///
/// Returns the raw CDP response `Value` containing matching nodes.
pub async fn ax_query(session: &Session, selector: &str) -> Result<Value> {
    let tab = session.create_background_tab().await?;
    let sid = tab.session_id.as_deref();

    session
        .connection()
        .call("Accessibility.enable", json!({}), sid)
        .await?;

    let (role, name) = parse_selector(selector);

    let mut params = json!({});
    if let Some(r) = role {
        params["role"] = json!(r);
    }
    if let Some(n) = name {
        params["accessibleName"] = json!(n);
    }

    let result = session
        .connection()
        .call("Accessibility.queryAXTree", params, sid)
        .await?;

    let _ = session
        .connection()
        .call("Accessibility.disable", json!({}), sid)
        .await;

    session.close_tab(tab).await?;

    Ok(result)
}

/// Parse a selector string into (role, name).
///
/// Supported formats:
/// - `"button"` → (`Some("button")`, `None`)
/// - `"button:Submit"` → (`Some("button")`, `Some("Submit")`)
/// - `r#"button["Submit"]"#` → (`Some("button")`, `Some("Submit")`)
fn parse_selector(selector: &str) -> (Option<&str>, Option<&str>) {
    let trimmed = selector.trim();
    if trimmed.is_empty() {
        return (None, None);
    }

    // Try role["name"] format
    if let Some(bracket_pos) = trimmed.find('[') {
        let role_part = trimmed[..bracket_pos].trim();
        let rest = &trimmed[bracket_pos + 1..];
        if let Some(close_pos) = rest.find(']') {
            let name_part = rest[..close_pos].trim();
            let name = name_part
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .unwrap_or(name_part)
                .trim();
            let role = if role_part.is_empty() {
                None
            } else {
                Some(role_part)
            };
            let name = if name.is_empty() { None } else { Some(name) };
            return (role, name);
        }
    }

    // Try role:name format
    if let Some(colon_pos) = trimmed.find(':') {
        let role_part = trimmed[..colon_pos].trim();
        let name_part = trimmed[colon_pos + 1..].trim();
        let role = if role_part.is_empty() {
            None
        } else {
            Some(role_part)
        };
        let name = if name_part.is_empty() {
            None
        } else {
            Some(name_part)
        };
        return (role, name);
    }

    // Just a role
    (Some(trimmed), None)
}

// ---------------------------------------------------------------------------
// Plain helper functions (no closures, easy lifetime handling)
// ---------------------------------------------------------------------------

/// Extract role string from a node.
fn node_role(node: &Value) -> &str {
    node.get("role")
        .and_then(|r| r.get("value"))
        .and_then(|v| v.as_str())
        .unwrap_or(if node_ignored(node) {
            "IGNORED"
        } else {
            "NONE"
        })
}

/// Extract name from a node.
fn node_name(node: &Value) -> String {
    node.get("name")
        .and_then(|r| r.get("value"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Check if a node is ignored.
fn node_ignored(node: &Value) -> bool {
    node.get("ignored")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// Collect text from StaticText children recursively.
fn collect_text_children<'a>(
    node: &'a Value,
    by_id: &HashMap<&'a str, &'a Value>,
    parts: &mut Vec<String>,
) {
    if let Some(child_ids) = node.get("childIds").and_then(|v| v.as_array()) {
        for cid in child_ids {
            if let Some(cid_str) = cid.as_str() {
                if let Some(child) = by_id.get(cid_str) {
                    if node_role(child) == "StaticText" {
                        if let Some(name) = child
                            .get("name")
                            .and_then(|r| r.get("value"))
                            .and_then(|v| v.as_str())
                        {
                            parts.push(name.to_string());
                        }
                    }
                    collect_text_children(child, by_id, parts);
                }
            }
        }
    }
}

/// Mark StaticText children for suppression.
fn suppress_static_text<'a>(
    node: &'a Value,
    by_id: &HashMap<&'a str, &'a Value>,
    suppress: &mut HashSet<&'a str>,
) {
    if let Some(child_ids) = node.get("childIds").and_then(|v| v.as_array()) {
        for cid in child_ids {
            if let Some(cid_str) = cid.as_str() {
                if let Some(child) = by_id.get(cid_str) {
                    if node_role(child) == "StaticText" {
                        if let Some(nid) = child.get("nodeId").and_then(|v| v.as_str()) {
                            suppress.insert(nid);
                        }
                    }
                    suppress_static_text(child, by_id, suppress);
                }
            }
        }
    }
}

/// Check if a node should be kept for its own sake (not just as ancestor).
fn keep_self(
    node: &Value,
    interactive_set: &HashSet<&str>,
    landmark_set: &HashSet<&str>,
    drop_set: &HashSet<&str>,
    leaf_set: &HashSet<&str>,
) -> bool {
    let r = node_role(node);
    let nm = node_name(node);

    if leaf_set.contains(r) {
        return false;
    }
    // Always keep interactive and landmark roles
    if interactive_set.contains(r) || landmark_set.contains(r) {
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
    if !nm.is_empty() && !drop_set.contains(r) {
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
    interactive_set: &HashSet<&str>,
    landmark_set: &HashSet<&str>,
    drop_set: &HashSet<&str>,
    leaf_set: &HashSet<&str>,
) -> bool {
    if !node.is_object() {
        return false;
    }
    let r = node_role(node);
    if leaf_set.contains(r) {
        return false;
    }
    if node_ignored(node) {
        let mut any_child = false;
        if let Some(child_ids) = node.get("childIds").and_then(|v| v.as_array()) {
            for cid in child_ids {
                if let Some(cid_str) = cid.as_str() {
                    if let Some(child) = by_id.get(cid_str) {
                        if post_traverse(
                            child,
                            by_id,
                            survive,
                            interactive_set,
                            landmark_set,
                            drop_set,
                            leaf_set,
                        ) {
                            any_child = true;
                        }
                    }
                }
            }
        }
        return any_child;
    }

    let mut keep = keep_self(node, interactive_set, landmark_set, drop_set, leaf_set);
    let mut children_survive = false;

    if let Some(child_ids) = node.get("childIds").and_then(|v| v.as_array()) {
        for cid in child_ids {
            if let Some(cid_str) = cid.as_str() {
                if let Some(child) = by_id.get(cid_str) {
                    if post_traverse(
                        child,
                        by_id,
                        survive,
                        interactive_set,
                        landmark_set,
                        drop_set,
                        leaf_set,
                    ) {
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

/// Extract properties map from a node.
fn node_props(node: &Value) -> HashMap<String, Value> {
    let mut p = HashMap::new();
    if let Some(properties) = node.get("properties").and_then(|v| v.as_array()) {
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

/// Recursively emit the compressed tree.
#[allow(clippy::too_many_arguments)]
fn emit_tree<'a>(
    node: &'a Value,
    depth: usize,
    by_id: &HashMap<&'a str, &'a Value>,
    survive: &HashSet<&'a str>,
    suppress: &HashSet<&'a str>,
    interactive_set: &HashSet<&str>,
    landmark_set: &HashSet<&str>,
    drop_set: &HashSet<&str>,
    leaf_set: &HashSet<&str>,
    out: &mut Vec<String>,
    ref_by_id: &mut HashMap<&'a str, usize>,
    ref_map: &mut Vec<(usize, i64)>,
    next_ref: &mut usize,
) {
    if !node.is_object() {
        return;
    }
    let nid = match node.get("nodeId").and_then(|v| v.as_str()) {
        Some(id) => id,
        None => return,
    };
    if suppress.contains(nid) {
        return;
    }

    let r = node_role(node);
    let nm = node_name(node);

    // Handle ignored / dropped nodes by descending into children
    if node_ignored(node)
        || (drop_set.contains(r)
            && r != "heading"
            && !interactive_set.contains(r)
            && !landmark_set.contains(r))
    {
        if let Some(child_ids) = node.get("childIds").and_then(|v| v.as_array()) {
            for cid in child_ids {
                if let Some(cid_str) = cid.as_str() {
                    if let Some(child) = by_id.get(cid_str) {
                        emit_tree(
                            child,
                            depth,
                            by_id,
                            survive,
                            suppress,
                            interactive_set,
                            landmark_set,
                            drop_set,
                            leaf_set,
                            out,
                            ref_by_id,
                            ref_map,
                            next_ref,
                        );
                    }
                }
            }
        }
        return;
    }

    if !survive.contains(nid) || leaf_set.contains(r) {
        return;
    }

    // Build ref for actionable/named nodes
    let dom_id = node.get("backendDOMNodeId").and_then(|v| v.as_i64());
    let has_ref = dom_id.is_some()
        && (interactive_set.contains(r) || r == "heading" || (r != "StaticText" && !nm.is_empty()));

    let ref_str = if has_ref {
        let ref_num = if let Some(existing) = ref_by_id.get(nid) {
            *existing
        } else {
            let n = *next_ref;
            ref_by_id.insert(nid, n);
            if let Some(bdom) = dom_id {
                ref_map.push((n, bdom));
            }
            *next_ref += 1;
            n
        };
        format!("[{}] ", ref_num)
    } else {
        String::new()
    };

    // Build flags string
    let p = node_props(node);
    let mut flags: Vec<&str> = Vec::new();
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
    let value_part = if let Some(val) = p.get("value") {
        if let Some(s) = val.as_str() {
            if !s.is_empty() {
                format!(" =\"{}\"", &s[..s.len().min(40)])
            } else {
                String::new()
            }
        } else if let Some(n) = val.as_f64() {
            format!(" =\"{}\"", n)
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    out.push(format!(
        "{}{}{}{}{}{}{}",
        indent, ref_str, r, name_part, flags_part, level_part, value_part
    ));

    // Emit children
    if let Some(child_ids) = node.get("childIds").and_then(|v| v.as_array()) {
        for cid in child_ids {
            if let Some(cid_str) = cid.as_str() {
                if let Some(child) = by_id.get(cid_str) {
                    emit_tree(
                        child,
                        depth + 1,
                        by_id,
                        survive,
                        suppress,
                        interactive_set,
                        landmark_set,
                        drop_set,
                        leaf_set,
                        out,
                        ref_by_id,
                        ref_map,
                        next_ref,
                    );
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
pub fn compress_ax_tree(value: &Value, max_nodes: Option<usize>) -> AxTreeResult {
    // Extract nodes — they can be at `result.nodes` (full tree) or `result` (direct array)
    let nodes = value
        .get("result")
        .and_then(|r| r.get("nodes"))
        .or_else(|| value.get("nodes"))
        .or_else(|| {
            // Maybe it's already an array
            if value.is_array() { Some(value) } else { None }
        })
        .and_then(|v| v.as_array())
        .map(std::vec::Vec::as_slice)
        .unwrap_or_default();

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

    let interactive_set: HashSet<&str> = HashSet::from_iter(INTERACTIVE_ROLES.iter().copied());
    let landmark_set: HashSet<&str> = HashSet::from_iter(LANDMARK_ROLES.iter().copied());
    let drop_set: HashSet<&str> = HashSet::from_iter(DROP_ROLES.iter().copied());
    let leaf_set: HashSet<&str> = HashSet::from_iter(LEAF_ROLES.iter().copied());

    // Find root — prefer RootWebArea, else first non-ignored
    let root = by_id
        .values()
        .find(|n| node_role(n) == "RootWebArea")
        .or_else(|| by_id.values().find(|n| !node_ignored(n)))
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

    post_traverse(
        root,
        &by_id,
        &mut survive,
        &interactive_set,
        &landmark_set,
        &drop_set,
        &leaf_set,
    );

    // Phase 1b: coalesce redundant text children — if a node's name matches the
    // concatenated text of its StaticText children, suppress those children.
    let mut suppress: HashSet<&str> = HashSet::new();
    for node in by_id.values() {
        let nid = match node.get("nodeId").and_then(|v| v.as_str()) {
            Some(id) => id,
            None => continue,
        };
        if !survive.contains(nid) {
            continue;
        }
        let r = node_role(node);
        if r == "StaticText" {
            continue;
        }
        let nm = node_name(node);
        if nm.is_empty() {
            continue;
        }
        let mut text_parts: Vec<String> = Vec::new();
        collect_text_children(node, &by_id, &mut text_parts);
        let joined: String = text_parts.join(" ");
        if joined.split_whitespace().eq(nm.split_whitespace()) {
            suppress_static_text(node, &by_id, &mut suppress);
        }
    }

    // Phase 2: emit tree
    let mut out: Vec<String> = Vec::new();
    let mut ref_by_id: HashMap<&str, usize> = HashMap::new();
    let mut ref_map: Vec<(usize, i64)> = Vec::new();
    let mut next_ref: usize = 1;

    emit_tree(
        root,
        0,
        &by_id,
        &survive,
        &suppress,
        &interactive_set,
        &landmark_set,
        &drop_set,
        &leaf_set,
        &mut out,
        &mut ref_by_id,
        &mut ref_map,
        &mut next_ref,
    );

    let total_nodes = out.len();
    let effective_max = max_nodes.unwrap_or(500);
    let truncated = effective_max > 0 && total_nodes > effective_max;

    // Build final output: truncated tree + refs map
    let mut final_lines: Vec<String> = if truncated {
        let mut lines: Vec<String> = out.drain(..effective_max).collect();
        lines.push(String::new());
        lines.push(format!(
            "... truncated {} nodes (max {}) ...",
            total_nodes - effective_max,
            effective_max
        ));
        lines
    } else {
        out
    };

    // Append refs map (always included, after any truncation message)
    if !ref_map.is_empty() {
        final_lines.push(String::new());
        final_lines.push("# refs -> backendDOMNodeId".to_string());
        let refs_line: String = ref_map
            .iter()
            .map(|(r, b)| format!("[{}]={}", r, b))
            .collect::<Vec<_>>()
            .join(" ");
        final_lines.push(refs_line);
    }

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
    let bytes = text.as_bytes();
    let n = bytes.len();
    let mut i = 0;
    while i < n {
        if bytes[i] == b'[' {
            // Try to find a closing ]
            let mut j = i + 1;
            let mut has_digits = false;
            while j < n && bytes[j].is_ascii_digit() {
                has_digits = true;
                j += 1;
            }
            if has_digits && j < n && bytes[j] == b']' {
                // Consume the ref, then skip optional whitespace
                let mut k = j + 1;
                while k < n && bytes[k] == b' ' {
                    k += 1;
                }
                i = k;
                continue;
            }
        }
        result.push(bytes[i] as char);
        i += 1;
    }
    result
}

/// Compute the LCS-based structural diff of two compressed AX tree strings.
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
    let n = a.len();
    let m = b.len();

    // Compute LCS length table
    let mut dp = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            let key_a = normalize_line(&a[i]);
            let key_b = normalize_line(&b[j]);
            dp[i][j] = if key_a == key_b {
                dp[i + 1][j + 1] + 1
            } else {
                dp[i + 1][j].max(dp[i][j + 1])
            };
        }
    }

    // Trace back to produce diff (strip refs from both comparison AND output)
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    let mut j = 0;
    while i < n && j < m {
        let key_a = normalize_line(&a[i]);
        let key_b = normalize_line(&b[j]);
        if key_a == key_b {
            i += 1;
            j += 1;
        } else if i + 1 < n && dp[i + 1][j] >= dp[i][j + 1] {
            let stripped = strip_ref_pattern(a[i].trim_start());
            out.push(format!("- {}", stripped.trim()));
            i += 1;
        } else if j + 1 < m {
            let stripped = strip_ref_pattern(b[j].trim_start());
            out.push(format!("+ {}", stripped.trim()));
            j += 1;
        } else {
            // edge: one of them is at the last element
            if dp[i + 1][j] >= dp[i][j + 1] {
                let stripped = strip_ref_pattern(a[i].trim_start());
                out.push(format!("- {}", stripped.trim()));
                i += 1;
            } else {
                let stripped = strip_ref_pattern(b[j].trim_start());
                out.push(format!("+ {}", stripped.trim()));
                j += 1;
            }
        }
    }
    while i < n {
        let stripped = strip_ref_pattern(a[i].trim_start());
        out.push(format!("- {}", stripped.trim()));
        i += 1;
    }
    while j < m {
        let stripped = strip_ref_pattern(b[j].trim_start());
        out.push(format!("+ {}", stripped.trim()));
        j += 1;
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
    fn test_parse_selector_role_only() {
        let (role, name) = parse_selector("button");
        assert_eq!(role, Some("button"));
        assert_eq!(name, None);
    }

    #[test]
    fn test_parse_selector_role_name_colon() {
        let (role, name) = parse_selector("button:Submit");
        assert_eq!(role, Some("button"));
        assert_eq!(name, Some("Submit"));
    }

    #[test]
    fn test_parse_selector_role_name_bracket() {
        let (role, name) = parse_selector(r#"button["Submit"]"#);
        assert_eq!(role, Some("button"));
        assert_eq!(name, Some("Submit"));
    }

    #[test]
    fn test_parse_selector_empty() {
        let (role, name) = parse_selector("");
        assert_eq!(role, None);
        assert_eq!(name, None);
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

    // ── Explicitly-named tests for parse_selector ─────────────────────────

    #[test]
    fn test_ax_query_parses_role_only() {
        let (role, name) = parse_selector("button");
        assert_eq!(role, Some("button"));
        assert_eq!(name, None);
    }

    #[test]
    fn test_ax_query_parses_role_and_name() {
        let (role, name) = parse_selector("button:Submit");
        assert_eq!(role, Some("button"));
        assert_eq!(name, Some("Submit"));
    }

    #[test]
    fn test_ax_query_parses_role_with_bracket_name() {
        let (role, name) = parse_selector(r#"button["Submit"]"#);
        assert_eq!(role, Some("button"));
        assert_eq!(name, Some("Submit"));
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
