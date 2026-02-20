/// Child process manager: spawn MCP servers, communicate over stdio, manage lifecycle.
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Instant;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

pub use crate::config::ServerConfig;
use crate::protocol::ToolDef;

#[derive(Debug)]
struct ChildProcess {
    child: Child,
    stdin: tokio::process::ChildStdin,
    stdout_lines: Arc<Mutex<tokio::io::Lines<BufReader<tokio::process::ChildStdout>>>>,
    next_id: u64,
    tools: Vec<ToolDef>,
    last_used: Instant,
    server_name: String,
    protocol_version: String,
}

pub struct ChildManager {
    configs: Arc<Mutex<HashMap<String, ServerConfig>>>,
    children: Arc<Mutex<HashMap<String, ChildProcess>>>,
    idle_timeout_ms: u64,
}

impl ChildManager {
    pub fn new(configs: HashMap<String, ServerConfig>, idle_timeout_ms: u64) -> Self {
        Self {
            configs: Arc::new(Mutex::new(configs)),
            children: Arc::new(Mutex::new(HashMap::new())),
            idle_timeout_ms,
        }
    }

    pub async fn update_configs(&self, new_configs: HashMap<String, ServerConfig>) {
        let mut current_configs = self.configs.lock().await;
        
        // Find removed or changed servers and stop them
        let mut to_stop = Vec::new();
        for (name, old_cfg) in current_configs.iter() {
            if let Some(new_cfg) = new_configs.get(name) {
                if old_cfg != new_cfg {
                    to_stop.push(name.clone());
                }
            } else {
                to_stop.push(name.clone());
            }
        }

        for name in to_stop {
            self.stop_server(&name).await;
        }

        *current_configs = new_configs;
    }

    /// Resolve a server name case-insensitively.
    /// Matches exact first, then case-insensitive, then kebab/snake normalization.
    async fn resolve_name(&self, name: &str) -> Option<String> {
        let configs = self.configs.lock().await;
        // 1. Exact match
        if configs.contains_key(name) {
            return Some(name.to_string());
        }
        // 2. Case-insensitive match
        let lower = name.to_lowercase();
        for key in configs.keys() {
            if key.to_lowercase() == lower {
                return Some(key.clone());
            }
        }
        // 3. Normalize: strip hyphens/underscores, compare lowercase
        let normalized = lower.replace(['-', '_'], "");
        for key in configs.keys() {
            let key_normalized = key.to_lowercase().replace(['-', '_'], "");
            if key_normalized == normalized {
                return Some(key.clone());
            }
        }
        None
    }

    /// Start a server by name with retry logic. Returns its tools list.
    pub async fn start_server(&self, name: &str) -> Result<Vec<ToolDef>, String> {
        let name_resolved = self.resolve_name(name).await
            .ok_or_else(|| format!("Unknown server: {}", name))?;
        let name = name_resolved.as_str();

        // Already running?
        {
            let mut children = self.children.lock().await;
            if let Some(proc) = children.get_mut(name) {
                proc.last_used = Instant::now();
                return Ok(proc.tools.clone());
            }
        }

        const MAX_RETRIES: u32 = 3;
        const BACKOFF_MS: [u64; 3] = [500, 1000, 2000];
        let mut last_error = String::new();

        for attempt in 0..MAX_RETRIES {
            if attempt > 0 {
                let delay = BACKOFF_MS.get(attempt as usize - 1).copied().unwrap_or(2000);
                eprintln!("[McpHub][RETRY] {} attempt {}/{} (backoff {}ms)", name, attempt + 1, MAX_RETRIES, delay);
                tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
            }

            match self.try_start_server(name).await {
                Ok(tools) => return Ok(tools),
                Err(e) => {
                    last_error = e;
                    if attempt < MAX_RETRIES - 1 {
                        eprintln!("[McpHub][WARN] {} failed: {} — retrying...", name, last_error);
                    }
                }
            }
        }

        Err(format!("{} (after {} attempts)", last_error, MAX_RETRIES))
    }

    /// Single attempt to start a server.
    async fn try_start_server(&self, name: &str) -> Result<Vec<ToolDef>, String> {
        let config = {
            let configs = self.configs.lock().await;
            configs
                .get(name)
                .ok_or_else(|| format!("Unknown server: {}", name))?
                .clone()
        };

        let start = Instant::now();
        eprintln!("[McpHub][INFO] Starting server: {}", name);

        let mut cmd = Command::new(&config.command);
        cmd.args(&config.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        for (k, v) in &config.env {
            cmd.env(k, v);
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Failed to spawn {}: {}", name, e))?;

        let stdin = child.stdin.take().ok_or("No stdin")?;
        let stdout = child.stdout.take().ok_or("No stdout")?;

        let reader = BufReader::new(stdout);
        let lines = Arc::new(Mutex::new(reader.lines()));

        let mut proc = ChildProcess {
            child,
            stdin,
            stdout_lines: lines,
            next_id: 1,
            tools: Vec::new(),
            last_used: Instant::now(),
            server_name: name.to_string(),
            protocol_version: "2024-11-05".to_string(),
        };

        // Initialize MCP handshake
        let init_result = send_request(
            &mut proc,
            "initialize",
            serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "McpHub", "version": "4.0.0" }
            }),
        )
        .await?;

        if let Some(pv) = init_result.get("protocolVersion").and_then(|v| v.as_str()) {
            proc.protocol_version = pv.to_string();
            eprintln!("[McpHub][INFO] Server '{}' negotiated protocol: {}", name, pv);
        }

        // Send initialized notification
        send_notification(&mut proc, "notifications/initialized", serde_json::json!({}))
            .await?;

        // List tools
        let tools_result = send_request(&mut proc, "tools/list", serde_json::json!({})).await?;

        let tools: Vec<ToolDef> = tools_result
            .get("tools")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        let elapsed = start.elapsed();
        eprintln!(
            "[McpHub][INFO] Server '{}' ready: {} tools in {:.0}ms",
            name,
            tools.len(),
            elapsed.as_secs_f64() * 1000.0
        );

        proc.tools = tools.clone();

        let mut children = self.children.lock().await;
        children.insert(name.to_string(), proc);

        Ok(tools)
    }

    /// Call a generic method on a specific server.
    pub async fn call_method(
        &self,
        server_name: &str,
        method: &str,
        arguments: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let resolved = self.resolve_name(server_name).await
            .ok_or_else(|| format!("Unknown server: {}", server_name))?;
        let server_name = resolved.as_str();

        if !self.is_running(server_name).await {
            return Err(format!("Server not running: {}", server_name));
        }

        let result = {
            let mut children = self.children.lock().await;
            let proc = children
                .get_mut(server_name)
                .ok_or_else(|| format!("Server not running: {}", server_name))?;

            proc.last_used = Instant::now();
            send_request(proc, method, arguments.clone()).await
        };

        match result {
            Err(e) if is_connection_error(&e) => {
                eprintln!("[McpHub][WARN] Connection error on '{}': {}. Retrying...", server_name, e);
                self.restart_server(server_name).await?;
                
                let mut children = self.children.lock().await;
                let proc = children
                    .get_mut(server_name)
                    .ok_or_else(|| format!("Server not running: {}", server_name))?;

                proc.last_used = Instant::now();
                send_request(proc, method, arguments).await
            }
            other => other,
        }
    }

    /// Call a tool on a specific server.
    pub async fn call_tool(
        &self,
        server_name: &str,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let resolved = self.resolve_name(server_name).await
            .ok_or_else(|| format!("Unknown server: {}", server_name))?;
        let server_name = resolved.as_str();

        // Auto-start if needed
        if !self.is_running(server_name).await {
            self.start_server(server_name).await?;
        }

        let result = {
            let mut children = self.children.lock().await;
            let proc = children
                .get_mut(server_name)
                .ok_or_else(|| format!("Server not running: {}", server_name))?;

            proc.last_used = Instant::now();

            send_request(
                proc,
                "tools/call",
                serde_json::json!({
                    "name": tool_name,
                    "arguments": arguments.clone(),
                }),
            )
            .await
        };

        match result {
            Err(e) if is_connection_error(&e) => {
                eprintln!("[McpHub][WARN] Connection error on '{}': {}. Retrying...", server_name, e);
                self.restart_server(server_name).await?;
                
                let mut children = self.children.lock().await;
                let proc = children
                    .get_mut(server_name)
                    .ok_or_else(|| format!("Server not running: {}", server_name))?;

                proc.last_used = Instant::now();

                send_request(
                    proc,
                    "tools/call",
                    serde_json::json!({
                        "name": tool_name,
                        "arguments": arguments,
                    }),
                )
                .await
            }
            other => other,
        }
    }

    pub async fn is_running(&self, name: &str) -> bool {
        let children = self.children.lock().await;
        children.contains_key(name)
    }

    /// Stop a server by name.
    #[allow(dead_code)]
    pub async fn stop_server(&self, name: &str) {
        let mut children = self.children.lock().await;
        if let Some(mut proc) = children.remove(name) {
            let _ = proc.child.kill().await;
            eprintln!("[McpHub][INFO] Stopped server: {}", name);
        }
    }

    /// Stop all servers.
    pub async fn stop_all(&self) {
        let mut children = self.children.lock().await;
        for (name, mut proc) in children.drain() {
            let _ = proc.child.kill().await;
            eprintln!("[McpHub][INFO] Stopped server: {}", name);
        }
    }

    /// Returns list of all configured server names.
    pub async fn server_names(&self) -> Vec<String> {
        let configs = self.configs.lock().await;
        configs.keys().cloned().collect()
    }

    /// Send a request to all running servers.
    pub async fn request_all_running(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Vec<(String, Result<serde_json::Value, String>)> {
        let running_servers: Vec<String> = {
            let children = self.children.lock().await;
            children.keys().cloned().collect()
        };

        let mut results = Vec::new();
        for name in running_servers {
            let result = {
                let mut children = self.children.lock().await;
                if let Some(proc) = children.get_mut(&name) {
                    proc.last_used = Instant::now();
                    send_request(proc, method, params.clone()).await
                } else {
                    Err("Server stopped".into())
                }
            };
            results.push((name, result));
        }
        
        results
    }

    /// Forward a notification to a specific server.
    pub async fn forward_notification(
        &self,
        server_name: &str,
        method: &str,
        params: serde_json::Value,
    ) -> Result<(), String> {
        let mut children = self.children.lock().await;
        if let Some(proc) = children.get_mut(server_name) {
            proc.last_used = Instant::now();
            send_notification(proc, method, params).await
        } else {
            Err("Server not running".into())
        }
    }

    /// Run idle reaper: stop servers not used in idle_timeout_ms.
    pub async fn reap_idle(&self) {
        let timeout = std::time::Duration::from_millis(self.idle_timeout_ms);
        let mut children = self.children.lock().await;

        let idle_servers: Vec<String> = children
            .iter()
            .filter(|(_, proc)| proc.last_used.elapsed() > timeout)
            .map(|(name, _)| name.clone())
            .collect();

        for name in idle_servers {
            if let Some(mut proc) = children.remove(&name) {
                let _ = proc.child.kill().await;
                eprintln!("[McpHub][INFO] Idle-stopped server: {}", name);
            }
        }
    }

    /// Health check: ping all running servers. Returns list of dead/unresponsive ones.
    pub async fn health_check(&self) -> Vec<(String, String)> {
        let mut dead_servers: Vec<(String, String)> = Vec::new();
        let mut children = self.children.lock().await;

        let names: Vec<String> = children.keys().cloned().collect();
        for name in names {
            let proc = match children.get_mut(&name) {
                Some(p) => p,
                None => continue,
            };

            // Check if process is still alive
            match proc.child.try_wait() {
                Ok(Some(status)) => {
                    dead_servers.push((name.clone(), format!("Process exited: {}", status)));
                    children.remove(&name);
                    continue;
                }
                Ok(None) => {} // Still running
                Err(e) => {
                    dead_servers.push((name.clone(), format!("Process check failed: {}", e)));
                    children.remove(&name);
                    continue;
                }
            }

            // Try MCP ping with short timeout
            let ping_timeout = std::time::Duration::from_secs(5);
            let ping_result = tokio::time::timeout(
                ping_timeout,
                send_request_inner(proc, "ping", serde_json::json!({})),
            ).await;

            match ping_result {
                Ok(Ok(_)) => {} // Healthy
                Ok(Err(e)) => {
                    dead_servers.push((name.clone(), format!("Ping error: {}", e)));
                    if let Some(mut p) = children.remove(&name) {
                        let _ = p.child.kill().await;
                    }
                }
                Err(_) => {
                    dead_servers.push((name.clone(), "Ping timeout (5s)".to_string()));
                    if let Some(mut p) = children.remove(&name) {
                        let _ = p.child.kill().await;
                    }
                }
            }
        }

        dead_servers
    }

    /// Restart a server by name. Returns Ok with tool count or error.
    pub async fn restart_server(&self, name: &str) -> Result<usize, String> {
        // Kill if still present
        {
            let mut children = self.children.lock().await;
            if let Some(mut proc) = children.remove(name) {
                let _ = proc.child.kill().await;
            }
        }
        // Small delay before restart
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let tools = self.start_server(name).await?;
        Ok(tools.len())
    }
}

fn is_connection_error(e: &str) -> bool {
    e.contains("Write error") || e.contains("Flush error") || e.contains("Read error") || e.contains("Server closed connection")
}

// ─── MCP Protocol Communication ─────────────────────────────

const REQUEST_TIMEOUT_SECS: u64 = 30;

async fn send_request(
    proc: &mut ChildProcess,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let timeout = std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS);
    match tokio::time::timeout(timeout, send_request_inner(proc, method, params)).await {
        Ok(result) => result,
        Err(_) => Err(format!("Timeout: server did not respond within {}s", REQUEST_TIMEOUT_SECS)),
    }
}

async fn send_request_inner(
    proc: &mut ChildProcess,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let id = proc.next_id;
    proc.next_id += 1;

    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    });

    let mut msg = serde_json::to_string(&request).map_err(|e| e.to_string())?;
    msg.push('\n');

    proc.stdin
        .write_all(msg.as_bytes())
        .await
        .map_err(|e| format!("Write error: {}", e))?;
    proc.stdin
        .flush()
        .await
        .map_err(|e| format!("Flush error: {}", e))?;

    // Read lines until we get a response with our ID
    let mut lines = proc.stdout_lines.lock().await;
    loop {
        let line = lines
            .next_line()
            .await
            .map_err(|e| format!("Read error: {}", e))?
            .ok_or("Server closed connection")?;

        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }

        let parsed: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        // Skip notifications
        if parsed.get("id").is_none() {
            if let Some(method) = parsed.get("method").and_then(|v| v.as_str()) {
                if method == "notifications/message" {
                    if let Some(params) = parsed.get("params") {
                        if let Some(level) = params.get("level").and_then(|v| v.as_str()) {
                            if let Some(data) = params.get("data").and_then(|v| v.as_str()) {
                                eprintln!("[McpHub][{}][{}] {}", proc.server_name, level.to_uppercase(), data);
                            }
                        }
                    }
                }
            }
            continue;
        }

        // Check if this is our response
        if let Some(resp_id) = parsed.get("id") {
            if resp_id.as_u64() == Some(id) {
                if let Some(error) = parsed.get("error") {
                    return Err(format!("MCP error: {}", error));
                }
                return Ok(parsed.get("result").cloned().unwrap_or(serde_json::Value::Null));
            }
        }
    }
}

async fn send_notification(
    proc: &mut ChildProcess,
    method: &str,
    params: serde_json::Value,
) -> Result<(), String> {
    let notification = serde_json::json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
    });

    let mut msg = serde_json::to_string(&notification).map_err(|e| e.to_string())?;
    msg.push('\n');

    proc.stdin
        .write_all(msg.as_bytes())
        .await
        .map_err(|e| format!("Write error: {}", e))?;
    proc.stdin
        .flush()
        .await
        .map_err(|e| format!("Flush error: {}", e))?;

    Ok(())
}
