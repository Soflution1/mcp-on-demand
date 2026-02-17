use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use crate::protocol::ToolDef;

#[derive(Serialize, Deserialize)]
pub struct SchemaCache {
    pub version: String,
    pub servers: HashMap<String, Vec<ToolDef>>,
}

fn cache_path() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    Some(home.join(".mcp-on-demand").join("schema-cache.json"))
}

pub fn load_cache() -> Option<SchemaCache> {
    let path = cache_path()?;
    if !path.exists() { return None; }
    let content = fs::read_to_string(&path).ok()?;
    let cache: SchemaCache = serde_json::from_str(&content).ok()?;
    let total_tools: usize = cache.servers.values().map(|v| v.len()).sum();
    eprintln!("[mcp-on-demand][INFO] Loaded cache: {} servers, {} tools", cache.servers.len(), total_tools);
    Some(cache)
}

pub fn save_cache(servers: &HashMap<String, Vec<ToolDef>>) {
    let cache = SchemaCache {
        version: env!("CARGO_PKG_VERSION").to_string(),
        servers: servers.clone(),
    };
    if let Some(path) = cache_path() {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(&cache) {
            let _ = fs::write(&path, json);
            let total_tools: usize = servers.values().map(|v| v.len()).sum();
            eprintln!("[mcp-on-demand][INFO] Saved cache: {} servers, {} tools", servers.len(), total_tools);
        }
    }
}
