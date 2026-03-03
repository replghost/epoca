use std::collections::BTreeMap;

use crate::ast::*;
use crate::state::{StateError, StateStore};
use epoca_protocol::*;

/// Maximum actions per handler invocation (DoS protection).
const MAX_STEPS: usize = 1000;

#[derive(Debug, Clone)]
pub enum EvalError {
    DivisionByZero,
    StepLimitExceeded,
    TypeError(String),
    StateError(StateError),
}

impl std::fmt::Display for EvalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EvalError::DivisionByZero => write!(f, "division by zero"),
            EvalError::StepLimitExceeded => write!(f, "action step limit exceeded ({MAX_STEPS})"),
            EvalError::TypeError(msg) => write!(f, "type error: {msg}"),
            EvalError::StateError(e) => write!(f, "state error: {e}"),
        }
    }
}

impl From<StateError> for EvalError {
    fn from(e: StateError) -> Self {
        EvalError::StateError(e)
    }
}

/// Counter for assigning unique node IDs and callback IDs during evaluation.
struct IdGen {
    next_node: NodeId,
    next_cb: CallbackId,
}

impl IdGen {
    fn new() -> Self {
        Self {
            next_node: 1,
            next_cb: 1,
        }
    }

    fn node_id(&mut self) -> NodeId {
        let id = self.next_node;
        self.next_node += 1;
        id
    }

    fn callback_id(&mut self) -> CallbackId {
        let id = self.next_cb;
        self.next_cb += 1;
        id
    }
}

/// Mapping from callback_id → (handler_index_in_node_path)
/// We store enough info to find the handler when an event fires.
#[derive(Debug, Clone)]
pub struct CallbackEntry {
    pub callback_id: CallbackId,
    pub actions: Vec<Action>,
}

/// Result of evaluating a ZML app.
pub struct EvalResult {
    pub tree: ViewTree,
    pub callbacks: Vec<CallbackEntry>,
}

/// Evaluate a ZML app AST with the given state, producing a ViewTree.
pub fn eval_app(app: &ZmlApp, state: &StateStore) -> EvalResult {
    let mut ids = IdGen::new();
    let mut callbacks = Vec::new();

    // Build the root node — if multiple body nodes, wrap in VStack
    let root = if app.body.len() == 1 {
        eval_node(&app.body[0], state, &mut ids, &mut callbacks)
    } else {
        let children: Vec<ViewNode> = app
            .body
            .iter()
            .filter_map(|n| {
                let node = eval_node(n, state, &mut ids, &mut callbacks);
                Some(node)
            })
            .collect();
        ViewNode {
            id: ids.node_id(),
            kind: NodeKind::VStack,
            props: BTreeMap::new(),
            children,
            callbacks: Vec::new(),
        }
    };

    EvalResult {
        tree: ViewTree { root },
        callbacks,
    }
}

fn eval_node(
    node: &Node,
    state: &StateStore,
    ids: &mut IdGen,
    callbacks: &mut Vec<CallbackEntry>,
) -> ViewNode {
    match node {
        Node::Element {
            kind,
            props,
            children,
            handlers,
        } => {
            // Check visible prop
            let visible = props.iter().find(|p| p.key == "visible");
            if let Some(vis_prop) = visible {
                let val = eval_expr(&vis_prop.value, state);
                if !val.is_truthy() {
                    // Return an empty spacer with zero size (invisible)
                    return ViewNode {
                        id: ids.node_id(),
                        kind: NodeKind::Spacer,
                        props: {
                            let mut p = BTreeMap::new();
                            p.insert("hidden".to_string(), PropValue::Bool(true));
                            p
                        },
                        children: Vec::new(),
                        callbacks: Vec::new(),
                    };
                }
            }

            let node_kind = match kind.as_str() {
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
                _ => NodeKind::Container,
            };

            let node_id = ids.node_id();

            // Evaluate props
            let mut view_props = BTreeMap::new();
            for prop in props {
                if prop.key == "visible" {
                    continue; // Already handled
                }

                // Handle bind= specially for Input: set up value and placeholder
                if prop.key == "bind" {
                    if let Expr::Literal(ZmlValue::Str(var_name)) = &prop.value {
                        // Get current value from state
                        if let Some(val) = state.get_key(var_name) {
                            view_props.insert(
                                "value".to_string(),
                                PropValue::String(val.to_display_string()),
                            );
                        }
                        view_props.insert(
                            "bind".to_string(),
                            PropValue::String(var_name.clone()),
                        );
                    }
                    continue;
                }

                let val = eval_expr(&prop.value, state);
                view_props.insert(prop.key.clone(), zml_to_prop(&val));
            }

            // Evaluate children
            let view_children: Vec<ViewNode> = children
                .iter()
                .map(|c| eval_node(c, state, ids, callbacks))
                .collect();

            // Register callbacks for handlers
            let mut view_callbacks = Vec::new();
            for handler in handlers {
                let cb_id = ids.callback_id();
                let event_kind = match handler.event.as_str() {
                    "click" => EventKind::Click,
                    "input" => EventKind::Input,
                    "submit" => EventKind::Submit,
                    "change" => EventKind::Change,
                    "focus" => EventKind::Focus,
                    "blur" => EventKind::Blur,
                    _ => continue,
                };
                view_callbacks.push(Callback {
                    id: cb_id,
                    event: event_kind,
                });
                callbacks.push(CallbackEntry {
                    callback_id: cb_id,
                    actions: handler.actions.clone(),
                });
            }

            // For Input with bind= prop, auto-register an input callback
            if node_kind == NodeKind::Input {
                if let Some(bind_val) = view_props.get("bind") {
                    if let PropValue::String(_) = bind_val {
                        let cb_id = ids.callback_id();
                        view_callbacks.push(Callback {
                            id: cb_id,
                            event: EventKind::Input,
                        });
                        // The bind callback has a special synthetic action
                        // that will be handled by the evaluator
                        callbacks.push(CallbackEntry {
                            callback_id: cb_id,
                            actions: Vec::new(), // marker: empty actions = bind callback
                        });
                    }
                }
            }

            ViewNode {
                id: node_id,
                kind: node_kind,
                props: view_props,
                children: view_children,
                callbacks: view_callbacks,
            }
        }
    }
}

/// Evaluate an expression against state.
pub fn eval_expr(expr: &Expr, state: &StateStore) -> ZmlValue {
    match expr {
        Expr::Literal(v) => v.clone(),
        Expr::Path(segments) => state
            .get(segments)
            .cloned()
            .unwrap_or(ZmlValue::Null),
        Expr::BinOp(left, op, right) => {
            let l = eval_expr(left, state);
            let r = eval_expr(right, state);
            eval_binop(&l, *op, &r)
        }
        Expr::Interpolated(parts) => {
            let mut result = String::new();
            for part in parts {
                match part {
                    InterpolPart::Literal(s) => result.push_str(s),
                    InterpolPart::Expr(e) => {
                        let val = eval_expr(e, state);
                        result.push_str(&val.to_display_string());
                    }
                }
            }
            ZmlValue::Str(result)
        }
        Expr::Negate(inner) => {
            let val = eval_expr(inner, state);
            match val {
                ZmlValue::Int(n) => ZmlValue::Int(-n),
                ZmlValue::Float(f) => ZmlValue::Float(-f),
                _ => ZmlValue::Null,
            }
        }
    }
}

fn eval_binop(left: &ZmlValue, op: BinOp, right: &ZmlValue) -> ZmlValue {
    // Numeric operations
    if let (Some(l), Some(r)) = (left.as_f64(), right.as_f64()) {
        let is_int = matches!(left, ZmlValue::Int(_)) && matches!(right, ZmlValue::Int(_));

        let result = match op {
            BinOp::Add => l + r,
            BinOp::Sub => l - r,
            BinOp::Mul => l * r,
            BinOp::Div => {
                if r == 0.0 {
                    return ZmlValue::Null; // Division by zero → Null
                }
                l / r
            }
            BinOp::Mod => {
                if r == 0.0 {
                    return ZmlValue::Null;
                }
                l % r
            }
            BinOp::Eq => return ZmlValue::Bool((l - r).abs() < f64::EPSILON),
            BinOp::Ne => return ZmlValue::Bool((l - r).abs() >= f64::EPSILON),
            BinOp::Lt => return ZmlValue::Bool(l < r),
            BinOp::Gt => return ZmlValue::Bool(l > r),
            BinOp::Le => return ZmlValue::Bool(l <= r),
            BinOp::Ge => return ZmlValue::Bool(l >= r),
        };

        if is_int && result == (result as i64) as f64 {
            ZmlValue::Int(result as i64)
        } else {
            ZmlValue::Float(result)
        }
    } else {
        // String concatenation for Add
        if op == BinOp::Add {
            if let (ZmlValue::Str(l), ZmlValue::Str(r)) = (left, right) {
                return ZmlValue::Str(format!("{l}{r}"));
            }
        }
        // Equality for all types
        match op {
            BinOp::Eq => ZmlValue::Bool(left == right),
            BinOp::Ne => ZmlValue::Bool(left != right),
            _ => ZmlValue::Null,
        }
    }
}

fn zml_to_prop(val: &ZmlValue) -> PropValue {
    match val {
        ZmlValue::Null => PropValue::String("null".to_string()),
        ZmlValue::Bool(b) => PropValue::Bool(*b),
        ZmlValue::Int(n) => PropValue::Int(*n),
        ZmlValue::Float(f) => PropValue::Float(*f),
        ZmlValue::Str(s) => PropValue::String(s.clone()),
        ZmlValue::List(items) => PropValue::List(items.iter().map(zml_to_prop).collect()),
        ZmlValue::Map(_) => PropValue::String("{...}".to_string()),
    }
}

/// Execute actions from a handler, mutating state.
pub fn exec_actions(
    actions: &[Action],
    state: &mut StateStore,
    _event_data: &EventData,
) -> Result<(), EvalError> {
    let mut steps = 0;

    for action in actions {
        steps += 1;
        if steps > MAX_STEPS {
            return Err(EvalError::StepLimitExceeded);
        }

        match action {
            Action::Set { path, value } => {
                let val = eval_expr(value, state);
                state.set(path, val)?;
            }
        }
    }

    Ok(())
}

/// Handle a bind callback — update the bound state variable from input text.
pub fn handle_bind(
    node_props: &BTreeMap<String, PropValue>,
    state: &mut StateStore,
    event_data: &EventData,
) {
    if let Some(PropValue::String(var_name)) = node_props.get("bind") {
        if let EventData::Text(text) = event_data {
            // Try to parse as number if the current state value is numeric
            let new_val = if let Some(current) = state.get_key(var_name) {
                match current {
                    ZmlValue::Int(_) => text
                        .parse::<i64>()
                        .map(ZmlValue::Int)
                        .unwrap_or_else(|_| {
                            text.parse::<f64>()
                                .map(ZmlValue::Float)
                                .unwrap_or(ZmlValue::Str(text.clone()))
                        }),
                    ZmlValue::Float(_) => text
                        .parse::<f64>()
                        .map(ZmlValue::Float)
                        .unwrap_or(ZmlValue::Str(text.clone())),
                    _ => ZmlValue::Str(text.clone()),
                }
            } else {
                ZmlValue::Str(text.clone())
            };
            let _ = state.set(&[var_name.clone()], new_val);
        }
    }
}

/// Initialize state from the state block expressions.
pub fn init_state(state_block: &[(String, Expr)], state: &mut StateStore) {
    let bindings: Vec<(String, ZmlValue)> = state_block
        .iter()
        .map(|(name, expr)| {
            let val = eval_expr(expr, state);
            (name.clone(), val)
        })
        .collect();
    state.init(bindings);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_state(pairs: Vec<(&str, ZmlValue)>) -> StateStore {
        let mut s = StateStore::new();
        for (k, v) in pairs {
            s.set(&[k.to_string()], v).unwrap();
        }
        s
    }

    #[test]
    fn eval_literal() {
        let state = StateStore::new();
        let val = eval_expr(&Expr::Literal(ZmlValue::Int(42)), &state);
        assert_eq!(val, ZmlValue::Int(42));
    }

    #[test]
    fn eval_path() {
        let state = make_state(vec![("count", ZmlValue::Int(5))]);
        let val = eval_expr(&Expr::Path(vec!["count".to_string()]), &state);
        assert_eq!(val, ZmlValue::Int(5));
    }

    #[test]
    fn eval_addition() {
        let state = make_state(vec![("count", ZmlValue::Int(5))]);
        let expr = Expr::BinOp(
            Box::new(Expr::Path(vec!["count".to_string()])),
            BinOp::Add,
            Box::new(Expr::Literal(ZmlValue::Int(1))),
        );
        assert_eq!(eval_expr(&expr, &state), ZmlValue::Int(6));
    }

    #[test]
    fn eval_multiplication() {
        let state = make_state(vec![("a", ZmlValue::Int(100))]);
        let expr = Expr::BinOp(
            Box::new(Expr::Path(vec!["a".to_string()])),
            BinOp::Mul,
            Box::new(Expr::Literal(ZmlValue::Float(1.35))),
        );
        assert_eq!(eval_expr(&expr, &state), ZmlValue::Float(135.0));
    }

    #[test]
    fn eval_division_by_zero() {
        let state = StateStore::new();
        let expr = Expr::BinOp(
            Box::new(Expr::Literal(ZmlValue::Int(10))),
            BinOp::Div,
            Box::new(Expr::Literal(ZmlValue::Int(0))),
        );
        assert_eq!(eval_expr(&expr, &state), ZmlValue::Null);
    }

    #[test]
    fn eval_interpolated() {
        let state = make_state(vec![("name", ZmlValue::Str("World".to_string()))]);
        let expr = Expr::Interpolated(vec![
            InterpolPart::Literal("Hello ".to_string()),
            InterpolPart::Expr(Expr::Path(vec!["name".to_string()])),
            InterpolPart::Literal("!".to_string()),
        ]);
        assert_eq!(
            eval_expr(&expr, &state),
            ZmlValue::Str("Hello World!".to_string())
        );
    }

    #[test]
    fn eval_app_produces_view_tree() {
        let app = ZmlApp {
            permissions: None,
            state_block: vec![],
            body: vec![Node::Element {
                kind: "Text".to_string(),
                props: vec![Prop {
                    key: "content".to_string(),
                    value: Expr::Literal(ZmlValue::Str("hello".to_string())),
                }],
                children: vec![],
                handlers: vec![],
            }],
        };
        let state = StateStore::new();
        let result = eval_app(&app, &state);
        assert_eq!(result.tree.root.kind, NodeKind::Text);
        assert_eq!(
            result.tree.root.props.get("content"),
            Some(&PropValue::String("hello".to_string()))
        );
    }

    #[test]
    fn eval_visible_false_hides_node() {
        let state = make_state(vec![("show", ZmlValue::Bool(false))]);
        let app = ZmlApp {
            permissions: None,
            state_block: vec![],
            body: vec![Node::Element {
                kind: "Text".to_string(),
                props: vec![Prop {
                    key: "visible".to_string(),
                    value: Expr::Path(vec!["show".to_string()]),
                }],
                children: vec![],
                handlers: vec![],
            }],
        };
        let result = eval_app(&app, &state);
        // Hidden nodes become invisible spacers
        assert_eq!(result.tree.root.kind, NodeKind::Spacer);
    }

    #[test]
    fn exec_actions_set() {
        let mut state = make_state(vec![("count", ZmlValue::Int(0))]);
        let actions = vec![Action::Set {
            path: vec!["count".to_string()],
            value: Expr::BinOp(
                Box::new(Expr::Path(vec!["count".to_string()])),
                BinOp::Add,
                Box::new(Expr::Literal(ZmlValue::Int(1))),
            ),
        }];
        exec_actions(&actions, &mut state, &EventData::None).unwrap();
        assert_eq!(state.get(&["count".to_string()]), Some(&ZmlValue::Int(1)));
    }

    #[test]
    fn full_pipeline_counter() {
        let src = r#"state
  count = 0

VStack gap=12
  Text "Count: {count}" style=heading
  Button "+" variant=primary
    on click
      count = count + 1
"#;
        let app = crate::parser::parse(src).unwrap();
        let mut state = StateStore::new();
        init_state(&app.state_block, &mut state);

        // Initial render
        let result = eval_app(&app, &state);
        assert_eq!(result.tree.root.kind, NodeKind::VStack);

        // Simulate click on "+"
        let click_cb = result.callbacks.iter().find(|c| !c.actions.is_empty()).unwrap();
        exec_actions(&click_cb.actions, &mut state, &EventData::None).unwrap();

        assert_eq!(state.get(&["count".to_string()]), Some(&ZmlValue::Int(1)));

        // Re-render
        let result2 = eval_app(&app, &state);
        let text_node = &result2.tree.root.children[0];
        assert_eq!(
            text_node.props.get("content"),
            Some(&PropValue::String("Count: 1".to_string()))
        );
    }
}
