/// A complete parsed ZML application.
#[derive(Debug, Clone)]
pub struct ZmlApp {
    pub permissions: Option<ZmlPermissions>,
    pub state_block: Vec<(String, Expr)>,
    pub body: Vec<Node>,
}

/// Inline permissions block.
#[derive(Debug, Clone)]
pub struct ZmlPermissions {
    pub network: Vec<String>,
    pub storage: Option<String>,
    pub camera: bool,
    pub geolocation: String,
    pub gpu: String,
}

impl Default for ZmlPermissions {
    fn default() -> Self {
        Self {
            network: Vec::new(),
            storage: None,
            camera: false,
            geolocation: "none".to_string(),
            gpu: "none".to_string(),
        }
    }
}

/// A single node in the ZML tree.
#[derive(Debug, Clone)]
pub enum Node {
    Element {
        kind: String,
        props: Vec<Prop>,
        children: Vec<Node>,
        handlers: Vec<Handler>,
    },
}

/// A key=value property on an element.
#[derive(Debug, Clone)]
pub struct Prop {
    pub key: String,
    pub value: Expr,
}

/// An event handler block.
#[derive(Debug, Clone)]
pub struct Handler {
    pub event: String,
    pub actions: Vec<Action>,
}

/// An action inside an event handler.
#[derive(Debug, Clone)]
pub enum Action {
    Set { path: Vec<String>, value: Expr },
}

/// An expression that can be evaluated.
#[derive(Debug, Clone)]
pub enum Expr {
    Literal(ZmlValue),
    Path(Vec<String>),
    BinOp(Box<Expr>, BinOp, Box<Expr>),
    Interpolated(Vec<InterpolPart>),
    Negate(Box<Expr>),
}

/// Binary operators.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
}

/// A part of an interpolated string.
#[derive(Debug, Clone)]
pub enum InterpolPart {
    Literal(String),
    Expr(Expr),
}

/// Runtime values in ZML.
#[derive(Debug, Clone, PartialEq)]
pub enum ZmlValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    List(Vec<ZmlValue>),
    Map(indexmap::IndexMap<String, ZmlValue>),
}

impl ZmlValue {
    pub fn is_truthy(&self) -> bool {
        match self {
            ZmlValue::Null => false,
            ZmlValue::Bool(b) => *b,
            ZmlValue::Int(n) => *n != 0,
            ZmlValue::Float(f) => *f != 0.0,
            ZmlValue::Str(s) => !s.is_empty(),
            ZmlValue::List(l) => !l.is_empty(),
            ZmlValue::Map(m) => !m.is_empty(),
        }
    }

    pub fn to_display_string(&self) -> String {
        match self {
            ZmlValue::Null => "null".to_string(),
            ZmlValue::Bool(b) => b.to_string(),
            ZmlValue::Int(n) => n.to_string(),
            ZmlValue::Float(f) => {
                if *f == (*f as i64) as f64 {
                    format!("{}", *f as i64)
                } else {
                    f.to_string()
                }
            }
            ZmlValue::Str(s) => s.clone(),
            ZmlValue::List(_) => "[...]".to_string(),
            ZmlValue::Map(_) => "{...}".to_string(),
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            ZmlValue::Int(n) => Some(*n as f64),
            ZmlValue::Float(f) => Some(*f),
            _ => None,
        }
    }
}
