mod cache;
pub mod child;
mod config;
mod dashboard;
mod protocol;
mod proxy;
mod search;

use config::auto_detect;
use proxy::ProxyServer;
use search::{IndexedTool, SearchEngine};

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn print_help() {
    eprintln!(
        r#"
mcp-on-demand v{VERSION} — Fastest MCP proxy with BM25 tool discovery

USAGE:
  mcp-on-demand              Start proxy (loads from cache, instant)
  mcp-on-demand generate     Start all servers, index tools, save cache
  mcp-on-demand dashboard    Open web dashboard on http://127.0.0.1:24680
  mcp-on-demand status       Show detected servers and cache info
  mcp-on-demand search "q"   Test BM25 search
  mcp-on-demand version      Show version
  mcp-on-demand help         Show this help

FIRST TIME SETUP:
  1. Configure servers in ~/.mcp-on-demand/config.json
  2. Run: mcp-on-demand generate    (one-time, ~60s)
  3. Add to Cursor mcp.json
  4. Every startup is instant (<1ms from cache)
"#,
        VERSION = VERSION
    );
}

fn cmd_status() {
    let config = auto_detect();
    println!("mcp-on-demand v{}", VERSION);
    println!("Mode: {:?}", config.mode);
    println!("Servers configured: {}", config.servers.len());

    // Cache info
    if let Some(cached) = cache::load_cache() {
        let total_tools: usize = cached.servers.values().map(|v: &Vec<crate::protocol::ToolDef>| v.len()).sum::<usize>();
        println!("Cache: {} servers, {} tools (v{})", cached.servers.len(), total_tools, cached.version);
    } else {
        println!("Cache: NOT FOUND — run 'mcp-on-demand generate' first");
    }

    println!();
    let mut names: Vec<_> = config.servers.keys().collect();
    names.sort();
    for name in names {
        let s = &config.servers[name];
        let args = s.args.join(" ");
        println!("  {} → {} {}", name, s.command, args);
    }
}

async fn cmd_generate() {
    let config = auto_detect();
    if config.servers.is_empty() {
        eprintln!("No servers found. Add servers to ~/.mcp-on-demand/config.json");
        return;
    }

    let total = config.servers.len();
    eprintln!("Generating cache for {} servers...\n", total);

    let manager = std::sync::Arc::new(child::ChildManager::new(
        config.servers.clone(),
        config.idle_timeout_ms,
    ));

    let mut server_tools: std::collections::HashMap<String, Vec<protocol::ToolDef>> = std::collections::HashMap::new();
    let mut all_tools: Vec<IndexedTool> = Vec::new();
    let mut ok = 0;
    let mut fail = 0;

    let mut names: Vec<String> = config.servers.keys().cloned().collect();
    names.sort();

    for (i, name) in names.iter().enumerate() {
        eprint!("[{}/{}] {} ... ", i + 1, total, name);
        match manager.start_server(name).await {
            Ok(tools) => {
                eprintln!("{} tools ✓", tools.len());
                server_tools.insert(name.clone(), tools.clone());
                for tool in tools {
                    all_tools.push(IndexedTool {
                        name: format!("{}__{}", name, tool.name),
                        original_name: tool.name.clone(),
                        server_name: name.clone(),
                        description: tool.description.clone(),
                        tool_def: tool,
                    });
                }
                ok += 1;
            }
            Err(e) => {
                eprintln!("FAILED: {}", e);
                fail += 1;
            }
        }
    }

    // Build index to verify
    let mut engine = SearchEngine::new();
    engine.build_index(all_tools);

    // Save cache
    cache::save_cache(&server_tools);

    // Stop all servers
    manager.stop_all().await;

    eprintln!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    eprintln!("Done: {} OK, {} failed, {} total tools", ok, fail, engine.tool_count());
    eprintln!("Cache saved to ~/.mcp-on-demand/schema-cache.json");
    eprintln!("Proxy will now start instantly from cache.");
}

fn cmd_search(query: &str) {
    if let Some(cached) = cache::load_cache() {
        let mut engine = SearchEngine::new();
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
        engine.build_index(all_tools);
        let results = engine.search(query, 10);
        println!("Query: \"{}\" ({} tools indexed)", query, engine.tool_count());
        for (i, t) in results.iter().enumerate() {
            println!("  {}. {} (server: {}) — {}", i + 1, t.original_name, t.server_name, &t.description[..t.description.len().min(80)]);
        }
    } else {
        println!("No cache found. Run 'mcp-on-demand generate' first.");
    }
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();

    match args.get(1).map(|s| s.as_str()) {
        Some("help") | Some("--help") | Some("-h") => print_help(),
        Some("version") | Some("--version") | Some("-V") => println!("mcp-on-demand v{}", VERSION),
        Some("status") => cmd_status(),
        Some("generate") => cmd_generate().await,
        Some("dashboard") | Some("ui") | Some("web") => dashboard::start_dashboard().await,
        Some("search") => {
            let query = args.get(2).map(|s| s.as_str()).unwrap_or("*");
            cmd_search(query);
        }
        _ => {
            eprintln!("mcp-on-demand v{} — starting...", VERSION);
            let config = auto_detect();
            let proxy = ProxyServer::new(config);
            proxy.run().await;
        }
    }
}
