use anyhow::{Context, Result};
use std::collections::BTreeMap;
use epoca_protocol::*;

/// Parse a declarative app markup string (TOML-based) into a ViewTree.
///
/// Format example:
/// ```toml
/// [app]
/// title = "My Widget"
///
/// [[node]]
/// kind = "VStack"
/// gap = 8
///
///   [[node.children]]
///   kind = "Text"
///   content = "Hello World"
///   style = "heading"
///
///   [[node.children]]
///   kind = "Button"
///   label = "Click Me"
///   variant = "primary"
///   on_click = "increment"
///
///   [[node.children]]
///   kind = "Text"
///   content = "Counter: 0"
///   bind = "counter_display"
/// ```
pub fn parse_declarative(source: &str) -> Result<ViewTree> {
    let doc: toml::Value = toml::from_str(source)
        .context("Failed to parse declarative markup")?;

    let node = doc
        .get("node")
        .context("Missing top-level 'node' in declarative markup")?;

    let root = parse_node(node, &mut IdAllocator::new())
        .context("Failed to parse root node")?;

    Ok(ViewTree { root })
}

struct IdAllocator {
    next: NodeId,
}

impl IdAllocator {
    fn new() -> Self {
        Self { next: 1 }
    }

    fn alloc(&mut self) -> NodeId {
        let id = self.next;
        self.next += 1;
        id
    }
}

fn parse_node(value: &toml::Value, ids: &mut IdAllocator) -> Result<ViewNode> {
    let table = value
        .as_table()
        .context("Node must be a table")?;

    let kind_str = table
        .get("kind")
        .and_then(|v| v.as_str())
        .context("Node must have a 'kind' field")?;

    let kind = match kind_str {
        "VStack" => NodeKind::VStack,
        "HStack" => NodeKind::HStack,
        "ZStack" => NodeKind::ZStack,
        "Text" => NodeKind::Text,
        "Button" => NodeKind::Button,
        "Input" => NodeKind::Input,
        "List" => NodeKind::List,
        "Image" => NodeKind::Image,
        "Table" => NodeKind::Table,
        "Chart" => NodeKind::Chart,
        "Spacer" => NodeKind::Spacer,
        "Divider" => NodeKind::Divider,
        "Container" => NodeKind::Container,
        other => anyhow::bail!("Unknown node kind: {}", other),
    };

    let mut props = BTreeMap::new();
    let mut callbacks = Vec::new();

    for (key, val) in table {
        match key.as_str() {
            "kind" | "children" => continue,
            k if k.starts_with("on_") => {
                // Event callbacks — the value is an action name string
                let event = match &k[3..] {
                    "click" => EventKind::Click,
                    "input" => EventKind::Input,
                    "submit" => EventKind::Submit,
                    "change" => EventKind::Change,
                    "focus" => EventKind::Focus,
                    "blur" => EventKind::Blur,
                    _ => continue,
                };
                // Use a hash of the action name as the callback ID
                let action = val.as_str().unwrap_or("unknown");
                let cb_id = hash_string(action);
                callbacks.push(Callback { id: cb_id, event });
            }
            _ => {
                if let Some(prop_val) = toml_to_prop(val) {
                    props.insert(key.clone(), prop_val);
                }
            }
        }
    }

    let children = if let Some(children_val) = table.get("children") {
        match children_val {
            toml::Value::Array(arr) => arr
                .iter()
                .map(|v| parse_node(v, ids))
                .collect::<Result<Vec<_>>>()?,
            _ => vec![],
        }
    } else {
        vec![]
    };

    Ok(ViewNode {
        id: ids.alloc(),
        kind,
        props,
        children,
        callbacks,
    })
}

fn toml_to_prop(value: &toml::Value) -> Option<PropValue> {
    match value {
        toml::Value::String(s) => Some(PropValue::String(s.clone())),
        toml::Value::Integer(i) => Some(PropValue::Int(*i)),
        toml::Value::Float(f) => Some(PropValue::Float(*f)),
        toml::Value::Boolean(b) => Some(PropValue::Bool(*b)),
        _ => None,
    }
}

fn hash_string(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple() {
        let source = r#"
[node]
kind = "VStack"
gap = 8

[[node.children]]
kind = "Text"
content = "Hello"
style = "heading"

[[node.children]]
kind = "Button"
label = "Click"
on_click = "do_thing"
"#;
        let tree = parse_declarative(source).unwrap();
        assert_eq!(tree.root.kind, NodeKind::VStack);
        assert_eq!(tree.root.children.len(), 2);
        assert_eq!(tree.root.children[0].kind, NodeKind::Text);
        assert_eq!(tree.root.children[1].kind, NodeKind::Button);
        assert_eq!(tree.root.children[1].callbacks.len(), 1);
    }
}
