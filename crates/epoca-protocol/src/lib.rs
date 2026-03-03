#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

/// Unique identifier for a node in the view tree.
pub type NodeId = u64;

/// Unique identifier for a callback registered by the guest.
pub type CallbackId = u64;

/// A complete view tree emitted by a guest application.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewTree {
    pub root: ViewNode,
}

/// A single node in the view tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewNode {
    pub id: NodeId,
    pub kind: NodeKind,
    pub props: Props,
    pub children: Vec<ViewNode>,
    pub callbacks: Vec<Callback>,
}

/// The type of view node.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum NodeKind {
    VStack,
    HStack,
    ZStack,
    Text,
    Button,
    Input,
    List,
    Image,
    Table,
    Chart,
    Spacer,
    Divider,
    Container,
}

/// Properties for a view node, stored as an ordered map (no_std compatible).
pub type Props = BTreeMap<String, PropValue>;

/// A property value that can hold various types.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum PropValue {
    String(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    Color(String),
    List(Vec<PropValue>),
}

impl PropValue {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            PropValue::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            PropValue::Int(i) => Some(*i),
            _ => None,
        }
    }

    pub fn as_float(&self) -> Option<f64> {
        match self {
            PropValue::Float(f) => Some(*f),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            PropValue::Bool(b) => Some(*b),
            _ => None,
        }
    }
}

/// A callback that the guest registers on a node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Callback {
    pub id: CallbackId,
    pub event: EventKind,
}

/// The kind of event that triggers a callback.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum EventKind {
    Click,
    Input,
    Submit,
    Change,
    Focus,
    Blur,
}

/// An event sent from the host back to the guest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuestEvent {
    pub callback_id: CallbackId,
    pub kind: EventKind,
    pub data: EventData,
}

/// Data carried by an event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EventData {
    None,
    Text(String),
    Index(usize),
}

/// A patch operation produced by diffing two view trees.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ViewPatch {
    Replace { node_id: NodeId, new_node: ViewNode },
    UpdateProps { node_id: NodeId, props: Props },
    InsertChild { parent_id: NodeId, index: usize, node: ViewNode },
    RemoveChild { parent_id: NodeId, index: usize },
    ReplaceAll { root: ViewNode },
}

/// Diff two view trees and produce a list of patches.
pub fn diff_trees(old: &ViewNode, new: &ViewNode) -> Vec<ViewPatch> {
    let mut patches = Vec::new();
    diff_node(old, new, &mut patches);
    patches
}

fn diff_node(old: &ViewNode, new: &ViewNode, patches: &mut Vec<ViewPatch>) {
    if old.id != new.id || old.kind != new.kind {
        patches.push(ViewPatch::Replace {
            node_id: old.id,
            new_node: new.clone(),
        });
        return;
    }

    if old.props != new.props {
        patches.push(ViewPatch::UpdateProps {
            node_id: new.id,
            props: new.props.clone(),
        });
    }

    let old_len = old.children.len();
    let new_len = new.children.len();
    let min_len = old_len.min(new_len);

    for i in 0..min_len {
        diff_node(&old.children[i], &new.children[i], patches);
    }

    for i in min_len..new_len {
        patches.push(ViewPatch::InsertChild {
            parent_id: new.id,
            index: i,
            node: new.children[i].clone(),
        });
    }

    for i in (min_len..old_len).rev() {
        patches.push(ViewPatch::RemoveChild {
            parent_id: old.id,
            index: i,
        });
    }
}

/// Serialize a ViewTree to postcard bytes.
pub fn serialize_view_tree(tree: &ViewTree) -> Result<Vec<u8>, postcard::Error> {
    postcard::to_allocvec(tree)
}

/// Deserialize a ViewTree from postcard bytes.
pub fn deserialize_view_tree(bytes: &[u8]) -> Result<ViewTree, postcard::Error> {
    postcard::from_bytes(bytes)
}

/// Serialize a GuestEvent to postcard bytes.
pub fn serialize_event(event: &GuestEvent) -> Result<Vec<u8>, postcard::Error> {
    postcard::to_allocvec(event)
}

/// Deserialize a GuestEvent from postcard bytes.
pub fn deserialize_event(bytes: &[u8]) -> Result<GuestEvent, postcard::Error> {
    postcard::from_bytes(bytes)
}

// Helper to build nodes ergonomically
impl ViewNode {
    pub fn new(id: NodeId, kind: NodeKind) -> Self {
        Self {
            id,
            kind,
            props: BTreeMap::new(),
            children: Vec::new(),
            callbacks: Vec::new(),
        }
    }

    pub fn with_prop(mut self, key: &str, value: PropValue) -> Self {
        self.props.insert(String::from(key), value);
        self
    }

    pub fn with_child(mut self, child: ViewNode) -> Self {
        self.children.push(child);
        self
    }

    pub fn with_callback(mut self, id: CallbackId, event: EventKind) -> Self {
        self.callbacks.push(Callback { id, event });
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_node(id: NodeId, content: &str) -> ViewNode {
        ViewNode::new(id, NodeKind::Text)
            .with_prop("content", PropValue::String(String::from(content)))
    }

    #[test]
    fn test_serialize_roundtrip() {
        let tree = ViewTree {
            root: ViewNode::new(1, NodeKind::VStack)
                .with_child(text_node(2, "Hello")),
        };
        let bytes = serialize_view_tree(&tree).unwrap();
        let decoded = deserialize_view_tree(&bytes).unwrap();
        assert_eq!(decoded.root.id, 1);
        assert_eq!(decoded.root.children.len(), 1);
    }

    #[test]
    fn test_diff_prop_change() {
        let old = text_node(1, "Hello");
        let new = text_node(1, "World");
        let patches = diff_trees(&old, &new);
        assert_eq!(patches.len(), 1);
        assert!(matches!(&patches[0], ViewPatch::UpdateProps { node_id: 1, .. }));
    }

    #[test]
    fn test_diff_child_added() {
        let old = ViewNode::new(1, NodeKind::VStack)
            .with_child(text_node(2, "A"));
        let new = ViewNode::new(1, NodeKind::VStack)
            .with_child(text_node(2, "A"))
            .with_child(text_node(3, "B"));
        let patches = diff_trees(&old, &new);
        assert_eq!(patches.len(), 1);
        assert!(matches!(&patches[0], ViewPatch::InsertChild { parent_id: 1, index: 1, .. }));
    }
}
