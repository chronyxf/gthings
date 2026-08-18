//! Role sets and configuration constants for AX tree compression.

use std::collections::HashSet;
use std::sync::OnceLock;
use std::time::Duration;

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

/// Polling window while waiting for Chrome's lazily-built renderer AX tree.
/// Chrome may return an empty or all-ignored snapshot immediately after
/// `Accessibility.enable`, so `getFullAXTree` is re-polled until the tree is
/// usable or this bound elapses.
pub(super) const AX_TREE_POLL_INTERVAL: Duration = Duration::from_millis(150);
pub(super) const AX_TREE_POLL_TIMEOUT: Duration = Duration::from_secs(5);

/// Minimum node count for a snapshot to count as *settled*: the non-ignored
/// `RootWebArea` plus at least one more node. A lone root is a mid-render
/// shell — Chrome may not have built any body content yet.
pub(super) const AX_TREE_MIN_NODES: usize = 2;

/// Lazily-initialized `&'static HashSet` for the interactive role set.
fn interactive_roles() -> &'static HashSet<&'static str> {
    static SET: OnceLock<HashSet<&'static str>> = OnceLock::new();
    SET.get_or_init(|| HashSet::from_iter(INTERACTIVE_ROLES.iter().copied()))
}

/// Lazily-initialized `&'static HashSet` for the landmark role set.
fn landmark_roles() -> &'static HashSet<&'static str> {
    static SET: OnceLock<HashSet<&'static str>> = OnceLock::new();
    SET.get_or_init(|| HashSet::from_iter(LANDMARK_ROLES.iter().copied()))
}

/// Lazily-initialized `&'static HashSet` for the drop role set.
fn drop_roles() -> &'static HashSet<&'static str> {
    static SET: OnceLock<HashSet<&'static str>> = OnceLock::new();
    SET.get_or_init(|| HashSet::from_iter(DROP_ROLES.iter().copied()))
}

/// Lazily-initialized `&'static HashSet` for the leaf role set.
fn leaf_roles() -> &'static HashSet<&'static str> {
    static SET: OnceLock<HashSet<&'static str>> = OnceLock::new();
    SET.get_or_init(|| HashSet::from_iter(LEAF_ROLES.iter().copied()))
}

/// Configuration for AX tree compression — bundles role sets and max_nodes limit.
///
/// Role sets are `&'static HashSet` references backed by [`OnceLock`], so each
/// set is built exactly once and shared across all [`AxTreeConfig`] instances.
pub(super) struct AxTreeConfig {
    pub(super) interactive: &'static HashSet<&'static str>,
    pub(super) landmark: &'static HashSet<&'static str>,
    pub(super) drop: &'static HashSet<&'static str>,
    pub(super) leaf: &'static HashSet<&'static str>,
    pub(super) max_nodes: usize,
}

impl AxTreeConfig {
    pub(super) fn new(max_nodes: Option<usize>) -> Self {
        Self {
            interactive: interactive_roles(),
            landmark: landmark_roles(),
            drop: drop_roles(),
            leaf: leaf_roles(),
            max_nodes: max_nodes.unwrap_or(500),
        }
    }
}
