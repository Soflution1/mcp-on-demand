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
}

pub struct ChildManager {
    configs: HashMap<String, ServerConfig>,
    children: Arc<Mutex<HashMap<String, ChildProcess>>>,
    idle_timeout_ms: u64,
}

impl ChildManager {
    pub fn new(configs: HashMap<String, ServerConfig>, idle_timeout_ms: u64) -> Self {
        Self {
            configs,
            children: Arc::new(Mutex::new(HashMap::new())),
            idle_timeout_ms,
        }
    }

    /// Start a server by name. Returns its tools list.
    pub async fn start_server(&self, name: &str) -> Result<Vec<ToolDef>, String> {
        // Already running?
        {
            let mut children = self.children.lock().await;
            if let Some(proc) = children.get_mut(name) {
                proc.last_used = Instant::now();
                return Ok(proc.tools.clone());
            }
        }

        let config = self
            .configs
            .get(name)
            .ok_or_else(|| format!("Unknown server: {}", name))?
            .clone();

        let start = Instant::now();
        eprintln!("[mcp-on-demand][INFO] Starting server: {}", name);

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
        };

        // Initialize MCP handshake
        let _init_result = send_request(
            &mut proc,
            "initialize",
            serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "mcp-on-demand", "version": "2.0.0" }
            }),
        )
        .await?;

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
            "[mcp-on-demand][INFO] Server '{}' ready: {} tools in {:.0}ms",
            name,
            tools.len(),
            elapsed.as_secs_f64() * 1000.0
        );

        proc.tools = tools.clone();

        let mut children = self.children.lock().await;
        children.insert(name.to_string(), proc);

        Ok(tools)
    }

    /// Call a tool on a specific server.
    pub async fn call_tool(
        &self,
        server_name: &str,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        // Auto-start if needed
        if !self.is_running(server_name).await {
            self.start_server(server_name).await?;
        }

        let mut children = self.children.lock().await;
        let proc = children
            .get_mut(server_name)
            .ok_or_else(|| format!("Server not running: {}", server_name))?;

        proc.last_used = Instant::now();

        let result = send_request(
            proc,
            "tools/call",
            serde_json::json!({
                "name": tool_name,
                "arguments": arguments,
            }),
        )
        .await?;

        Ok(result)
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
            eprintln!("[mcp-on-demand][INFO] Stopped server: {}", name);
        }
    }

    /// Stop all servers.
    pub async fn stop_all(&self) {
        let mut children = self.children.lock().await;
        for (name, mut proc) in children.drain() {
            let _ = proc.child.kill().await;
            eprintln!("[mcp-on-demand][INFO] Stopped server: {}", name);
        }
    }

    /// Returns list of all configured server names.
    pub fn server_names(&self) -> Vec<String> {
        self.configs.keys().cloned().collect()
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
                eprintln!("[mcp-on-demand][INFO] Idle-stopped server: {}", name);
            }
        }
    }
}

// ─── MCP Protocol Communication ─────────────────────────────

async fn send_request(
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
