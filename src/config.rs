/// Auto-detect MCP server configurations from Cursor, Claude Desktop, etc.
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub command: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Mode {
    Discover,
    Passthrough,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Preload {
    All,
    None,
    Some(Vec<String>),
}

#[derive(Debug, Clone)]
pub struct ProxyConfig {
    pub servers: HashMap<String, ServerConfig>,
    pub mode: Mode,
    pub preload: Preload,
    pub idle_timeout_ms: u64,
    pub preload_delay_ms: u64,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            servers: HashMap::new(),
            mode: Mode::Discover,
            preload: Preload::All,
            idle_timeout_ms: 5 * 60 * 1000,
            preload_delay_ms: 200,
        }
    }
}

/// Get all known config file paths across platforms.
fn get_config_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Some(home) = dirs::home_dir() {
        // Cursor IDE
        paths.push(home.join(".cursor").join("mcp.json"));

        // Claude Desktop
        if cfg!(target_os = "macos") {
            if let Some(support) = dirs::data_dir() {
                paths.push(support.join("Claude").join("claude_desktop_config.json"));
            }
            // Also try explicit path
            paths.push(
                home.join("Library")
                    .join("Application Support")
                    .join("Claude")
                    .join("claude_desktop_config.json"),
            );
        } else if cfg!(target_os = "windows") {
            if let Some(appdata) = dirs::config_dir() {
                paths.push(appdata.join("Claude").join("claude_desktop_config.json"));
            }
        } else {
            paths.push(
                home.join(".config")
                    .join("claude")
                    .join("claude_desktop_config.json"),
            );
        }

        // Windsurf
        paths.push(home.join(".codeium").join("windsurf").join("mcp_config.json"));

        // VS Code
        paths.push(home.join(".vscode").join("mcp.json"));
    }

    paths
}

/// Parse a single config file and extract server definitions.
fn parse_config_file(path: &PathBuf) -> HashMap<String, ServerConfig> {
    let mut result = HashMap::new();

    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return result,
    };

    let json: Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return result,
    };

    // Handle both { "mcpServers": {...} } and { "servers": {...} } formats
    let servers_obj = json
        .get("mcpServers")
        .or_else(|| json.get("servers"))
        .unwrap_or(&json);

    let servers = match servers_obj.as_object() {
        Some(m) => m,
        None => return result,
    };

    for (name, config) in servers {
        // Skip disabled servers (underscore prefix)
        if name.starts_with('_') {
            continue;
        }

        // Skip our own proxy entry
        if let Some(cmd) = config.get("command").and_then(|v| v.as_str()) {
            if let Some(args) = config.get("args").and_then(|v| v.as_array()) {
                let is_self = args.iter().any(|a| {
                    a.as_str()
                        .map(|s| s.contains("mcp-on-demand"))
                        .unwrap_or(false)
                });
                if is_self {
                    continue;
                }
            }

            let args: Vec<String> = config
                .get("args")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();

            let env: HashMap<String, String> = config
                .get("env")
                .and_then(|v| v.as_object())
                .map(|obj| {
                    obj.iter()
                        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                        .collect()
                })
                .unwrap_or_default();

            result.insert(
                name.clone(),
                ServerConfig {
                    command: cmd.to_string(),
                    args,
                    env,
                },
            );
        }
    }

    result
}

/// Auto-detect all MCP servers from known config files.
pub fn auto_detect() -> ProxyConfig {
    let mut config = ProxyConfig::default();
    let paths = get_config_paths();

    for path in &paths {
        if path.exists() {
            let servers = parse_config_file(path);
            if !servers.is_empty() {
                eprintln!(
                    "[mcp-on-demand][INFO] Found {} servers in {}",
                    servers.len(),
                    path.display()
                );
                config.servers.extend(servers);
            }
        }
    }

    let total = config.servers.len();
    if total == 0 {
        eprintln!("[mcp-on-demand][WARN] No MCP servers found. Add servers to ~/.cursor/mcp.json");
    } else {
        eprintln!("[mcp-on-demand][INFO] Total: {} MCP servers detected", total);
    }

    // Override from env
    if let Ok(mode) = std::env::var("MCP_ON_DEMAND_MODE") {
        config.mode = match mode.as_str() {
            "passthrough" => Mode::Passthrough,
            _ => Mode::Discover,
        };
    }

    if let Ok(preload) = std::env::var("MCP_ON_DEMAND_PRELOAD") {
        config.preload = match preload.as_str() {
            "none" => Preload::None,
            _ => Preload::All,
        };
    }

    config
}
