# ⚡ mcp-on-demand

**Lazy-loading MCP proxy with Tool Search for Cursor IDE** — save GBs of RAM and 85% of context tokens.

## The Problem

Every MCP server in your Cursor config starts immediately and stays running forever. With 10+ servers, that's **5-10 GB of RAM** wasted on servers you're not even using. On top of that, all tool definitions are loaded into the model's context window, consuming **30-50% of available tokens** before you even type a prompt.

## The Solution

`mcp-on-demand` sits between Cursor and your MCP servers. It provides two key optimizations:

1. **Lazy server loading** — servers start only when needed, saving GBs of RAM
2. **Tool Search mode** (v1.2.0) — exposes just 2 meta-tools instead of hundreds, reducing context token usage by ~85%

**Before:** 22 servers, 80 processes, 9.6 GB RAM, ~80K tokens of tool definitions  
**After:** 1 proxy, ~50 MB RAM, 2 meta-tools (~5K tokens)

## Installation (30 seconds)

Add this to your `~/.cursor/mcp.json` alongside your existing servers:

```json
{
  "mcpServers": {
    "mcp-on-demand": {
      "command": "npx",
      "args": ["-y", "@soflution/mcp-on-demand"]
    }
  }
}
```

Restart Cursor. Done.

On first launch, the proxy automatically reads your other MCP servers from the same config file, briefly starts each one to discover its tools, caches the schemas, and then shuts them all down. Subsequent starts are instant.

## How It Works

### Tool Search Mode (default)

Instead of exposing all 200+ tools to Cursor, the proxy exposes just **2 meta-tools**:

- **`search_tools`** — Search across all available tools by keyword, capability, or server name. Returns matching tools with their full schemas.
- **`use_tool`** — Call any discovered tool by name with its arguments.

This mirrors Claude Code's native MCP Tool Search feature (shipped January 2026), bringing the same 85% context reduction to Cursor.

```
User: "Create a branch on GitHub"

Cursor calls search_tools({query: "git branch"})
  → Returns: create_branch, list_branches, delete_branch with full schemas

Cursor calls use_tool({tool_name: "create_branch", arguments: {repo: "...", branch: "..."}})
  → Proxy spawns GitHub MCP server on-demand, executes tool, returns result
```

### Passthrough Mode

For users who prefer the classic behavior (all tools exposed directly):

```json
{
  "mcpServers": {
    "mcp-on-demand": {
      "command": "npx",
      "args": ["-y", "@soflution/mcp-on-demand", "--mode", "passthrough"]
    }
  }
}
```

### Architecture

```
Cursor <-stdio-> mcp-on-demand proxy <-stdio-> MCP Servers (spawned on-demand)
                     |
              Schema Cache (~50 MB)
              Tool Search Index
              (all tools indexed)
```

1. Proxy starts with cached tool schemas (~50 MB RAM)
2. **Tool Search mode:** Cursor sees 2 meta-tools (search_tools + use_tool)
3. **Passthrough mode:** Cursor sees all tools directly from cache
4. Either way: servers only spawn when a tool is actually called
5. Server idles for 5 min -> proxy kills it automatically

## What Gets Proxied

- **Proxied:** All stdio-based MCP servers (npx, node, python, etc.)
- **Skipped:** URL-based servers (like Vercel MCP), disabled servers, and the proxy itself
- Skipped servers continue working normally through Cursor's native handling

## Optional CLI Commands

```bash
npx @soflution/mcp-on-demand status   # Show detected servers, cache, mode info
npx @soflution/mcp-on-demand reset    # Clear cache (forces re-discovery)
npx @soflution/mcp-on-demand help     # Show help
```

## Configuration

The proxy works with zero configuration. For advanced users, create `~/.mcp-on-demand/config.json`:

```json
{
  "settings": {
    "mode": "tool-search",
    "idleTimeout": 300,
    "logLevel": "info",
    "startupTimeout": 30000,
    "prefixTools": false
  }
}
```

| Setting | Default | Description |
|---------|---------|-------------|
| `mode` | `tool-search` | `tool-search` (2 meta-tools) or `passthrough` (all tools direct) |
| `idleTimeout` | 300 | Seconds before stopping an idle server |
| `logLevel` | info | debug, info, warn, error, silent |
| `startupTimeout` | 30000 | Max ms to wait for a server to start |
| `prefixTools` | false | Prefix tool names with server name (passthrough mode) |

## Changelog

### v1.2.0 — Tool Search Mode
- **NEW:** Tool Search mode (default) — exposes 2 meta-tools instead of all tools
- **NEW:** `search_tools` meta-tool with BM25-style keyword matching
- **NEW:** `use_tool` meta-tool for calling discovered tools
- **NEW:** `--mode` CLI flag to switch between tool-search and passthrough
- Context token reduction of ~85% in Cursor
- Server capability catalog auto-generated from cached schemas

### v1.1.0 — Initial Release
- Auto-detection of Cursor MCP config
- On-demand server spawning with idle timeout
- Schema caching for instant startup
- Duplicate tool detection and collision handling

## Requirements

- Node.js 18+
- Cursor IDE with MCP servers configured

## License

MIT — SOFLUTION LTD
