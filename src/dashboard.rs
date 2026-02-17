//! Embedded web dashboard for mcp-on-demand.
//! Serves HTML + JSON API on http://127.0.0.1:24680
//! Zero external dependencies — uses tokio::net::TcpListener directly.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

// ─── Config I/O ──────────────────────────────────────────────

fn config_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join(".mcp-on-demand")
}

fn config_path() -> PathBuf {
    config_dir().join("config.json")
}

fn cache_path() -> PathBuf {
    config_dir().join("schema-cache.json")
}

fn binary_path() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| {
        dirs::home_dir()
            .unwrap_or_default()
            .join("mcp-on-demand")
            .join("target")
            .join("release")
            .join("mcp-on-demand")
    })
}

fn read_config() -> Value {
    let path = config_path();
    if !path.exists() {
        return json!({"mcpServers": {}, "settings": {"mode": "discover", "idleTimeout": 300}});
    }
    fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!({"mcpServers": {}, "settings": {}}))
}

fn save_config(config: &Value) -> bool {
    let path = config_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    serde_json::to_string_pretty(config)
        .ok()
        .and_then(|json| fs::write(&path, json).ok())
        .is_some()
}

fn read_cache() -> Option<Value> {
    let path = cache_path();
    if !path.exists() {
        return None;
    }
    fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
}

// ─── HTTP Parsing ────────────────────────────────────────────

struct HttpRequest {
    method: String,
    path: String,
    body: String,
}

fn parse_request(raw: &str) -> Option<HttpRequest> {
    let mut lines = raw.lines();
    let first = lines.next()?;
    let parts: Vec<&str> = first.split_whitespace().collect();
    if parts.len() < 2 {
        return None;
    }
    let method = parts[0].to_string();
    let path = parts[1].to_string();

    // Find Content-Length
    let mut content_length: usize = 0;
    for line in raw.lines() {
        if line.is_empty() || line == "\r" {
            break;
        }
        let lower = line.to_lowercase();
        if let Some(val) = lower.strip_prefix("content-length:") {
            content_length = val.trim().parse().unwrap_or(0);
        }
    }

    let body = if content_length > 0 {
        if let Some(idx) = raw.find("\r\n\r\n") {
            raw[idx + 4..].to_string()
        } else if let Some(idx) = raw.find("\n\n") {
            raw[idx + 2..].to_string()
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    Some(HttpRequest { method, path, body })
}

fn http_response(status: u16, status_text: &str, content_type: &str, body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, POST, PUT, DELETE, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type\r\nConnection: close\r\n\r\n{}",
        status, status_text, content_type, body.len(), body
    )
    .into_bytes()
}

fn json_ok(data: Value) -> Vec<u8> {
    http_response(200, "OK", "application/json", &data.to_string())
}

fn json_err(status: u16, msg: &str) -> Vec<u8> {
    http_response(
        status,
        "Error",
        "application/json",
        &json!({"error": msg}).to_string(),
    )
}

// ─── API Handlers ────────────────────────────────────────────

fn handle_get_servers() -> Vec<u8> {
    let config = read_config();
    let cache = read_cache();
    let servers_obj = config
        .get("mcpServers")
        .or_else(|| config.get("servers"))
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();
    let cached_servers = cache
        .as_ref()
        .and_then(|c| c.get("servers"))
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();

    let mut result: Vec<Value> = Vec::new();
    let mut names: Vec<String> = servers_obj.keys().cloned().collect();
    names.sort();

    for name in &names {
        let srv = &servers_obj[name];
        let cached = cached_servers.get(name);
        let tools: Vec<String> = cached
            .and_then(|c| c.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|t| t.get("name").and_then(|n| n.as_str()).map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let tool_count = tools.len();

        result.push(json!({
            "name": name,
            "command": srv.get("command").and_then(|v| v.as_str()).unwrap_or(""),
            "args": srv.get("args").unwrap_or(&json!([])),
            "env": srv.get("env").unwrap_or(&json!({})),
            "disabled": srv.get("disabled").and_then(|v| v.as_bool()).unwrap_or(false),
            "tools": tool_count,
            "toolNames": tools,
            "status": if cached.is_some() { "cached" } else { "uncached" }
        }));
    }

    let total_tools: usize = result.iter().map(|s| s["tools"].as_u64().unwrap_or(0) as usize).sum();

    json_ok(json!({
        "servers": result,
        "settings": config.get("settings").unwrap_or(&json!({})),
        "totalTools": total_tools,
        "cacheExists": cache.is_some()
    }))
}

fn handle_add_server(body: &str) -> Vec<u8> {
    let data: Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(_) => return json_err(400, "Invalid JSON"),
    };
    let name = match data.get("name").and_then(|v| v.as_str()) {
        Some(n) => n.to_string(),
        None => return json_err(400, "Name required"),
    };
    let command = match data.get("command").and_then(|v| v.as_str()) {
        Some(c) => c.to_string(),
        None => return json_err(400, "Command required"),
    };

    let args = if let Some(s) = data.get("args").and_then(|v| v.as_str()) {
        Value::Array(
            s.split_whitespace()
                .map(|a| Value::String(a.to_string()))
                .collect(),
        )
    } else {
        data.get("args").cloned().unwrap_or(json!([]))
    };

    let env = data.get("env").cloned().unwrap_or(json!({}));

    let mut config = read_config();
    let key = if config.get("servers").is_some() { "servers" } else { "mcpServers" };
    if config.get(key).is_none() {
        config[key] = json!({});
    }
    config[key][&name] = json!({
        "command": command,
        "args": args,
        "env": env
    });

    if save_config(&config) {
        json_ok(json!({"ok": true, "message": "Server added"}))
    } else {
        json_err(500, "Failed to save config")
    }
}

fn handle_update_server(name: &str, body: &str) -> Vec<u8> {
    let data: Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(_) => return json_err(400, "Invalid JSON"),
    };

    let mut config = read_config();
    let key = if config.get("servers").and_then(|v| v.as_object()).is_some() { "servers" } else { "mcpServers" };
    let servers = match config.get_mut(key).and_then(|v| v.as_object_mut()) {
        Some(s) => s,
        None => return json_err(404, "No servers configured"),
    };

    if !servers.contains_key(name) {
        return json_err(404, "Server not found");
    }

    let new_name = data
        .get("newName")
        .and_then(|v| v.as_str())
        .unwrap_or(name);

    if new_name != name {
        let existing = servers.remove(name).unwrap();
        servers.insert(new_name.to_string(), existing);
    }

    let srv = servers.get_mut(new_name).unwrap();
    if let Some(cmd) = data.get("command").and_then(|v| v.as_str()) {
        srv["command"] = json!(cmd);
    }
    if let Some(args) = data.get("args") {
        if let Some(s) = args.as_str() {
            srv["args"] = Value::Array(
                s.split_whitespace()
                    .map(|a| Value::String(a.to_string()))
                    .collect(),
            );
        } else {
            srv["args"] = args.clone();
        }
    }
    if let Some(env) = data.get("env") {
        srv["env"] = env.clone();
    }

    if save_config(&config) {
        json_ok(json!({"ok": true}))
    } else {
        json_err(500, "Failed to save config")
    }
}

fn handle_delete_server(name: &str) -> Vec<u8> {
    let mut config = read_config();
    let key = if config.get("servers").and_then(|v| v.as_object()).is_some() { "servers" } else { "mcpServers" };
    let servers = match config.get_mut(key).and_then(|v| v.as_object_mut()) {
        Some(s) => s,
        None => return json_err(404, "No servers configured"),
    };
    if servers.remove(name).is_none() {
        return json_err(404, "Server not found");
    }
    if save_config(&config) {
        json_ok(json!({"ok": true}))
    } else {
        json_err(500, "Failed to save config")
    }
}

fn handle_toggle_server(name: &str, body: &str) -> Vec<u8> {
    let data: Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(_) => return json_err(400, "Invalid JSON"),
    };
    let disabled = data.get("disabled").and_then(|v| v.as_bool()).unwrap_or(false);

    let mut config = read_config();
    let key = if config.get("servers").and_then(|v| v.as_object()).is_some() { "servers" } else { "mcpServers" };
    let servers = match config.get_mut(key).and_then(|v| v.as_object_mut()) {
        Some(s) => s,
        None => return json_err(404, "No servers configured"),
    };
    let server = match servers.get_mut(name) {
        Some(s) => s,
        None => return json_err(404, "Server not found"),
    };
    if let Some(obj) = server.as_object_mut() {
        if disabled {
            obj.insert("disabled".to_string(), json!(true));
        } else {
            obj.remove("disabled");
        }
    }
    if save_config(&config) {
        json_ok(json!({"ok": true, "disabled": disabled}))
    } else {
        json_err(500, "Failed to save config")
    }
}

fn handle_get_settings() -> Vec<u8> {
    let config = read_config();
    let settings = config
        .get("settings")
        .cloned()
        .unwrap_or(json!({"mode": "discover", "idleTimeout": 300}));
    json_ok(settings)
}

fn handle_update_settings(body: &str) -> Vec<u8> {
    let data: Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(_) => return json_err(400, "Invalid JSON"),
    };
    let mut config = read_config();
    if let Some(existing) = config.get_mut("settings").and_then(|v| v.as_object_mut()) {
        if let Some(obj) = data.as_object() {
            for (k, v) in obj {
                existing.insert(k.clone(), v.clone());
            }
        }
    } else {
        config["settings"] = data;
    }
    if save_config(&config) {
        json_ok(json!({"ok": true, "settings": config["settings"]}))
    } else {
        json_err(500, "Failed to save config")
    }
}

async fn handle_generate() -> Vec<u8> {
    let bin = binary_path();
    if !bin.exists() {
        return json_err(500, "Binary not found");
    }
    let output = tokio::process::Command::new(&bin)
        .arg("generate")
        .output()
        .await;

    match output {
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let stdout = String::from_utf8_lossy(&out.stdout);
            let combined = format!("{}{}", stderr, stdout);

            let mut server_results: Vec<Value> = Vec::new();
            for line in combined.lines() {
                if let Some(caps) = line.find("] ").and_then(|i| {
                    let rest = &line[i + 2..];
                    let name_end = rest.find(" ...")?;
                    let name = &rest[..name_end];
                    if rest.contains("FAILED") {
                        Some((name.to_string(), 0, false))
                    } else {
                        let tools_str = rest.find("... ")
                            .map(|j| &rest[j + 4..])
                            .and_then(|s| s.split_whitespace().next())
                            .and_then(|n| n.parse::<usize>().ok())
                            .unwrap_or(0);
                        Some((name.to_string(), tools_str, true))
                    }
                }) {
                    server_results.push(json!({
                        "name": caps.0,
                        "tools": caps.1,
                        "ok": caps.2
                    }));
                }
            }

            let summary = if let Some(idx) = combined.find("Done:") {
                let rest = &combined[idx..];
                let parts: Vec<&str> = rest.split_whitespace().collect();
                let ok_count = parts.get(1).and_then(|s| s.parse::<usize>().ok()).unwrap_or(0);
                let failed = parts.get(3).and_then(|s| s.trim_end_matches(',').parse::<usize>().ok()).unwrap_or(0);
                let total = parts.get(5).and_then(|s| s.parse::<usize>().ok()).unwrap_or(0);
                Some(json!({"ok": ok_count, "failed": failed, "totalTools": total}))
            } else {
                None
            };

            json_ok(json!({
                "ok": out.status.success(),
                "servers": server_results,
                "summary": summary
            }))
        }
        Err(e) => json_err(500, &format!("Failed to run generate: {}", e)),
    }
}

// ─── Router ──────────────────────────────────────────────────

async fn route(req: &HttpRequest) -> Vec<u8> {
    let path = req.path.split('?').next().unwrap_or(&req.path);

    if req.method == "OPTIONS" {
        return http_response(204, "No Content", "text/plain", "");
    }

    match (&req.method[..], path) {
        ("GET", "/") => http_response(200, "OK", "text/html; charset=utf-8", DASHBOARD_HTML),
        ("GET", "/api/servers") => handle_get_servers(),
        ("POST", "/api/servers") => handle_add_server(&req.body),
        ("GET", "/api/settings") => handle_get_settings(),
        ("PUT", "/api/settings") => handle_update_settings(&req.body),
        ("POST", "/api/generate") => handle_generate().await,
        _ => {
            if path.starts_with("/api/servers/") {
                let rest = &path["/api/servers/".len()..];
                if rest.ends_with("/toggle") {
                    let name = &rest[..rest.len() - "/toggle".len()];
                    let decoded = urldecode(name);
                    handle_toggle_server(&decoded, &req.body)
                } else {
                    let decoded = urldecode(rest);
                    match &req.method[..] {
                        "PUT" => handle_update_server(&decoded, &req.body),
                        "DELETE" => handle_delete_server(&decoded),
                        _ => json_err(405, "Method not allowed"),
                    }
                }
            } else {
                json_err(404, "Not found")
            }
        }
    }
}

fn urldecode(s: &str) -> String {
    let mut result = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '%' {
            let hex: String = chars.by_ref().take(2).collect();
            if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                result.push(byte as char);
            }
        } else if c == '+' {
            result.push(' ');
        } else {
            result.push(c);
        }
    }
    result
}

// ─── Server Entry Point ─────────────────────────────────────

pub async fn start_dashboard() {
    let addr = "127.0.0.1:24680";
    let listener = match TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("[dashboard] Failed to bind {}: {}", addr, e);
            eprintln!("[dashboard] Is another instance running?");
            return;
        }
    };

    eprintln!("[dashboard] Running on http://{}", addr);

    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open")
        .arg(format!("http://{}", addr))
        .spawn();
    #[cfg(target_os = "linux")]
    let _ = std::process::Command::new("xdg-open")
        .arg(format!("http://{}", addr))
        .spawn();
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("cmd")
        .args(["/c", "start", &format!("http://{}", addr)])
        .spawn();

    loop {
        let (mut stream, _) = match listener.accept().await {
            Ok(conn) => conn,
            Err(_) => continue,
        };

        tokio::spawn(async move {
            let mut buf = vec![0u8; 65536];
            let n = match stream.read(&mut buf).await {
                Ok(n) if n > 0 => n,
                _ => return,
            };
            let raw = String::from_utf8_lossy(&buf[..n]).to_string();

            if let Some(req) = parse_request(&raw) {
                let response = route(&req).await;
                let _ = stream.write_all(&response).await;
            }
            let _ = stream.shutdown().await;
        });
    }
}

// ─── Embedded HTML ───────────────────────────────────────────

const DASHBOARD_HTML: &str = include_str!("../static/dashboard.html");
