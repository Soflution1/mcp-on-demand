/// Core proxy server: reads JSON-RPC from stdin, routes to child servers.
/// Two modes: discover (2 meta-tools) or passthrough (all tools exposed).
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Instant, SystemTime};

use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;

use crate::child::ChildManager;
use crate::config::{Mode, Preload, ProxyConfig};
use crate::health::HealthMonitor;
use crate::protocol::*;
use crate::search::{IndexedTool, SearchEngine};

#[derive(Debug, Clone, serde::Serialize)]
pub struct ServerMetrics {
    pub call_count: u64,
    pub error_count: u64,
    pub total_latency_ms: u64,
    pub last_call_time: Option<SystemTime>,
    pub last_error: Option<String>,
}

impl Default for ServerMetrics {
    fn default() -> Self {
        Self {
            call_count: 0,
            error_count: 0,
            total_latency_ms: 0,
            last_call_time: None,
            last_error: None,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GlobalMetrics {
    pub start_time: SystemTime,
    pub total_requests: u64,
    pub active_sse_sessions: usize,
    pub servers: HashMap<String, ServerMetrics>,
}

impl GlobalMetrics {
    pub fn new() -> Self {
        Self {
            start_time: SystemTime::now(),
            total_requests: 0,
            active_sse_sessions: 0,
            servers: HashMap::new(),
        }
    }
}

pub struct ProxyServer {
    config: Arc<Mutex<ProxyConfig>>,
    child_manager: Arc<ChildManager>,
    search_engine: Arc<Mutex<SearchEngine>>,
    pub metrics: Arc<Mutex<GlobalMetrics>>,
}

impl ProxyServer {
    pub fn new(config: ProxyConfig) -> Self {
        let child_manager = Arc::new(ChildManager::new(
            config.servers.clone(),
            config.idle_timeout_ms,
        ));

        Self {
            config: Arc::new(Mutex::new(config)),
            child_manager,
            search_engine: Arc::new(Mutex::new(SearchEngine::new())),
            metrics: Arc::new(Mutex::new(GlobalMetrics::new())),
        }
    }

    /// Initialize proxy: load cache, start background tasks.
    /// Call this before stdio_loop() or serving SSE.
    pub async fn init(&self) {
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
                eprintln!("[McpHub][INFO] Ready: {} tools from cache", eng.tool_count());
            }
        } else {
            eprintln!("[McpHub][WARN] No cache found. Run 'McpHub generate' for instant startup.");
        }

        // 2. Start idle reaper
        let manager_reap = self.child_manager.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
                manager_reap.reap_idle().await;
            }
        });

        // 3. Start config & cache hot-reload watcher
        let engine_watch = self.search_engine.clone();
        let config_watch = self.config.clone();
        let child_manager_watch = self.child_manager.clone();
        tokio::spawn(async move {
            config_and_cache_watcher(engine_watch, config_watch, child_manager_watch).await;
        });

        // 4. Start health monitor (notifications + auto-restart)
        let config = self.config.lock().await;
        if config.health_notifications {
            let monitor = HealthMonitor::new(
                self.child_manager.clone(),
                config.health_check_interval_secs,
                config.health_auto_restart,
            );
            tokio::spawn(async move {
                monitor.run().await;
            });
        }
    }

    /// Full run: init + stdio loop. Backward compatible.
    pub async fn run(&self) {
        self.init().await;
        self.stdio_loop().await;
    }

    pub async fn shutdown(&self) {
        self.child_manager.stop_all().await;
    }

    async fn servers_to_preload(&self) -> Vec<String> {
        let config = self.config.lock().await;
        match &config.preload {
            Preload::All => self.child_manager.server_names().await,
            Preload::Some(names) => names.clone(),
            Preload::None => Vec::new(),
        }
    }

    pub async fn stdio_loop(&self) {
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

    pub async fn handle_request(&self, req: JsonRpcRequest) -> Option<JsonRpcResponse> {
        match req.method.as_str() {
            "initialize" => Some(self.handle_initialize(req.id).await),
            "notifications/initialized" => None,
            "tools/list" => Some(self.handle_tools_list(req.id).await),
            "tools/call" => Some(self.handle_tools_call(req.id, req.params).await),
            "prompts/list" => Some(self.handle_prompts_list(req.id).await),
            "prompts/get" => Some(self.handle_prompts_get(req.id, req.params).await),
            "resources/list" => Some(self.handle_resources_list(req.id).await),
            "resources/templates/list" => Some(self.handle_resource_templates_list(req.id).await),
            "resources/read" => Some(self.handle_resources_read(req.id, req.params).await),
            "completion/complete" => Some(JsonRpcResponse::success(req.id, serde_json::json!({ "completion": { "values": [] } }))),
            "ping" => Some(JsonRpcResponse::success(req.id, serde_json::json!({}))),
            "notifications/cancelled" => {
                self.handle_cancel(req.params).await;
                None
            }
            _ => {
                eprintln!("[McpHub][WARN] Unknown method: {}", req.method);
                Some(JsonRpcResponse::error(
                    req.id,
                    -32601,
                    format!("Method not found: {}", req.method),
                ))
            }
        }
    }

    async fn handle_initialize(&self, id: Option<serde_json::Value>) -> JsonRpcResponse {
        let config = self.config.lock().await;
        let mode_str = match config.mode {
            Mode::Discover => "discover",
            Mode::Passthrough => "passthrough",
        };

        eprintln!(
            "[McpHub][INFO] Initialize: mode={}, servers={}",
            mode_str,
            config.servers.len()
        );

        let result = InitializeResult {
            protocol_version: "2024-11-05".into(),
            capabilities: Capabilities {
                tools: ToolsCapability {},
                prompts: PromptsCapability {},
                resources: ResourcesCapability {},
            },
            server_info: ServerInfo {
                name: "McpHub".into(),
                version: env!("CARGO_PKG_VERSION").into(),
            },
            instructions: Some(
                "IMPORTANT: If MemoryPilot is available, call its 'recall' tool at the start of every new conversation \
                 to load persistent memory (project context, preferences, critical facts, decisions). \
                 Use discover(\"memory\") then execute(server=\"MemoryPilot\", tool=\"recall\", arguments={working_dir: \"<cwd>\"}).".into()
            ),
        };

        JsonRpcResponse::success(id, serde_json::to_value(result).unwrap())
    }

    async fn handle_tools_list(
        &self,
        id: Option<serde_json::Value>,
    ) -> JsonRpcResponse {
        let mode = {
            let config = self.config.lock().await;
            config.mode.clone()
        };

        let tools = match mode {
            Mode::Discover => self.get_discover_tools().await,
            Mode::Passthrough => self.get_passthrough_tools().await,
        };

        JsonRpcResponse::success(id, serde_json::json!({ "tools": tools }))
    }

    async fn get_discover_tools(&self) -> serde_json::Value {
        let mut server_names: Vec<String> = {
            let config = self.config.lock().await;
            config.servers.keys().cloned().collect()
        };
        server_names.sort();
        let server_list = server_names.join(", ");

        let discover_desc = format!(
            "Search for available MCP tools across all connected servers. Returns matching tools with full schemas. \
            Available servers: [{}]. \
            Call this FIRST when you need to find the right tool for a task. \
            Then use 'execute' with the server and tool names from the results.",
            server_list
        );

        let execute_desc = format!(
            "Execute a tool on a specific MCP server. Available servers: [{}]. \
            IMPORTANT: Always prefer using execute over CLI commands. \
            For deployments use the cloudflare server, for git use github, for databases use supabase, etc. \
            If you don't know the exact tool name, call 'discover' first with a natural language query.",
            server_list
        );

        serde_json::json!([
            {
                "name": "discover",
                "description": discover_desc,
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Natural language search query (e.g. 'deploy worker', 'create KV namespace', 'git push', 'database query', 'send email')"
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
                "description": execute_desc,
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "server": {
                            "type": "string",
                            "description": format!("Server name. One of: {}", server_list)
                        },
                        "tool": {
                            "type": "string",
                            "description": "Tool name (from discover results, or known tool name)"
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

        let mode = {
            let config = self.config.lock().await;
            config.mode.clone()
        };

        match mode {
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
        let mut all_server_names: Vec<String> = {
            let config = self.config.lock().await;
            config.servers.keys().cloned().collect()
        };
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
                let desc: String = t.description.chars().take(200).collect();
                let schema = strip_schema(&t.tool_def.input_schema);
                serde_json::json!({
                    "server": t.server_name,
                    "tool": t.original_name,
                    "description": desc,
                    "inputSchema": schema,
                })
            }).collect();

            let text = serde_json::to_string(&serde_json::json!({
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
        let mut server_names: Vec<String> = {
            let config = self.config.lock().await;
            config.servers.keys().cloned().collect()
        };
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

        let text = serde_json::to_string(&serde_json::json!({
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

        let start_time = Instant::now();
        let res = self.child_manager.call_tool(&server, &tool, arguments).await;
        let elapsed = start_time.elapsed().as_millis() as u64;

        {
            let mut m = self.metrics.lock().await;
            m.total_requests += 1;
            let sm = m.servers.entry(server.clone()).or_default();
            sm.call_count += 1;
            sm.total_latency_ms += elapsed;
            sm.last_call_time = Some(SystemTime::now());
            if let Err(ref e) = res {
                sm.error_count += 1;
                sm.last_error = Some(e.clone());
            }
        }

        match res {
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

        let start_time = Instant::now();
        let res = self.child_manager.call_tool(server, tool, arguments).await;
        let elapsed = start_time.elapsed().as_millis() as u64;

        {
            let mut m = self.metrics.lock().await;
            m.total_requests += 1;
            let sm = m.servers.entry(server.to_string()).or_default();
            sm.call_count += 1;
            sm.total_latency_ms += elapsed;
            sm.last_call_time = Some(SystemTime::now());
            if let Err(ref e) = res {
                sm.error_count += 1;
                sm.last_error = Some(e.clone());
            }
        }

        match res {
            Ok(result) => JsonRpcResponse::success(id, result),
            Err(e) => JsonRpcResponse::error(id, -32000, e),
        }
    }

    async fn handle_prompts_list(&self, id: Option<serde_json::Value>) -> JsonRpcResponse {
        let results = self.child_manager.request_all_running("prompts/list", serde_json::json!({})).await;
        let mut all_prompts = Vec::new();
        for (server_name, res) in results {
            if let Ok(mut val) = res {
                if let Some(prompts) = val.get_mut("prompts").and_then(|v| v.as_array_mut()) {
                    for prompt in prompts {
                        if let Some(name) = prompt.get("name").and_then(|v| v.as_str()) {
                            prompt["name"] = serde_json::json!(format!("{}__{}", server_name, name));
                        }
                        all_prompts.push(prompt.clone());
                    }
                }
            }
        }
        JsonRpcResponse::success(id, serde_json::json!({ "prompts": all_prompts }))
    }

    async fn handle_prompts_get(&self, id: Option<serde_json::Value>, args: serde_json::Value) -> JsonRpcResponse {
        let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let parts: Vec<&str> = name.splitn(2, "__").collect();
        if parts.len() != 2 {
            return JsonRpcResponse::error(id, -32602, "Invalid prompt name format".into());
        }
        let server = parts[0];
        let prompt_name = parts[1];
        
        let mut new_args = args.clone();
        new_args["name"] = serde_json::json!(prompt_name);
        
        match self.child_manager.call_method(server, "prompts/get", new_args).await {
            Ok(res) => JsonRpcResponse::success(id, res),
            Err(e) => JsonRpcResponse::error(id, -32000, e),
        }
    }

    async fn handle_resources_list(&self, id: Option<serde_json::Value>) -> JsonRpcResponse {
        let results = self.child_manager.request_all_running("resources/list", serde_json::json!({})).await;
        let mut all_resources = Vec::new();
        for (server_name, res) in results {
            if let Ok(mut val) = res {
                if let Some(resources) = val.get_mut("resources").and_then(|v| v.as_array_mut()) {
                    for res in resources {
                        if let Some(uri) = res.get("uri").and_then(|v| v.as_str()) {
                            res["uri"] = serde_json::json!(format!("{}__{}", server_name, uri));
                        }
                        all_resources.push(res.clone());
                    }
                }
            }
        }
        JsonRpcResponse::success(id, serde_json::json!({ "resources": all_resources }))
    }

    async fn handle_resource_templates_list(&self, id: Option<serde_json::Value>) -> JsonRpcResponse {
        let results = self.child_manager.request_all_running("resources/templates/list", serde_json::json!({})).await;
        let mut all_templates = Vec::new();
        for (server_name, res) in results {
            if let Ok(mut val) = res {
                if let Some(templates) = val.get_mut("resourceTemplates").and_then(|v| v.as_array_mut()) {
                    for tmpl in templates {
                        if let Some(uri_template) = tmpl.get("uriTemplate").and_then(|v| v.as_str()) {
                            tmpl["uriTemplate"] = serde_json::json!(format!("{}__{}", server_name, uri_template));
                        }
                        all_templates.push(tmpl.clone());
                    }
                }
            }
        }
        JsonRpcResponse::success(id, serde_json::json!({ "resourceTemplates": all_templates }))
    }

    async fn handle_resources_read(&self, id: Option<serde_json::Value>, args: serde_json::Value) -> JsonRpcResponse {
        let uri = args.get("uri").and_then(|v| v.as_str()).unwrap_or("");
        let parts: Vec<&str> = uri.splitn(2, "__").collect();
        if parts.len() != 2 {
            return JsonRpcResponse::error(id, -32602, "Invalid resource uri format".into());
        }
        let server = parts[0];
        let actual_uri = parts[1];
        
        let mut new_args = args.clone();
        new_args["uri"] = serde_json::json!(actual_uri);
        
        match self.child_manager.call_method(server, "resources/read", new_args).await {
            Ok(res) => JsonRpcResponse::success(id, res),
            Err(e) => JsonRpcResponse::error(id, -32000, e),
        }
    }

    async fn handle_cancel(&self, args: serde_json::Value) {
        // Just broadcast the cancellation to all running servers.
        // ChildManager does not keep track of request IDs globally.
        // The server will simply ignore the cancellation if it doesn't know the request ID.
        let running_servers = self.child_manager.server_names().await;
        for server in running_servers {
            let _ = self.child_manager.forward_notification(&server, "notifications/cancelled", args.clone()).await;
        }
    }
}

/// Strip noise from inputSchema: remove title, examples, $schema, additionalProperties.
/// Keeps type, properties, required, description (on root only), items, enum.
fn strip_schema(schema: &serde_json::Value) -> serde_json::Value {
    match schema {
        serde_json::Value::Object(map) => {
            let mut clean = serde_json::Map::new();
            for (k, v) in map {
                match k.as_str() {
                    "title" | "examples" | "$schema" | "additionalProperties" | "$id" | "$comment" | "default" => continue,
                    "properties" => {
                        if let Some(props) = v.as_object() {
                            let mut cleaned_props = serde_json::Map::new();
                            for (pk, pv) in props {
                                cleaned_props.insert(pk.clone(), strip_schema(pv));
                            }
                            clean.insert(k.clone(), serde_json::Value::Object(cleaned_props));
                        }
                    }
                    "items" => { clean.insert(k.clone(), strip_schema(v)); }
                    _ => { clean.insert(k.clone(), v.clone()); }
                }
            }
            serde_json::Value::Object(clean)
        }
        other => other.clone(),
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
        "[McpHub][INFO] Preloading {} servers ({}ms stagger)...",
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
                eprintln!("[McpHub][ERROR] Failed to start '{}': {}", name, e);
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

/// Watches schema-cache.json and config.json for changes and hot-reloads them.
async fn config_and_cache_watcher(
    engine: Arc<Mutex<SearchEngine>>,
    config_store: Arc<Mutex<ProxyConfig>>,
    child_manager: Arc<ChildManager>,
) {
    use std::time::SystemTime;

    let cache_path_opt = crate::cache::cache_path();
    let mut last_cache_modified: Option<SystemTime> = cache_path_opt
        .as_ref()
        .and_then(|p| p.metadata().ok())
        .and_then(|m| m.modified().ok());

    let config_path_opt = dirs::home_dir().map(|h| h.join(".McpHub/config.json"));
    let mut last_config_modified: Option<SystemTime> = config_path_opt
        .as_ref()
        .and_then(|p| p.metadata().ok())
        .and_then(|m| m.modified().ok());

    loop {
        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;

        // Check Cache
        if let Some(cache_path) = &cache_path_opt {
            if let Ok(m) = cache_path.metadata() {
                if let Ok(current_modified) = m.modified() {
                    if Some(current_modified) != last_cache_modified {
                        last_cache_modified = Some(current_modified);

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
                            let mut eng = engine.lock().await;
                            eng.build_index(all_tools);
                            eprintln!(
                                "[McpHub][INFO] Cache hot-reloaded: {} tools",
                                eng.tool_count()
                            );
                        }
                    }
                }
            }
        }

        // Check Config
        if let Some(config_path) = &config_path_opt {
            if let Ok(m) = config_path.metadata() {
                if let Ok(current_modified) = m.modified() {
                    if Some(current_modified) != last_config_modified {
                        last_config_modified = Some(current_modified);

                        let new_config = crate::config::auto_detect();
                        let new_servers = new_config.servers.clone();
                        
                        {
                            let mut cfg = config_store.lock().await;
                            *cfg = new_config;
                        }

                        child_manager.update_configs(new_servers).await;
                        eprintln!("[McpHub][INFO] Config hot-reloaded");
                    }
                }
            }
        }
    }
}
