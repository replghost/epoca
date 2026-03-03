use crate::text::TextEngine;
use crate::theme::Theme;
use epoca_protocol::*;

/// A laid-out node with computed pixel positions and sizes.
#[derive(Debug, Clone)]
pub struct LayoutNode {
    pub node_id: NodeId,
    pub kind: NodeKind,
    pub bounds: Rect,
    pub children: Vec<LayoutNode>,
    pub callbacks: Vec<Callback>,
    pub props: Props,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
}

/// Intrinsic size from the measure pass.
struct IntrinsicSize {
    width: f32,
    height: f32,
    /// Whether this node is a flex spacer (absorbs remaining space).
    is_spacer: bool,
}

/// Lay out a ViewTree into positioned LayoutNodes.
pub fn layout(
    tree: &ViewTree,
    viewport: (f32, f32),
    text_engine: &mut TextEngine,
    theme: &Theme,
) -> LayoutNode {
    let (vw, vh) = viewport;
    let mut root = build_layout_node(&tree.root);
    arrange(&mut root, 0.0, 0.0, vw, vh, text_engine, theme);
    root
}

/// Build a LayoutNode tree from a ViewNode tree (no positioning yet).
fn build_layout_node(node: &ViewNode) -> LayoutNode {
    LayoutNode {
        node_id: node.id,
        kind: node.kind.clone(),
        bounds: Rect::default(),
        children: node.children.iter().map(build_layout_node).collect(),
        callbacks: node.callbacks.clone(),
        props: node.props.clone(),
    }
}

/// Arrange a LayoutNode tree within the given bounds (top-down).
fn arrange(
    node: &mut LayoutNode,
    x: f32,
    y: f32,
    available_w: f32,
    available_h: f32,
    text_engine: &mut TextEngine,
    theme: &Theme,
) {
    node.bounds = Rect::new(x, y, available_w, available_h);

    match &node.kind {
        NodeKind::VStack => {
            arrange_vstack(node, x, y, available_w, available_h, text_engine, theme);
        }
        NodeKind::HStack => {
            arrange_hstack(node, x, y, available_w, available_h, text_engine, theme);
        }
        NodeKind::Container => {
            let inner_x = x + theme.padding;
            let inner_y = y + theme.padding;
            let inner_w = available_w - theme.padding * 2.0;
            let inner_h = available_h - theme.padding * 2.0;
            // Container lays out children vertically like a VStack with no gap.
            arrange_children_vertical(node, inner_x, inner_y, inner_w, inner_h, 0.0, text_engine, theme);
        }
        NodeKind::Text => {
            let content = node
                .props
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let is_heading = node
                .props
                .get("style")
                .and_then(|v| v.as_str())
                .map(|s| s == "heading")
                .unwrap_or(false);
            let font_size = if is_heading {
                theme.heading_size
            } else {
                theme.font_size
            };
            let (tw, th) = text_engine.measure(content, font_size, available_w);
            node.bounds = Rect::new(x, y, tw, th);
        }
        NodeKind::Button => {
            let label = node
                .props
                .get("label")
                .and_then(|v| v.as_str())
                .unwrap_or("Button");
            let (tw, th) = text_engine.measure(label, theme.font_size, available_w);
            let bw = tw + theme.button_pad_h * 2.0;
            let bh = th + theme.button_pad_v * 2.0;
            node.bounds = Rect::new(x, y, bw, bh);
        }
        NodeKind::Input => {
            node.bounds = Rect::new(x, y, available_w, theme.input_height);
        }
        NodeKind::Divider => {
            node.bounds = Rect::new(x, y, available_w, 1.0);
        }
        NodeKind::Spacer => {
            // Spacer size is determined by the parent arrange function.
            // bounds already set above if parent is VStack/HStack.
        }
        _ => {}
    }
}

fn arrange_vstack(
    node: &mut LayoutNode,
    x: f32,
    y: f32,
    available_w: f32,
    available_h: f32,
    text_engine: &mut TextEngine,
    theme: &Theme,
) {
    let gap = get_gap_from_props(&node.props, theme);
    node.bounds = Rect::new(x, y, available_w, available_h);
    arrange_children_vertical(node, x, y, available_w, available_h, gap, text_engine, theme);
}

fn arrange_children_vertical(
    node: &mut LayoutNode,
    x: f32,
    y: f32,
    available_w: f32,
    available_h: f32,
    gap: f32,
    text_engine: &mut TextEngine,
    theme: &Theme,
) {
    // We need to temporarily take children out to avoid borrow issues.
    let mut children = std::mem::take(&mut node.children);
    let n = children.len();

    if n == 0 {
        node.children = children;
        return;
    }

    // Build a parallel ViewNode for measurement.
    // We can just measure using the child's props/kind directly.
    let mut child_heights = Vec::with_capacity(n);
    let mut spacer_indices = Vec::new();
    let mut total_fixed = 0.0f32;

    for (i, child) in children.iter().enumerate() {
        if is_spacer_node(child) {
            child_heights.push(0.0);
            spacer_indices.push(i);
        } else {
            let sz = measure_layout_node(child, available_w, text_engine, theme);
            child_heights.push(sz.height);
            total_fixed += sz.height;
        }
    }

    let gaps_total = if n > 1 { (n - 1) as f32 * gap } else { 0.0 };
    let remaining = (available_h - total_fixed - gaps_total).max(0.0);
    let spacer_h = if spacer_indices.is_empty() {
        0.0
    } else {
        remaining / spacer_indices.len() as f32
    };

    for &i in &spacer_indices {
        child_heights[i] = spacer_h;
    }

    let mut cy = y;
    for (i, child) in children.iter_mut().enumerate() {
        let ch = child_heights[i];
        arrange(child, x, cy, available_w, ch, text_engine, theme);
        cy += ch + gap;
    }

    node.children = children;
}

fn arrange_hstack(
    node: &mut LayoutNode,
    x: f32,
    y: f32,
    available_w: f32,
    available_h: f32,
    text_engine: &mut TextEngine,
    theme: &Theme,
) {
    let gap = get_gap_from_props(&node.props, theme);
    node.bounds = Rect::new(x, y, available_w, available_h);

    let mut children = std::mem::take(&mut node.children);
    let n = children.len();

    if n == 0 {
        node.children = children;
        return;
    }

    let mut child_widths = Vec::with_capacity(n);
    let mut spacer_indices = Vec::new();
    let mut total_fixed = 0.0f32;

    for (i, child) in children.iter().enumerate() {
        if is_spacer_node(child) {
            child_widths.push(0.0);
            spacer_indices.push(i);
        } else {
            let sz = measure_layout_node(child, available_w, text_engine, theme);
            child_widths.push(sz.width);
            total_fixed += sz.width;
        }
    }

    let gaps_total = if n > 1 { (n - 1) as f32 * gap } else { 0.0 };
    let remaining = (available_w - total_fixed - gaps_total).max(0.0);
    let spacer_w = if spacer_indices.is_empty() {
        0.0
    } else {
        remaining / spacer_indices.len() as f32
    };

    for &i in &spacer_indices {
        child_widths[i] = spacer_w;
    }

    let mut cx = x;
    for (i, child) in children.iter_mut().enumerate() {
        let cw = child_widths[i];
        arrange(child, cx, y, cw, available_h, text_engine, theme);
        cx += cw + gap;
    }

    node.children = children;
}

/// Measure a LayoutNode's intrinsic size (works from LayoutNode, not ViewNode).
fn measure_layout_node(
    node: &LayoutNode,
    max_width: f32,
    text_engine: &mut TextEngine,
    theme: &Theme,
) -> IntrinsicSize {
    if node.kind == NodeKind::Spacer {
        let is_hidden = node
            .props
            .get("hidden")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        return IntrinsicSize {
            width: 0.0,
            height: 0.0,
            is_spacer: !is_hidden,
        };
    }

    match &node.kind {
        NodeKind::Text => {
            let content = node
                .props
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let is_heading = node
                .props
                .get("style")
                .and_then(|v| v.as_str())
                .map(|s| s == "heading")
                .unwrap_or(false);
            let font_size = if is_heading {
                theme.heading_size
            } else {
                theme.font_size
            };
            let (w, h) = text_engine.measure(content, font_size, max_width);
            IntrinsicSize {
                width: w,
                height: h,
                is_spacer: false,
            }
        }
        NodeKind::Button => {
            let label = node
                .props
                .get("label")
                .and_then(|v| v.as_str())
                .unwrap_or("Button");
            let (tw, th) = text_engine.measure(label, theme.font_size, max_width);
            IntrinsicSize {
                width: tw + theme.button_pad_h * 2.0,
                height: th + theme.button_pad_v * 2.0,
                is_spacer: false,
            }
        }
        NodeKind::Input => IntrinsicSize {
            width: max_width,
            height: theme.input_height,
            is_spacer: false,
        },
        NodeKind::Divider => IntrinsicSize {
            width: max_width,
            height: 1.0,
            is_spacer: false,
        },
        NodeKind::VStack => {
            let gap = get_gap_from_props(&node.props, theme);
            let mut max_w = 0.0f32;
            let mut total_h = 0.0f32;
            let child_count = node.children.len();
            for child in &node.children {
                let sz = measure_layout_node(child, max_width, text_engine, theme);
                max_w = max_w.max(sz.width);
                if !sz.is_spacer {
                    total_h += sz.height;
                }
            }
            let gaps = if child_count > 1 {
                (child_count - 1) as f32 * gap
            } else {
                0.0
            };
            IntrinsicSize {
                width: max_w,
                height: total_h + gaps,
                is_spacer: false,
            }
        }
        NodeKind::HStack => {
            let gap = get_gap_from_props(&node.props, theme);
            let mut total_w = 0.0f32;
            let mut max_h = 0.0f32;
            let child_count = node.children.len();
            for child in &node.children {
                let sz = measure_layout_node(child, max_width, text_engine, theme);
                if !sz.is_spacer {
                    total_w += sz.width;
                }
                max_h = max_h.max(sz.height);
            }
            let gaps = if child_count > 1 {
                (child_count - 1) as f32 * gap
            } else {
                0.0
            };
            IntrinsicSize {
                width: total_w + gaps,
                height: max_h,
                is_spacer: false,
            }
        }
        NodeKind::Container => {
            let inner_w = max_width - theme.padding * 2.0;
            let mut max_w = 0.0f32;
            let mut total_h = 0.0f32;
            for child in &node.children {
                let sz = measure_layout_node(child, inner_w, text_engine, theme);
                max_w = max_w.max(sz.width);
                total_h += sz.height;
            }
            IntrinsicSize {
                width: max_w + theme.padding * 2.0,
                height: total_h + theme.padding * 2.0,
                is_spacer: false,
            }
        }
        _ => IntrinsicSize {
            width: 0.0,
            height: 0.0,
            is_spacer: false,
        },
    }
}

fn is_spacer_node(node: &LayoutNode) -> bool {
    if node.kind != NodeKind::Spacer {
        return false;
    }
    // Hidden spacers (from visible=false) don't flex.
    !node
        .props
        .get("hidden")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

fn get_gap_from_props(props: &Props, theme: &Theme) -> f32 {
    props
        .get("gap")
        .and_then(|v| v.as_int())
        .map(|g| g as f32)
        .unwrap_or(theme.default_gap)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn text_node(id: NodeId, content: &str) -> ViewNode {
        ViewNode::new(id, NodeKind::Text)
            .with_prop("content", PropValue::String(content.to_string()))
    }

    fn spacer_node(id: NodeId) -> ViewNode {
        ViewNode::new(id, NodeKind::Spacer)
    }

    fn button_node(id: NodeId, label: &str) -> ViewNode {
        ViewNode::new(id, NodeKind::Button)
            .with_prop("label", PropValue::String(label.to_string()))
    }

    #[test]
    fn test_simple_vstack_layout() {
        let mut text_engine = TextEngine::new();
        let theme = Theme::default();

        let tree = ViewTree {
            root: ViewNode {
                id: 1,
                kind: NodeKind::VStack,
                props: {
                    let mut p = BTreeMap::new();
                    p.insert("gap".to_string(), PropValue::Int(8));
                    p
                },
                children: vec![
                    text_node(2, "Hello"),
                    text_node(3, "World"),
                ],
                callbacks: vec![],
            },
        };

        let result = layout(&tree, (400.0, 800.0), &mut text_engine, &theme);

        assert_eq!(result.node_id, 1);
        assert_eq!(result.children.len(), 2);
        // First child starts at y=0
        assert_eq!(result.children[0].bounds.y, 0.0);
        // Second child starts below first + gap
        assert!(result.children[1].bounds.y > result.children[0].bounds.y);
    }

    #[test]
    fn test_hstack_layout() {
        let mut text_engine = TextEngine::new();
        let theme = Theme::default();

        let tree = ViewTree {
            root: ViewNode {
                id: 1,
                kind: NodeKind::HStack,
                props: {
                    let mut p = BTreeMap::new();
                    p.insert("gap".to_string(), PropValue::Int(8));
                    p
                },
                children: vec![
                    button_node(2, "+"),
                    button_node(3, "-"),
                ],
                callbacks: vec![],
            },
        };

        let result = layout(&tree, (400.0, 800.0), &mut text_engine, &theme);

        assert_eq!(result.children.len(), 2);
        // Both start at y=0
        assert_eq!(result.children[0].bounds.y, 0.0);
        assert_eq!(result.children[1].bounds.y, 0.0);
        // Second is to the right of first
        assert!(result.children[1].bounds.x > result.children[0].bounds.x);
    }

    #[test]
    fn test_spacer_absorbs_space() {
        let mut text_engine = TextEngine::new();
        let theme = Theme::default();

        let tree = ViewTree {
            root: ViewNode {
                id: 1,
                kind: NodeKind::VStack,
                props: BTreeMap::new(),
                children: vec![
                    text_node(2, "Top"),
                    spacer_node(3),
                    text_node(4, "Bottom"),
                ],
                callbacks: vec![],
            },
        };

        let result = layout(&tree, (400.0, 800.0), &mut text_engine, &theme);

        // The bottom text should be pushed toward the bottom of the viewport.
        let bottom_y = result.children[2].bounds.y;
        assert!(bottom_y > 400.0, "Bottom text should be pushed down by spacer, got y={}", bottom_y);
    }

    #[test]
    fn test_input_full_width() {
        let mut text_engine = TextEngine::new();
        let theme = Theme::default();

        let tree = ViewTree {
            root: ViewNode::new(1, NodeKind::Input)
                .with_prop("placeholder", PropValue::String("Type here".to_string())),
        };

        let result = layout(&tree, (400.0, 800.0), &mut text_engine, &theme);

        assert_eq!(result.bounds.w, 400.0);
        assert_eq!(result.bounds.h, theme.input_height);
    }

    #[test]
    fn test_counter_app_layout() {
        let mut text_engine = TextEngine::new();
        let theme = Theme::default();

        // Simulate the counter.zml layout structure.
        let tree = ViewTree {
            root: ViewNode {
                id: 1,
                kind: NodeKind::VStack,
                props: {
                    let mut p = BTreeMap::new();
                    p.insert("gap".to_string(), PropValue::Int(12));
                    p
                },
                children: vec![
                    text_node(2, "Count: 0").with_prop("style", PropValue::String("heading".to_string())),
                    ViewNode {
                        id: 3,
                        kind: NodeKind::HStack,
                        props: {
                            let mut p = BTreeMap::new();
                            p.insert("gap".to_string(), PropValue::Int(8));
                            p
                        },
                        children: vec![
                            button_node(4, "+"),
                            button_node(5, "-"),
                        ],
                        callbacks: vec![],
                    },
                ],
                callbacks: vec![],
            },
        };

        let result = layout(&tree, (400.0, 800.0), &mut text_engine, &theme);

        // Root VStack fills viewport.
        assert_eq!(result.bounds.w, 400.0);
        // Heading text measured.
        assert!(result.children[0].bounds.h > 0.0);
        // HStack with two buttons.
        let hstack = &result.children[1];
        assert_eq!(hstack.children.len(), 2);
        // Buttons side by side.
        assert!(hstack.children[1].bounds.x > hstack.children[0].bounds.x);
    }
}
