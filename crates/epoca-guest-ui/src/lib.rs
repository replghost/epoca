//! Ergonomic UI builder functions for Epoca PolkaVM guest apps.
//!
//! Instead of constructing `ViewNode` trees by hand:
//! ```ignore
//! ViewNode::new(1, NodeKind::VStack)
//!     .with_prop("gap", PropValue::Int(12))
//!     .with_child(ViewNode::new(2, NodeKind::Text)
//!         .with_prop("content", PropValue::String("hello".into())))
//! ```
//!
//! Use builder functions:
//! ```ignore
//! vstack(12, vec![
//!     text("hello"),
//! ])
//! ```
//!
//! All functions return `Node`, which converts to `ViewTree` via `.into_tree()`.
//! Node IDs are assigned automatically via a global counter.

#![no_std]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use epoca_protocol::*;

// ---------------------------------------------------------------------------
// Auto-incrementing ID generators
// ---------------------------------------------------------------------------

static NEXT_NODE_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_CB_ID: AtomicU64 = AtomicU64::new(1);

fn next_node_id() -> NodeId {
    NEXT_NODE_ID.fetch_add(1, Ordering::Relaxed)
}

fn next_cb_id() -> CallbackId {
    NEXT_CB_ID.fetch_add(1, Ordering::Relaxed)
}

/// Reset ID counters. Call at the start of each `emit_view` to get
/// stable IDs across re-renders (important for diffing).
pub fn reset_ids() {
    NEXT_NODE_ID.store(1, Ordering::Relaxed);
    NEXT_CB_ID.store(1, Ordering::Relaxed);
}

// ---------------------------------------------------------------------------
// Node — the builder type
// ---------------------------------------------------------------------------

/// A UI node builder. Wraps `ViewNode` with ergonomic methods.
pub struct Node {
    inner: ViewNode,
}

impl Node {
    /// Convert to a `ViewTree` (call on the root node).
    pub fn into_tree(self) -> ViewTree {
        ViewTree { root: self.inner }
    }

    /// Convert to the underlying `ViewNode`.
    pub fn into_view_node(self) -> ViewNode {
        self.inner
    }

    // -- Style modifiers --

    /// Set the text style to "heading".
    pub fn heading(mut self) -> Self {
        self.inner
            .props
            .insert(String::from("style"), PropValue::String(String::from("heading")));
        self
    }

    /// Mark as primary variant (buttons).
    pub fn primary(mut self) -> Self {
        self.inner
            .props
            .insert(String::from("variant"), PropValue::String(String::from("primary")));
        self
    }

    /// Set a custom string property.
    pub fn prop(mut self, key: &str, value: &str) -> Self {
        self.inner
            .props
            .insert(String::from(key), PropValue::String(String::from(value)));
        self
    }

    /// Set a custom integer property.
    pub fn prop_int(mut self, key: &str, value: i64) -> Self {
        self.inner
            .props
            .insert(String::from(key), PropValue::Int(value));
        self
    }

    /// Set a custom boolean property.
    pub fn prop_bool(mut self, key: &str, value: bool) -> Self {
        self.inner
            .props
            .insert(String::from(key), PropValue::Bool(value));
        self
    }

    /// Set placeholder text (inputs).
    pub fn placeholder(mut self, text: &str) -> Self {
        self.inner
            .props
            .insert(String::from("placeholder"), PropValue::String(String::from(text)));
        self
    }

    /// Set the current value (inputs).
    pub fn value(mut self, text: &str) -> Self {
        self.inner
            .props
            .insert(String::from("value"), PropValue::String(String::from(text)));
        self
    }

    /// Set visibility.
    pub fn visible(mut self, v: bool) -> Self {
        self.inner
            .props
            .insert(String::from("visible"), PropValue::Bool(v));
        self
    }

    // -- Event callbacks --

    /// Register a click handler. Returns the assigned `CallbackId`.
    pub fn on_click(mut self, id: &mut CallbackId) -> Self {
        let cb = next_cb_id();
        *id = cb;
        self.inner.callbacks.push(Callback {
            id: cb,
            event: EventKind::Click,
        });
        self
    }

    /// Register a click handler with a known callback ID.
    pub fn on_click_id(mut self, id: CallbackId) -> Self {
        self.inner.callbacks.push(Callback {
            id,
            event: EventKind::Click,
        });
        self
    }

    /// Register an input handler. Returns the assigned `CallbackId`.
    pub fn on_input(mut self, id: &mut CallbackId) -> Self {
        let cb = next_cb_id();
        *id = cb;
        self.inner.callbacks.push(Callback {
            id: cb,
            event: EventKind::Input,
        });
        self
    }

    /// Register an input handler with a known callback ID.
    pub fn on_input_id(mut self, id: CallbackId) -> Self {
        self.inner.callbacks.push(Callback {
            id,
            event: EventKind::Input,
        });
        self
    }

    /// Register a submit handler with a known callback ID.
    pub fn on_submit_id(mut self, id: CallbackId) -> Self {
        self.inner.callbacks.push(Callback {
            id,
            event: EventKind::Submit,
        });
        self
    }
}

// ---------------------------------------------------------------------------
// Builder functions
// ---------------------------------------------------------------------------

/// Vertical stack with gap (pixels) between children.
pub fn vstack(gap: i64, children: Vec<Node>) -> Node {
    let mut node = ViewNode::new(next_node_id(), NodeKind::VStack);
    node.props
        .insert(String::from("gap"), PropValue::Int(gap));
    node.children = children.into_iter().map(|n| n.inner).collect();
    Node { inner: node }
}

/// Horizontal stack with gap (pixels) between children.
pub fn hstack(gap: i64, children: Vec<Node>) -> Node {
    let mut node = ViewNode::new(next_node_id(), NodeKind::HStack);
    node.props
        .insert(String::from("gap"), PropValue::Int(gap));
    node.children = children.into_iter().map(|n| n.inner).collect();
    Node { inner: node }
}

/// Text label.
pub fn text(content: &str) -> Node {
    Node {
        inner: ViewNode::new(next_node_id(), NodeKind::Text)
            .with_prop("content", PropValue::String(String::from(content))),
    }
}

/// Button with a label.
pub fn button(label: &str) -> Node {
    Node {
        inner: ViewNode::new(next_node_id(), NodeKind::Button)
            .with_prop("label", PropValue::String(String::from(label))),
    }
}

/// Text input field.
pub fn input() -> Node {
    Node {
        inner: ViewNode::new(next_node_id(), NodeKind::Input),
    }
}

/// Flexible spacer — absorbs remaining space in a stack.
pub fn spacer() -> Node {
    Node {
        inner: ViewNode::new(next_node_id(), NodeKind::Spacer),
    }
}

/// Horizontal divider line.
pub fn divider() -> Node {
    Node {
        inner: ViewNode::new(next_node_id(), NodeKind::Divider),
    }
}

/// Container with padding.
pub fn container(children: Vec<Node>) -> Node {
    let mut node = ViewNode::new(next_node_id(), NodeKind::Container);
    node.children = children.into_iter().map(|n| n.inner).collect();
    Node { inner: node }
}

/// Z-stack (overlay children on top of each other).
pub fn zstack(children: Vec<Node>) -> Node {
    let mut node = ViewNode::new(next_node_id(), NodeKind::ZStack);
    node.children = children.into_iter().map(|n| n.inner).collect();
    Node { inner: node }
}

// ---------------------------------------------------------------------------
// Convenience: format text with Display values
// ---------------------------------------------------------------------------

/// Text label built from a formatted string.
/// Use like: `textf(&format!("Count: {}", count))`
pub fn textf(content: &str) -> Node {
    text(content)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    extern crate alloc;
    use alloc::vec;
    use super::*;

    #[test]
    fn test_counter_ui() {
        reset_ids();
        let count = 42i64;

        let tree = vstack(12, vec![
            text(&alloc::format!("Count: {count}")).heading(),
            hstack(8, vec![
                button("+").primary().on_click_id(100),
                button("-").on_click_id(101),
            ]),
        ])
        .into_tree();

        assert_eq!(tree.root.kind, NodeKind::VStack);
        assert_eq!(tree.root.children.len(), 2);

        let heading = &tree.root.children[0];
        assert_eq!(heading.kind, NodeKind::Text);
        assert_eq!(
            heading.props.get("content"),
            Some(&PropValue::String(String::from("Count: 42")))
        );
        assert_eq!(
            heading.props.get("style"),
            Some(&PropValue::String(String::from("heading")))
        );

        let hstack = &tree.root.children[1];
        assert_eq!(hstack.kind, NodeKind::HStack);
        assert_eq!(hstack.children.len(), 2);

        let plus_btn = &hstack.children[0];
        assert_eq!(plus_btn.kind, NodeKind::Button);
        assert_eq!(
            plus_btn.props.get("variant"),
            Some(&PropValue::String(String::from("primary")))
        );
        assert_eq!(plus_btn.callbacks.len(), 1);
        assert_eq!(plus_btn.callbacks[0].id, 100);
    }

    #[test]
    fn test_input_with_placeholder() {
        reset_ids();

        let tree = vstack(8, vec![
            text("Enter your name:"),
            input().placeholder("Name...").on_input_id(50),
        ])
        .into_tree();

        let input_node = &tree.root.children[1];
        assert_eq!(input_node.kind, NodeKind::Input);
        assert_eq!(
            input_node.props.get("placeholder"),
            Some(&PropValue::String(String::from("Name...")))
        );
        assert_eq!(input_node.callbacks.len(), 1);
        assert_eq!(input_node.callbacks[0].event, EventKind::Input);
    }

    #[test]
    fn test_spacer_and_divider() {
        reset_ids();

        let tree = vstack(0, vec![
            text("Top"),
            spacer(),
            divider(),
            text("Bottom"),
        ])
        .into_tree();

        assert_eq!(tree.root.children[1].kind, NodeKind::Spacer);
        assert_eq!(tree.root.children[2].kind, NodeKind::Divider);
    }

    #[test]
    fn test_ids_are_unique() {
        let tree = vstack(0, vec![text("a"), text("b")]).into_tree();

        let root_id = tree.root.id;
        let id_a = tree.root.children[0].id;
        let id_b = tree.root.children[1].id;

        // All three nodes get distinct IDs.
        assert_ne!(root_id, id_a);
        assert_ne!(root_id, id_b);
        assert_ne!(id_a, id_b);
    }

    #[test]
    fn test_visible_modifier() {
        reset_ids();
        let node = text("hidden").visible(false).into_view_node();
        assert_eq!(node.props.get("visible"), Some(&PropValue::Bool(false)));
    }
}
