<p align="center">
  <img src="static/banner.svg" alt="McpHub" width="900"/>
</p>

<p align="center">
  <strong>One proxy to rule all your MCP servers.</strong><br>
  <sub>Built-in web dashboard · ~99% context token savings · Zero dependencies</sub>
</p>

<p align="center">
  <a href="#install"><img src="https://img.shields.io/badge/install-30s-brightgreen" alt="Install"/></a>
  <img src="https://img.shields.io/badge/language-Rust-orange" alt="Rust"/>
  <img src="https://img.shields.io/badge/license-MIT-blue" alt="MIT"/>
  <img src="https://img.shields.io/badge/binary-~900KB-yellow" alt="Binary size"/>
</p>

---

## Install

### From source

```bash
git clone https://github.com/Soflution1/McpHub.git
cd McpHub
./install.sh
```

The install script will:
1. Build the release binary (~900KB)
2. Install to `~/.local/bin/McpHub`
3. Codesign for macOS (required since Sequoia)
4. Generate the tool cache

**Restart Cursor** and you're done.

> **macOS note:** Every time you rebuild, run `codesign --force --sign -` on the binary.
> The `install.sh` script handles this automatically.

## Dashboard

Open `http://127.0.0.1:24680` or run:

```bash
McpHub dashboard
```

The dashboard lets you:
- **Add servers** by pasting JSON from any MCP server README
- **Edit servers** with syntax-highlighted JSON (tokens/secrets highlighted in red)
- **Enable/disable** servers with a toggle
- **Rebuild cache** in one click
- **Monitor** token savings, cached vs failed servers

> **Bookmark `http://127.0.0.1:24680`** for quick access.

## How It Works

**Before:** Cursor loads 20+ MCP servers = 200+ tool definitions = ~20,000 tokens per request.

**After:** Cursor loads 1 proxy = 2 tools = ~160 tokens. Savings: **99%**.

```
Cursor (sees only 2 tools: discover + execute)
    |
McpHub (BM25 search index, <0.01ms)
    |
Your MCP servers (spawned on demand, killed when idle)
```

### Discover mode (default)

1. LLM calls `discover("send email")`
2. Proxy searches across all 200+ tools using BM25
3. Returns matching tools with full schemas + complete server list
4. LLM calls `execute("resend", "send-email", {to: "...", ...})`
5. Proxy spawns the server (if not running), calls the tool, returns result

Server names are resolved case-insensitively (e.g. `MemoryPilot`, `memory-pilot`, `memorypilot` all match).

### Passthrough mode

All tools exposed directly with `server__tool` prefix. Full visibility, higher token cost.

## CLI

```bash
McpHub                  # Start proxy (stdio, used by Cursor)
McpHub dashboard        # Open web dashboard
McpHub generate         # Rebuild tool cache
McpHub status           # Show detected servers
McpHub search "git"     # Test BM25 search
McpHub version          # Show version
```

## Configuration

Config lives in `~/.McpHub/config.json`:

```json
{
  "servers": {
    "github": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-github"],
      "env": { "GITHUB_TOKEN": "ghp_xxx" }
    }
  },
  "settings": {
    "mode": "discover",
    "idleTimeout": 300
  }
}
```

Cursor config (`~/.cursor/mcp.json`) just needs:

```json
{
  "mcpServers": {
    "McpHub": {
      "command": "/Users/you/.local/bin/McpHub"
    }
  }
}
```

## Performance

| Metric | Value |
|---|---|
| Binary size | ~900KB |
| Startup | <5ms |
| BM25 search (400+ tools) | <0.01ms |
| Context token savings | ~99% |
| RAM usage | ~5MB |
| Runtime dependencies | **None** |

## Environment Variables

| Variable | Values | Default |
|---|---|---|
| `MCP_ON_DEMAND_MODE` | `discover` / `passthrough` | `discover` |
| `MCP_ON_DEMAND_PRELOAD` | `all` / `none` | `all` |
| `MCP_ON_DEMAND_DEBUG` | `1` | - |

## Always-on Dashboard (macOS)

Run the dashboard 24/7 as a background service:

```bash
cat > ~/Library/LaunchAgents/com.soflution.mcphub-dashboard.plist << 'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.soflution.mcphub-dashboard</string>
    <key>ProgramArguments</key>
    <array>
        <string>/Users/you/.local/bin/McpHub</string>
        <string>dashboard</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
</dict>
</plist>
EOF

launchctl load ~/Library/LaunchAgents/com.soflution.mcphub-dashboard.plist
```

## Uninstall

```bash
rm ~/.local/bin/McpHub
rm -rf ~/.McpHub
launchctl unload ~/Library/LaunchAgents/com.soflution.mcphub-dashboard.plist 2>/dev/null
rm -f ~/Library/LaunchAgents/com.soflution.mcphub-dashboard.plist
```

## License

MIT - [SOFLUTION LTD](https://soflution.com)
