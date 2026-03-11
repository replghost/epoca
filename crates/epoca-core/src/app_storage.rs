//! Per-app key-value storage backed by a JSON file on disk.
//!
//! Each app gets its own file at `~/.epoca/apps/{app_id}/storage.json`.
//! The file is a flat JSON object: `{ "key1": "value1", "key2": "value2" }`.
//!
//! Operations read-modify-write the JSON file on every call (no caching).
//!
// TODO: encrypt storage file at rest

use std::collections::HashMap;
use std::path::PathBuf;

/// Returns the storage file path for the given app.
fn storage_path(app_id: &str) -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_default();
    PathBuf::from(home)
        .join(".epoca")
        .join("apps")
        .join(app_id)
        .join("storage.json")
}

/// Read the storage file for the given app, returning an empty map on any error.
fn read_storage(app_id: &str) -> HashMap<String, String> {
    let path = storage_path(app_id);
    let Ok(data) = std::fs::read_to_string(&path) else {
        return HashMap::new();
    };
    serde_json::from_str::<HashMap<String, String>>(&data).unwrap_or_default()
}

/// Write the storage map back to disk, creating directories as needed.
fn write_storage(app_id: &str, map: &HashMap<String, String>) -> Result<(), String> {
    let path = storage_path(app_id);
    // Create parent directory if it doesn't exist.
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("storage: failed to create directory: {e}"))?;
    }
    let json = serde_json::to_string(map)
        .map_err(|e| format!("storage: failed to serialize: {e}"))?;
    std::fs::write(&path, json)
        .map_err(|e| format!("storage: failed to write file: {e}"))
}

/// Get the value for the given key. Returns `None` if the key does not exist.
pub fn get(app_id: &str, key: &str) -> Option<String> {
    read_storage(app_id).remove(key)
}

/// Set the value for the given key, creating the storage file if it doesn't exist.
pub fn set(app_id: &str, key: &str, value: &str) -> Result<(), String> {
    let mut map = read_storage(app_id);
    map.insert(key.to_string(), value.to_string());
    write_storage(app_id, &map)
}

/// Remove the given key from storage. Succeeds even if the key did not exist.
pub fn remove(app_id: &str, key: &str) -> Result<(), String> {
    let mut map = read_storage(app_id);
    map.remove(key);
    write_storage(app_id, &map)
}
