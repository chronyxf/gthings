//! Bottom-up survival traversal, tree emission, and final output assembly
//! for the AX tree compressor.

use serde_json::Value;
use std::collections::{HashMap, HashSet};

use super::node::{AxNode, collect_text_children, suppress_static_text};
use super::roles::AxTreeConfig;

/// Iterate over the child nodes of `node`, resolved through `by_id`.
///
/// Shared by the survival traversal and tree emission passes so the
/// copy-pasted child-iteration blocks stay in one place.
fn children<'a>(
    node: &'a Value,
    by_id: &'a HashMap<&'a str, &'a Value>,
) -> impl Iterator<Item = &'a Value> + 'a {
    node.get("childIds")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|cid| cid.as_str())
        .filter_map(|cid_str| by_id.get(cid_str).copied())
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
    // Keep headings
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
pub(super) fn post_traverse<'a>(
    node: &'a Value,
    by_id: &'a HashMap<&'a str, &'a Value>,
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
        for child in children(node, by_id) {
            if post_traverse(child, by_id, survive, config) {
                any_child = true;
            }
        }
        return any_child;
    }

    let mut keep = keep_self(node, config);
    let mut children_survive = false;

    for child in children(node, by_id) {
        if post_traverse(child, by_id, survive, config) {
            children_survive = true;
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
pub(super) struct EmitContext<'a> {
    pub(super) out: Vec<String>,
    pub(super) ref_by_id: HashMap<&'a str, usize>,
    pub(super) ref_map: Vec<(usize, i64)>,
    pub(super) next_ref: usize,
    pub(super) by_id: &'a HashMap<&'a str, &'a Value>,
    pub(super) survive: &'a HashSet<&'a str>,
    pub(super) suppress: &'a HashSet<&'a str>,
    pub(super) config: &'a AxTreeConfig,
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
fn emit_value_part(p: &HashMap<String, Value>) -> String {
    match p.get("value") {
        Some(val) if let Some(s) = val.as_str() => {
            if s.is_empty() {
                String::new()
            } else {
                let mut end = s.len().min(40);
                while end > 0 && !s.is_char_boundary(end) {
                    end -= 1;
                }
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
pub(super) fn emit_tree<'a>(ctx: &mut EmitContext<'a>, node: &'a Value, depth: usize) {
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
        for child in children(node, ctx.by_id) {
            emit_tree(ctx, child, depth);
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
    for child in children(node, ctx.by_id) {
        emit_tree(ctx, child, depth + 1);
    }
}

/// Extract the nodes slice from a `getFullAXTree` result.
pub(super) fn extract_nodes_array(value: &Value) -> &[Value] {
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
pub(super) fn compute_text_suppression<'a>(
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
pub(super) fn build_final_output<'a>(
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

#[cfg(test)]
mod tests {
    use super::super::compress_ax_tree;
    use super::super::tests::make_node;
    use serde_json::json;

    #[test]
    fn test_compress_ax_tree_empty() {
        let result = compress_ax_tree(&json!({}), None);
        assert_eq!(result.tree, "");

        let result = compress_ax_tree(&json!({"result": {}}), None);
        assert_eq!(result.tree, "");
    }

    #[test]
    fn test_compress_ax_tree_keeps_root_with_all_ignored_children() {
        // A named RootWebArea whose children are all ignored must still
        // survive compression — the tree must not collapse to empty.
        let nodes = json!([
            make_node("1", "RootWebArea", "Example", false, vec!["2"], Some(1)),
            make_node("2", "generic", "", true, vec!["3"], None),
            make_node("3", "button", "Hidden", true, vec![], Some(3)),
        ]);
        let result = compress_ax_tree(&json!({"result": {"nodes": nodes}}), None);
        assert!(!result.tree.is_empty(), "tree should not be empty");
        assert!(result.tree.contains("RootWebArea"));
        assert!(result.tree.contains("Example"));
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
}
