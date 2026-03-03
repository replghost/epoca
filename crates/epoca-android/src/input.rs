use crate::layout::LayoutNode;
use epoca_protocol::*;

/// Handles touch input and hit-testing against the layout tree.
pub struct InputHandler {
    layout_root: Option<LayoutNode>,
    /// Track touch-down position for click detection.
    touch_start: Option<TouchStart>,
}

struct TouchStart {
    x: f32,
    y: f32,
    node_id: NodeId,
    callback_id: CallbackId,
}

/// Maximum distance (px) between touch-down and touch-up to count as a click.
const TAP_THRESHOLD: f32 = 20.0;

impl InputHandler {
    pub fn new() -> Self {
        Self {
            layout_root: None,
            touch_start: None,
        }
    }

    /// Update the layout tree used for hit-testing.
    pub fn set_layout(&mut self, root: LayoutNode) {
        self.layout_root = Some(root);
    }

    /// Handle a touch-start event. Returns true if the touch hit a clickable node.
    pub fn touch_down(&mut self, x: f32, y: f32) -> bool {
        if let Some(ref root) = self.layout_root {
            if let Some((node_id, cb_id)) = hit_test(root, x, y) {
                self.touch_start = Some(TouchStart {
                    x,
                    y,
                    node_id,
                    callback_id: cb_id,
                });
                return true;
            }
        }
        self.touch_start = None;
        false
    }

    /// Handle a touch-end event. Returns a GuestEvent if the touch was a click.
    pub fn touch_up(&mut self, x: f32, y: f32) -> Option<GuestEvent> {
        let start = self.touch_start.take()?;

        let dx = x - start.x;
        let dy = y - start.y;
        let dist = (dx * dx + dy * dy).sqrt();

        if dist > TAP_THRESHOLD {
            return None;
        }

        // Check if the touch-up is still within the same node.
        if let Some(ref root) = self.layout_root {
            if let Some((node_id, _)) = hit_test(root, x, y) {
                if node_id == start.node_id {
                    return Some(GuestEvent {
                        callback_id: start.callback_id,
                        kind: EventKind::Click,
                        data: EventData::None,
                    });
                }
            }
        }

        None
    }

    /// Handle text input (from soft keyboard). Finds the focused Input node
    /// and returns a GuestEvent with the text.
    pub fn text_input(&self, text: String) -> Option<GuestEvent> {
        let root = self.layout_root.as_ref()?;
        let (_, cb_id) = find_input_callback(root)?;
        Some(GuestEvent {
            callback_id: cb_id,
            kind: EventKind::Input,
            data: EventData::Text(text),
        })
    }
}

/// Hit-test a point against the layout tree.
/// Returns the deepest node with a Click callback.
fn hit_test(node: &LayoutNode, x: f32, y: f32) -> Option<(NodeId, CallbackId)> {
    if !node.bounds.contains(x, y) {
        return None;
    }

    // Check children depth-first (deepest match wins).
    for child in node.children.iter().rev() {
        if let Some(result) = hit_test(child, x, y) {
            return Some(result);
        }
    }

    // Check this node for a Click callback.
    for cb in &node.callbacks {
        if cb.event == EventKind::Click {
            return Some((node.node_id, cb.id));
        }
    }

    None
}

/// Find the first Input node with an Input callback in the layout tree.
fn find_input_callback(node: &LayoutNode) -> Option<(NodeId, CallbackId)> {
    if node.kind == NodeKind::Input {
        for cb in &node.callbacks {
            if cb.event == EventKind::Input {
                return Some((node.node_id, cb.id));
            }
        }
    }

    for child in &node.children {
        if let Some(result) = find_input_callback(child) {
            return Some(result);
        }
    }

    None
}
