use indexmap::IndexMap;

use crate::ast::ZmlValue;

/// Maximum total state size in bytes (10MB).
const MAX_STATE_SIZE: usize = 10 * 1024 * 1024;
/// Maximum single string value size (1MB).
const MAX_STRING_SIZE: usize = 1024 * 1024;

/// Reactive state store for a ZML app.
#[derive(Debug, Clone)]
pub struct StateStore {
    root: IndexMap<String, ZmlValue>,
    dirty: bool,
}

impl StateStore {
    pub fn new() -> Self {
        Self {
            root: IndexMap::new(),
            dirty: false,
        }
    }

    /// Initialize from a list of (key, value) pairs.
    pub fn init(&mut self, bindings: Vec<(String, ZmlValue)>) {
        self.root.clear();
        for (k, v) in bindings {
            self.root.insert(k, v);
        }
        self.dirty = true;
    }

    /// Get a value by dotted path segments.
    pub fn get(&self, path: &[String]) -> Option<&ZmlValue> {
        if path.is_empty() {
            return None;
        }
        let mut current = self.root.get(&path[0])?;
        for segment in &path[1..] {
            match current {
                ZmlValue::Map(m) => {
                    current = m.get(segment)?;
                }
                _ => return None,
            }
        }
        Some(current)
    }

    /// Get a single top-level value.
    pub fn get_key(&self, key: &str) -> Option<&ZmlValue> {
        self.root.get(key)
    }

    /// Set a value by dotted path segments, creating intermediate maps as needed.
    pub fn set(&mut self, path: &[String], value: ZmlValue) -> Result<(), StateError> {
        if path.is_empty() {
            return Err(StateError::EmptyPath);
        }

        // Enforce string size limit
        if let ZmlValue::Str(ref s) = value {
            if s.len() > MAX_STRING_SIZE {
                return Err(StateError::StringTooLarge);
            }
        }

        if path.len() == 1 {
            self.root.insert(path[0].clone(), value);
        } else {
            // Walk/create intermediate maps
            let entry = self
                .root
                .entry(path[0].clone())
                .or_insert_with(|| ZmlValue::Map(IndexMap::new()));

            set_nested(entry, &path[1..], value)?;
        }

        // Rough size check
        if self.estimate_size() > MAX_STATE_SIZE {
            return Err(StateError::StateTooLarge);
        }

        self.dirty = true;
        Ok(())
    }

    /// Check and clear dirty flag.
    pub fn take_dirty(&mut self) -> bool {
        let d = self.dirty;
        self.dirty = false;
        d
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Rough estimate of state size in memory.
    fn estimate_size(&self) -> usize {
        self.root
            .iter()
            .map(|(k, v)| k.len() + estimate_value_size(v))
            .sum()
    }
}

fn set_nested(
    current: &mut ZmlValue,
    path: &[String],
    value: ZmlValue,
) -> Result<(), StateError> {
    if path.is_empty() {
        *current = value;
        return Ok(());
    }

    match current {
        ZmlValue::Map(m) => {
            if path.len() == 1 {
                m.insert(path[0].clone(), value);
            } else {
                let entry = m
                    .entry(path[0].clone())
                    .or_insert_with(|| ZmlValue::Map(IndexMap::new()));
                set_nested(entry, &path[1..], value)?;
            }
            Ok(())
        }
        _ => {
            // Overwrite non-map with a map
            let mut m = IndexMap::new();
            if path.len() == 1 {
                m.insert(path[0].clone(), value);
            } else {
                let mut inner = ZmlValue::Map(IndexMap::new());
                set_nested(&mut inner, &path[1..], value)?;
                m.insert(path[0].clone(), inner);
            }
            *current = ZmlValue::Map(m);
            Ok(())
        }
    }
}

fn estimate_value_size(v: &ZmlValue) -> usize {
    match v {
        ZmlValue::Null => 1,
        ZmlValue::Bool(_) => 1,
        ZmlValue::Int(_) => 8,
        ZmlValue::Float(_) => 8,
        ZmlValue::Str(s) => s.len(),
        ZmlValue::List(items) => items.iter().map(estimate_value_size).sum::<usize>() + 8,
        ZmlValue::Map(m) => m
            .iter()
            .map(|(k, v)| k.len() + estimate_value_size(v))
            .sum::<usize>()
            + 8,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum StateError {
    EmptyPath,
    StringTooLarge,
    StateTooLarge,
}

impl std::fmt::Display for StateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StateError::EmptyPath => write!(f, "empty path"),
            StateError::StringTooLarge => write!(f, "string value exceeds 1MB limit"),
            StateError::StateTooLarge => write!(f, "total state exceeds 10MB limit"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_set_roundtrip() {
        let mut store = StateStore::new();
        store
            .set(&["count".to_string()], ZmlValue::Int(42))
            .unwrap();
        assert_eq!(
            store.get(&["count".to_string()]),
            Some(&ZmlValue::Int(42))
        );
    }

    #[test]
    fn nested_path() {
        let mut store = StateStore::new();
        store
            .set(
                &["response".to_string(), "temp".to_string()],
                ZmlValue::Float(24.5),
            )
            .unwrap();
        assert_eq!(
            store.get(&["response".to_string(), "temp".to_string()]),
            Some(&ZmlValue::Float(24.5))
        );
    }

    #[test]
    fn dirty_tracking() {
        let mut store = StateStore::new();
        assert!(!store.is_dirty());
        store
            .set(&["x".to_string()], ZmlValue::Int(1))
            .unwrap();
        assert!(store.is_dirty());
        assert!(store.take_dirty());
        assert!(!store.is_dirty());
    }

    #[test]
    fn init_clears_and_sets() {
        let mut store = StateStore::new();
        store
            .set(&["old".to_string()], ZmlValue::Int(99))
            .unwrap();
        store.init(vec![("new".to_string(), ZmlValue::Int(1))]);
        assert_eq!(store.get(&["old".to_string()]), None);
        assert_eq!(store.get(&["new".to_string()]), Some(&ZmlValue::Int(1)));
    }
}
