/// Core proxy server: reads JSON-RPC from stdin, routes to child servers.
/// Two modes: discover (2 meta-tools) or passthrough (all tools exposed).
use std::sync::Arc;

use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;

use crate::child::ChildManager;
use crate::config::{Mode, Preload, ProxyConfig};
use crate::protocol::*;
use crate::search::{IndexedTool, SearchEngine};

pub struct ProxyServer {
    config: ProxyConfig,
    child_manager: Arc<ChildManager>,
    search_engine: Arc<Mutex<SearchEngine>>,
}

impl ProxyServer {
    pub fn new(config: ProxyConfig) -> Self {
        let child_manager = Arc::new(ChildManager::new(
            config.servers.clone(),
            config.idle_timeout_ms,
        ));

        Self {
            config,
            child_manager,
            search_engine: Arc::new(Mutex::new(SearchEngine::new())),
        }
    }

    pub async fn run(&self) {
        // 1. Load cache synchronously FIRST (instant, <1ms)
        if let Some(cached) = crate::cache::load_cache() {
            let mut all_tools: Vec<IndexedTool> = Vec::new();
            for (server_name, tools) in &cached.servers {
                for tool in tools {
                    all_tools.push(IndexedTool {
                        name: format!("{}__{}", server_name, tool.name),
                        original_name: tool.name.clone(),
                        server_name: server_name.to_string(),
                        description: tool.description.clone(),
                        tool_def: tool.clone(),
                    });
                }
            }
            if !all_tools.is_empty() {
                let mut eng = self.search_engine.lock().await;
                eng.build_index(all_tools);
                eprintln!("[mcp-on-demand][INFO] Ready: {} tools from cache", eng.tool_count());
            }
        } else {
            eprintln!("[mcp-on-demand][WARN] No cache found. Run 'mcp-on-demand generate' for instant startup.");
        }

        // 2. Start idle reaper
        let manager_reap = self.child_manager.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
                manager_reap.reap_idle().await;
            }
        });

        // 3. Main stdio loop (index already populated)
        self.stdio_loop().await;
    }

    fn servers_to_preload(&self) -> Vec<String> {
        match &self.config.preload {
            Preload::All => self.child_manager.server_names(),
            Preload::Some(names) => names.clone(),
            Preload::None => Vec::new(),
        }
    }

    async fn stdio_loop(&self) {
        let stdin = io::stdin();
        let mut stdout = io::stdout();
        let reader = BufReader::new(stdin);
        let mut lines = reader.lines();

        while let Ok(Some(line)) = lines.next_line().await {
            let line = line.trim().to_string();
            if line.is_empty() {
                continue;
            }

            let request: JsonRpcRequest = match serde_json::from_str(&line) {
                Ok(r) => r,
                Err(_) => continue,
            };

            let response = self.handle_request(request).await;

            if let Some(resp) = response {
                let mut msg = serde_json::to_string(&resp).unwrap();
                msg.push('\n');
                let _ = stdout.write_all(msg.as_bytes()).await;
                let _ = stdout.flush().await;
            }
        }

        // Cleanup
        self.child_manager.stop_all().await;
    }

    async fn handle_request(&self, req: JsonRpcRequest) -> Option<JsonRpcResponse> {
        match req.method.as_str() {
            "initialize" => Some(self.handle_initialize(req.id)),
            "notifications/initialized" => None,
            "tools/list" => Some(self.handle_tools_list(req.id).await),
            "tools/call" => Some(self.handle_tools_call(req.id, req.params).await),
            "ping" => Some(JsonRpcResponse::success(req.id, serde_json::json!({}))),
            _ => Some(JsonRpcResponse::error(
                req.id,
                -32601,
                format!("Method not found: {}", req.method),
            )),
        }
    }

    fn handle_initialize(&self, id: Option<serde_json::Value>) -> JsonRpcResponse {
        let mode_str = match self.config.mode {
            Mode::Discover => "discover",
            Mode::Passthrough => "passthrough",
        };

        eprintln!(
            "[mcp-on-demand][INFO] Initialize: mode={}, servers={}",
            mode_str,
            self.config.servers.len()
        );

        let result = InitializeResult {
            protocol_version: "2024-11-05".into(),
            capabilities: Capabilities {
                tools: ToolsCapability {},
            },
            server_info: ServerInfo {
                name: "mcp-on-demand".into(),
                version: "2.0.0".into(),
            },
        };

        JsonRpcResponse::success(id, serde_json::to_value(result).unwrap())
    }

    async fn handle_tools_list(
        &self,
        id: Option<serde_json::Value>,
    ) -> JsonRpcResponse {
        let tools = match self.config.mode {
            Mode::Discover => self.get_discover_tools(),
            Mode::Passthrough => self.get_passthrough_tools().await,
        };

        JsonRpcResponse::success(id, serde_json::json!({ "tools": tools }))
    }

    fn get_discover_tools(&self) -> serde_json::Value {
        serde_json::json!([
            {
                "name": "discover",
                "description": "Search for available MCP tools across all servers using natural language. Returns matching tools with their full schemas, plus the complete list of available servers. Always call this FIRST to find the right tool before calling execute.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Natural language search query (e.g. 'read file', 'git commit', 'database query', 'send email')"
                        },
                        "top_k": {
                            "type": "number",
                            "description": "Max results to return (default: 10, max: 50)",
                            "default": 10
                        }
                    },
                    "required": ["query"]
                }
            },
            {
                "name": "execute",
                "description": "Execute a tool on a specific MCP server. Use the server and tool names from discover results.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "server": {
                            "type": "string",
                            "description": "Server name (from discover results)"
                        },
                        "tool": {
                            "type": "string",
                            "description": "Tool name (from discover results)"
                        },
                        "arguments": {
                            "type": "object",
                            "description": "Tool arguments matching the tool's inputSchema",
                            "default": {}
                        }
                    },
                    "required": ["server", "tool"]
                }
            }
        ])
    }

    async fn get_passthrough_tools(&self) -> serde_json::Value {
        let engine = self.search_engine.lock().await;
        let catalog = engine.get_catalog();

        // Expose all tools with prefixed names
        let mut tools = Vec::new();
        for entry in &catalog {
            if let Some(indexed) = engine.find_tool(&entry.server, &entry.name) {
                let mut tool_json = serde_json::to_value(&indexed.tool_def).unwrap();
                if let Some(obj) = tool_json.as_object_mut() {
                    obj.insert(
                        "name".into(),
                        serde_json::Value::String(indexed.name.clone()),
                    );
                }
                tools.push(tool_json);
            }
        }

        serde_json::Value::Array(tools)
    }

    async fn handle_tools_call(
        &self,
        id: Option<serde_json::Value>,
        params: serde_json::Value,
    ) -> JsonRpcResponse {
        let tool_name = params
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let arguments = params
            .get("arguments")
            .cloned()
            .unwrap_or(serde_json::json!({}));

        match self.config.mode {
            Mode::Discover => match tool_name {
                "discover" => self.handle_discover(id, arguments).await,
                "execute" => self.handle_execute(id, arguments).await,
                _ => JsonRpcResponse::error(
                    id,
                    -32602,
                    format!("Unknown tool: {}. Use 'discover' first.", tool_name),
                ),
            },
            Mode::Passthrough => self.handle_passthrough_call(id, tool_name, arguments).await,
        }
    }

    async fn handle_discover(
        &self,
        id: Option<serde_json::Value>,
        args: serde_json::Value,
    ) -> JsonRpcResponse {
        let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
        let top_k = args.get("top_k").and_then(|v| v.as_u64()).unwrap_or(10).min(50) as usize;

        // Always provide the full server list
        let mut all_server_names: Vec<String> = self.config.servers.keys().cloned().collect();
        all_server_names.sort();

        let engine = self.search_engine.lock().await;

        if engine.tool_count() > 0 {
            let results = engine.search(query, top_k);

            // Collect unique servers from results
            let mut seen_servers: Vec<String> = Vec::new();
            let tools_json: Vec<serde_json::Value> = results.iter().map(|t| {
                if !seen_servers.contains(&t.server_name) {
                    seen_servers.push(t.server_name.clone());
                }
                serde_json::json!({
                    "server": t.server_name,
                    "tool": t.original_name,
                    "description": t.description,
                    "inputSchema": t.tool_def.input_schema,
                })
            }).collect();

            let text = serde_json::to_string_pretty(&serde_json::json!({
                "query": query,
                "total_indexed": engine.tool_count(),
                "total_servers": all_server_names.len(),
                "available_servers": all_server_names,
                "results": tools_json,
            })).unwrap();

            return JsonRpcResponse::success(id, serde_json::json!({
                "content": [{ "type": "text", "text": text }]
            }));
        }

        drop(engine);

        let query_lower = query.to_lowercase();
        let mut server_names: Vec<String> = self.config.servers.keys().cloned().collect();
        server_names.sort();

        let mut matches: Vec<serde_json::Value> = Vec::new();
        for name in &server_names {
            if query_lower.is_empty()
                || name.to_lowercase().contains(&query_lower)
                || query_lower.contains(&name.to_lowercase())
            {
                matches.push(serde_json::json!({
                    "server": name,
                    "tool": "Use execute with this server name",
                    "description": format!("MCP server: {}. Call execute with server=\"{}\" and your tool name.", name, name),
                }));
            }
        }

        if matches.is_empty() {
            for name in &server_names {
                matches.push(serde_json::json!({
                    "server": name,
                    "tool": "Available server",
                    "description": format!("MCP server: {}", name),
                }));
            }
        }

        let matches: Vec<serde_json::Value> = matches.into_iter().take(top_k).collect();

        let text = serde_json::to_string_pretty(&serde_json::json!({
            "query": query,
            "total_indexed": 0,
            "note": "Servers loading in background. Results based on server names. Use execute to call tools.",
            "available_servers": server_names,
            "results": matches,
        })).unwrap();

        JsonRpcResponse::success(id, serde_json::json!({
            "content": [{ "type": "text", "text": text }]
        }))
    }

    async fn handle_execute(
        &self,
        id: Option<serde_json::Value>,
        args: serde_json::Value,
    ) -> JsonRpcResponse {
        let server = match args.get("server").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => {
                return JsonRpcResponse::error(id, -32602, "Missing 'server' parameter".into())
            }
        };

        let tool = match args.get("tool").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => {
                return JsonRpcResponse::error(id, -32602, "Missing 'tool' parameter".into())
            }
        };

        let arguments = args
            .get("arguments")
            .cloned()
            .unwrap_or(serde_json::json!({}));

        match self.child_manager.call_tool(&server, &tool, arguments).await {
            Ok(result) => JsonRpcResponse::success(id, result),
            Err(e) => JsonRpcResponse::error(id, -32000, e),
        }
    }

    async fn handle_passthrough_call(
        &self,
        id: Option<serde_json::Value>,
        prefixed_name: &str,
        arguments: serde_json::Value,
    ) -> JsonRpcResponse {
        // Parse "server__tool" format
        let parts: Vec<&str> = prefixed_name.splitn(2, "__").collect();
        if parts.len() != 2 {
            return JsonRpcResponse::error(
                id,
                -32602,
                format!("Invalid tool name format: {}", prefixed_name),
            );
        }

        let server = parts[0];
        let tool = parts[1];

        match self.child_manager.call_tool(server, tool, arguments).await {
            Ok(result) => JsonRpcResponse::success(id, result),
            Err(e) => JsonRpcResponse::error(id, -32000, e),
        }
    }
}

/// Preload servers with staggered starts and build search index.
async fn preload_servers(
    manager: Arc<ChildManager>,
    engine: Arc<Mutex<SearchEngine>>,
    names: Vec<String>,
    delay_ms: u64,
) {
    let total = names.len();
    eprintln!(
        "[mcp-on-demand][INFO] Preloading {} servers ({}ms stagger)...",
        total, delay_ms
    );

    let mut all_tools: Vec<IndexedTool> = Vec::new();

    for (i, name) in names.iter().enumerate() {
        match manager.start_server(name).await {
            Ok(tools) => {
                for tool in tools {
                    all_tools.push(IndexedTool {
                        name: format!("{}__{}", name, tool.name),
                        original_name: tool.name.clone(),
                        server_name: name.clone(),
                        description: tool.description.clone(),
                        tool_def: tool,
                    });
                }
            }
            Err(e) => {
                eprintln!("[mcp-on-demand][ERROR] Failed to start '{}': {}", name, e);
            }
        }

        // Stagger starts (skip delay after last)
        if i < total - 1 && delay_ms > 0 {
            tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
        }
    }

    // Build search index
    let mut eng = engine.lock().await;
    eng.build_index(all_tools);
}
