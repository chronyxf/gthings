//! AxNode wrapper and static-text helpers shared by the compression passes.

use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// A lightweight wrapper around a single AX node `&Value` that provides
/// convenient accessors for common Chrome AX tree fields.
pub(super) struct AxNode<'a> {
    node: &'a Value,
}

impl<'a> AxNode<'a> {
    pub(super) fn new(node: &'a Value) -> Self {
        Self { node }
    }

    /// Extract role string from a node.
    pub(super) fn role(&self) -> &str {
        self.node
            .get("role")
            .and_then(|r| r.get("value"))
            .and_then(|v| v.as_str())
            .unwrap_or_else(|| if self.ignored() { "IGNORED" } else { "NONE" })
    }

    /// Extract name from a node, normalizing whitespace.
    pub(super) fn name(&self) -> String {
        let text = self
            .node
            .get("name")
            .and_then(|r| r.get("value"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        text.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// Check if a node is ignored.
    pub(super) fn ignored(&self) -> bool {
        self.node
            .get("ignored")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    }

    /// Extract properties map from a node.
    pub(super) fn props(&self) -> HashMap<String, Value> {
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
pub(super) fn collect_text_children<'a>(
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
pub(super) fn suppress_static_text<'a>(
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
