# mcp-on-demand v2.0 (Rust)

**Fastest MCP proxy — BM25 tool discovery, lazy-loading, single binary.**

Exposes only 2 tools (`discover` + `execute`) to your LLM instead of 200+.
Sub-microsecond search. ~99% context token savings. Zero external dependencies.

## Why Rust?

| | TypeScript | **Rust** |
|---|---|---|
| Binary | 150MB (node_modules) | **~4MB** |
| Startup | ~150ms | **~5ms** |
| Runtime dep | Node.js 18+ | **None** |
| BM25 search | ~0.1ms | **~0.01ms** |
| Distribution | npm install | **Single binary** |

## Architecture

```
Cursor / Claude Desktop / VS Code (sees 2 tools)
    ↓ stdio (JSON-RPC)
mcp-on-demand (single Rust binary)
  ├─ BM25 Search Index (in-memory, <0.01ms)
  └─ Child Manager (spawn / pool / idle-stop)
    ↓ stdio (JSON-RPC)
Your MCP servers (github, supabase, filesystem...)
```

## Install

### One-line install (no Rust needed)

```bash
curl -fsSL https://raw.githubusercontent.com/Soflution1/mcp-on-demand/main/install.sh | bash
```

Downloads a pre-built binary (~4MB) for your platform. No dependencies.

### From source (requires Rust 1.80+)

```bash
git clone https://github.com/Soflution1/mcp-on-demand.git
cd mcp-on-demand
cargo build --release
cp target/release/mcp-on-demand ~/.local/bin/
```

## Configure

### Cursor IDE (`~/.cursor/mcp.json`)

```json
{
  "mcpServers": {
    "on-demand": {
      "command": "/path/to/mcp-on-demand"
    }
  }
}
```

### Claude Desktop

```json
{
  "mcpServers": {
    "on-demand": {
      "command": "/path/to/mcp-on-demand"
    }
  }
}
```

That's it. No args needed — it auto-detects your other MCP servers from config files.

## How It Works

### Discover Mode (default)

Your LLM sees only 2 tools:

1. **`discover(query)`** — BM25 search across all tools from all servers
2. **`execute(server, tool, arguments)`** — Run any tool on any server

Flow:
```
LLM: "I need to read a file"
  → calls discover("read file")
  → gets: [{server: "filesystem", tool: "read_file", schema: {...}}]
  → calls execute("filesystem", "read_file", {path: "/foo"})
  → gets file content
```

### Passthrough Mode

All tools exposed directly with `server__tool` prefix (legacy mode).

```bash
MCP_ON_DEMAND_MODE=passthrough mcp-on-demand
```

## Auto-Detection

Config files checked (in order):
- `~/.cursor/mcp.json`
- `~/Library/Application Support/Claude/claude_desktop_config.json` (macOS)
- `%APPDATA%/Claude/claude_desktop_config.json` (Windows)
- `~/.config/claude/claude_desktop_config.json` (Linux)
- `~/.codeium/windsurf/mcp_config.json`
- `~/.vscode/mcp.json`

Servers starting with `_` are skipped (disabled convention).
Self-references to `mcp-on-demand` are automatically excluded.

## Environment Variables

| Variable | Values | Default |
|---|---|---|
| `MCP_ON_DEMAND_MODE` | `discover` / `passthrough` | `discover` |
| `MCP_ON_DEMAND_PRELOAD` | `all` / `none` | `all` |
| `MCP_ON_DEMAND_DEBUG` | `1` | - |

## CLI

```bash
mcp-on-demand              # Start proxy (default)
mcp-on-demand status       # Show detected servers
mcp-on-demand search "git" # Test BM25 search
mcp-on-demand version      # Show version
mcp-on-demand help         # Show help
```

## Performance

| Metric | Value |
|---|---|
| Proxy startup | ~5ms |
| BM25 search (200 tools) | <0.01ms |
| Index build (200 tools) | <0.5ms |
| Tool execution overhead | <2ms |
| Binary size (stripped) | ~4MB |
| RAM usage | ~5MB |
| Context token savings | ~99% |

## Dependencies

Production binary: **zero runtime dependencies**.

Build only:
- `tokio` — async runtime
- `serde` / `serde_json` — JSON parsing
- `dirs` — cross-platform home directory detection

## License

MIT — SOFLUTION LTD
