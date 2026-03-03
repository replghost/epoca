use gpui::*;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::{Input, InputState};
use gpui_component::label::Label;
use gpui_component::theme::ActiveTheme;
use std::collections::HashMap;
use epoca_protocol::*;

/// The view bridge renders a ViewTree as GPUI elements inside a tab.
/// It maintains the previous tree for diffing and routes events back to the guest.
pub struct ViewBridge {
    current_tree: Option<ViewTree>,
    /// Callback to invoke when a guest event fires.
    event_sender: Box<dyn Fn(GuestEvent) + Send + Sync>,
}

impl ViewBridge {
    pub fn new(event_sender: impl Fn(GuestEvent) + Send + Sync + 'static) -> Self {
        Self {
            current_tree: None,
            event_sender: Box::new(event_sender),
        }
    }

    /// Update the view tree. Returns patches if there was a previous tree.
    pub fn update_tree(&mut self, new_tree: ViewTree) -> Option<Vec<ViewPatch>> {
        let patches = self
            .current_tree
            .as_ref()
            .map(|old| diff_trees(&old.root, &new_tree.root));
        self.current_tree = Some(new_tree);
        patches
    }

    /// Get the current tree for rendering.
    pub fn current_tree(&self) -> Option<&ViewTree> {
        self.current_tree.as_ref()
    }

    /// Fire an event back to the guest.
    pub fn fire_event(&self, event: GuestEvent) {
        (self.event_sender)(event);
    }
}

/// A GPUI view that renders a ViewTree.
pub struct SandboxAppView {
    pub(crate) bridge: ViewBridge,
    /// Input state entities keyed by node id, so they persist across re-renders.
    input_states: HashMap<NodeId, Entity<InputState>>,
}

impl SandboxAppView {
    pub fn new(event_sender: impl Fn(GuestEvent) + Send + Sync + 'static) -> Self {
        Self {
            bridge: ViewBridge::new(event_sender),
            input_states: HashMap::new(),
        }
    }

    pub fn set_tree(&mut self, tree: ViewTree) {
        self.bridge.update_tree(tree);
    }

    fn render_node(&mut self, node: &ViewNode, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        match &node.kind {
            NodeKind::VStack => self.render_vstack(node, window, cx),
            NodeKind::HStack => self.render_hstack(node, window, cx),
            NodeKind::Text => self.render_text(node),
            NodeKind::Button => self.render_button(node, cx),
            NodeKind::Input => self.render_input(node, window, cx),
            NodeKind::Spacer => self.render_spacer(),
            NodeKind::Divider => self.render_divider(cx),
            NodeKind::Container => self.render_container(node, window, cx),
            _ => self.render_placeholder(node, cx),
        }
    }

    fn render_vstack(&mut self, node: &ViewNode, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let gap = node
            .props
            .get("gap")
            .and_then(|v| v.as_int())
            .unwrap_or(4);

        let mut el = div().flex().flex_col().gap(px(gap as f32));

        for child in &node.children {
            el = el.child(self.render_node(child, window, cx));
        }

        el.into_any_element()
    }

    fn render_hstack(&mut self, node: &ViewNode, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let gap = node
            .props
            .get("gap")
            .and_then(|v| v.as_int())
            .unwrap_or(4);

        let mut el = div().flex().flex_row().gap(px(gap as f32));

        for child in &node.children {
            el = el.child(self.render_node(child, window, cx));
        }

        el.into_any_element()
    }

    fn render_text(&self, node: &ViewNode) -> AnyElement {
        let content = node
            .props
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let is_heading = node
            .props
            .get("style")
            .and_then(|v| v.as_str())
            .map(|s| s == "heading")
            .unwrap_or(false);

        let label = Label::new(content);
        if is_heading {
            div().text_xl().child(label).into_any_element()
        } else {
            label.into_any_element()
        }
    }

    fn render_button(&self, node: &ViewNode, cx: &mut Context<Self>) -> AnyElement {
        let label = node
            .props
            .get("label")
            .and_then(|v| v.as_str())
            .unwrap_or("Button")
            .to_string();

        let is_primary = node
            .props
            .get("variant")
            .and_then(|v| v.as_str())
            .map(|s| s == "primary")
            .unwrap_or(false);

        // Find click callback
        let click_cb = node
            .callbacks
            .iter()
            .find(|cb| cb.event == EventKind::Click)
            .map(|cb| cb.id);

        let mut btn = Button::new(SharedString::from(format!("btn-{}", node.id)))
            .label(label);

        if is_primary {
            btn = btn.primary();
        }

        if let Some(cb_id) = click_cb {
            btn = btn.on_click(cx.listener(move |this, _event, _window, _cx| {
                this.bridge.fire_event(GuestEvent {
                    callback_id: cb_id,
                    kind: EventKind::Click,
                    data: EventData::None,
                });
            }));
        }

        btn.into_any_element()
    }

    fn render_spacer(&self) -> AnyElement {
        div().flex_1().into_any_element()
    }

    fn render_divider(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .h(px(1.0))
            .w_full()
            .bg(cx.theme().border)
            .into_any_element()
    }

    fn render_input(&mut self, node: &ViewNode, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let node_id = node.id;

        let placeholder = node
            .props
            .get("placeholder")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let value = node
            .props
            .get("value")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        // Find input callback
        let input_cb = node
            .callbacks
            .iter()
            .find(|cb| cb.event == EventKind::Input)
            .map(|cb| cb.id);

        // Get or create persistent InputState for this node
        let input_state = self
            .input_states
            .entry(node_id)
            .or_insert_with(|| {
                cx.new(|cx| {
                    let mut state = InputState::new(window, cx);
                    if !placeholder.is_empty() {
                        state.set_placeholder(placeholder.clone(), window, cx);
                    }
                    if let Some(ref v) = value {
                        state.set_value(v.clone(), window, cx);
                    }
                    state
                })
            })
            .clone();

        // If a value prop is set and differs from current, update it
        if let Some(ref v) = value {
            input_state.update(cx, |state, cx| {
                let current = state.value().to_string();
                if current != *v {
                    state.set_value(v.clone(), window, cx);
                }
            });
        }

        // Wire input change event
        if let Some(cb_id) = input_cb {
            let state_clone = input_state.clone();
            cx.observe(&input_state, move |this, _entity, _cx| {
                let val = state_clone.read(_cx).value().to_string();
                this.bridge.fire_event(GuestEvent {
                    callback_id: cb_id,
                    kind: EventKind::Input,
                    data: EventData::Text(val),
                });
            })
            .detach();
        }

        Input::new(&input_state).into_any_element()
    }

    fn render_container(&mut self, node: &ViewNode, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let mut el = div().p_4();

        for child in &node.children {
            el = el.child(self.render_node(child, window, cx));
        }

        el.into_any_element()
    }

    fn render_placeholder(&self, node: &ViewNode, cx: &mut Context<Self>) -> AnyElement {
        div()
            .p_2()
            .border_1()
            .border_color(cx.theme().border)
            .rounded_md()
            .child(Label::new(format!("{:?}", node.kind)))
            .into_any_element()
    }
}

impl Render for SandboxAppView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(tree) = self.bridge.current_tree().cloned() {
            div()
                .size_full()
                .p_4()
                .child(self.render_node(&tree.root, window, cx))
        } else {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(Label::new("No view loaded"))
        }
    }
}
