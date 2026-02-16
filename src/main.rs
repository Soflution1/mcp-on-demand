mod child;
mod config;
mod protocol;
mod proxy;
mod search;

use config::auto_detect;
use proxy::ProxyServer;
use search::SearchEngine;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn print_help() {
    eprintln!(
        r#"
mcp-on-demand v{VERSION} — Fastest MCP proxy with BM25 tool discovery

USAGE:
  mcp-on-demand              Start proxy (default: discover mode)
  mcp-on-demand status       Show detected servers
  mcp-on-demand search "q"   Test BM25 search engine
  mcp-on-demand version      Show version
  mcp-on-demand help         Show this help

ENVIRONMENT:
  MCP_ON_DEMAND_MODE=discover|passthrough   Proxy mode (default: discover)
  MCP_ON_DEMAND_PRELOAD=all|none            Preload strategy (default: all)
  MCP_ON_DEMAND_DEBUG=1                     Enable debug logging

CONFIG (auto-detected):
  ~/.cursor/mcp.json
  ~/Library/Application Support/Claude/claude_desktop_config.json
"#,
        VERSION = VERSION
    );
}

fn cmd_status() {
    let config = auto_detect();
    println!("mcp-on-demand v{}", VERSION);
    println!("Mode: {:?}", config.mode);
    println!("Preload: {:?}", config.preload);
    println!("Servers: {}", config.servers.len());
    println!();

    let mut names: Vec<_> = config.servers.keys().collect();
    names.sort();
    for name in names {
        let s = &config.servers[name];
        let args = s.args.join(" ");
        println!("  {} → {} {}", name, s.command, args);
    }
}

fn cmd_search(query: &str) {
    let config = auto_detect();
    if config.servers.is_empty() {
        println!("No servers configured.");
        return;
    }

    println!(
        "Note: search test uses tool names/descriptions from config only (servers not started)."
    );
    println!("In production, tools are indexed after preload.\n");

    // Build a minimal index from server names
    let mut engine = SearchEngine::new();
    let tools: Vec<search::IndexedTool> = config
        .servers
        .keys()
        .map(|name| search::IndexedTool {
            name: format!("{}__placeholder", name),
            original_name: name.clone(),
            server_name: name.clone(),
            description: format!("MCP server: {}", name),
            tool_def: protocol::ToolDef {
                name: name.clone(),
                description: format!("MCP server: {}", name),
                input_schema: serde_json::json!({}),
            },
        })
        .collect();

    engine.build_index(tools);
    let results = engine.search(query, 10);

    println!("Query: \"{}\"", query);
    println!("Results: {}", results.len());
    for (i, t) in results.iter().enumerate() {
        println!("  {}. {} (server: {})", i + 1, t.original_name, t.server_name);
    }
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();

    match args.get(1).map(|s| s.as_str()) {
        Some("help") | Some("--help") | Some("-h") => {
            print_help();
        }
        Some("version") | Some("--version") | Some("-V") => {
            println!("mcp-on-demand v{}", VERSION);
        }
        Some("status") => {
            cmd_status();
        }
        Some("search") => {
            let query = args.get(2).map(|s| s.as_str()).unwrap_or("*");
            cmd_search(query);
        }
        _ => {
            // Default: run proxy
            eprintln!("mcp-on-demand v{} — starting...", VERSION);
            let config = auto_detect();
            let proxy = ProxyServer::new(config);
            proxy.run().await;
        }
    }
}
