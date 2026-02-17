#!/bin/bash
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
#  mcp-on-demand installer
#  One command to replace all your MCP servers
#  with a single intelligent proxy.
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
set -e

REPO="Soflution1/mcp-on-demand"
INSTALL_DIR="${HOME}/.local/bin"
CONFIG_DIR="${HOME}/.mcp-on-demand"
CURSOR_CONFIG="${HOME}/.cursor/mcp.json"
BINARY="${INSTALL_DIR}/mcp-on-demand"
DASHBOARD_URL="http://127.0.0.1:24680"

BOLD='\033[1m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
CYAN='\033[0;36m'
RED='\033[0;31m'
DIM='\033[2m'
NC='\033[0m'

echo ""
echo -e "${BOLD}${CYAN}  ╔══════════════════════════════════╗${NC}"
echo -e "${BOLD}${CYAN}  ║     MCP on Demand Installer      ║${NC}"
echo -e "${BOLD}${CYAN}  ╚══════════════════════════════════╝${NC}"
echo ""

# ── Step 1: Download binary ──────────────────
echo -e "${BOLD}[1/4]${NC} Downloading binary..."

OS=$(uname -s | tr '[:upper:]' '[:lower:]')
ARCH=$(uname -m)

case "${OS}-${ARCH}" in
  darwin-arm64)  ASSET="mcp-on-demand-macos-arm.tar.gz" ;;
  darwin-x86_64) ASSET="mcp-on-demand-macos-intel.tar.gz" ;;
  linux-x86_64)  ASSET="mcp-on-demand-linux-amd64.tar.gz" ;;
  *)
    echo -e "${RED}Unsupported platform: ${OS}-${ARCH}${NC}"
    echo "Build from source: cargo build --release"
    exit 1
    ;;
esac

LATEST=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" 2>/dev/null | grep "browser_download_url.*${ASSET}" | cut -d '"' -f 4)

if [ -z "$LATEST" ]; then
  echo -e "${YELLOW}No pre-built binary found. Building from source...${NC}"
  if ! command -v cargo &>/dev/null; then
    echo -e "${RED}Rust not installed. Install from https://rustup.rs${NC}"
    exit 1
  fi
  TMPDIR=$(mktemp -d)
  git clone --depth 1 "https://github.com/${REPO}.git" "$TMPDIR/mcp-on-demand"
  cd "$TMPDIR/mcp-on-demand"
  cargo build --release
  mkdir -p "${INSTALL_DIR}"
  cp target/release/mcp-on-demand "${BINARY}"
  rm -rf "$TMPDIR"
else
  mkdir -p "${INSTALL_DIR}"
  curl -fsSL "${LATEST}" | tar xz -C "${INSTALL_DIR}"
fi

chmod +x "${BINARY}"
echo -e "  ${GREEN}✓${NC} Installed to ${BINARY}"

# ── Step 2: Import MCP servers from Cursor ────
echo ""
echo -e "${BOLD}[2/4]${NC} Detecting MCP servers..."

mkdir -p "${CONFIG_DIR}"

IMPORTED=0

# Check for existing mcp-on-demand config
if [ -f "${CONFIG_DIR}/config.json" ]; then
  EXISTING=$(python3 -c "
import json
with open('${CONFIG_DIR}/config.json') as f:
    d=json.load(f)
s=d.get('servers',d.get('mcpServers',{}))
print(len(s))
" 2>/dev/null || echo "0")
  echo -e "  ${GREEN}✓${NC} Existing config found: ${EXISTING} servers"
  IMPORTED=$EXISTING
fi

# Import from Cursor config if exists and no existing config
if [ -f "$CURSOR_CONFIG" ] && [ "$IMPORTED" = "0" ]; then
  echo -e "  Found Cursor config: ${DIM}${CURSOR_CONFIG}${NC}"
  
  # Backup original
  cp "$CURSOR_CONFIG" "${CONFIG_DIR}/cursor-backup.json"
  echo -e "  ${GREEN}✓${NC} Backed up original to ${DIM}${CONFIG_DIR}/cursor-backup.json${NC}"
  
  # Extract servers (excluding mcp-on-demand itself)
  python3 -c "
import json, sys

with open('${CURSOR_CONFIG}') as f:
    cursor = json.load(f)

src = cursor.get('mcpServers', cursor.get('servers', {}))
servers = {}
skip = ['mcp-on-demand', 'on-demand', 'on_demand']

for name, cfg in src.items():
    if name in skip:
        continue
    cmd = cfg.get('command', '')
    if 'mcp-on-demand' in cmd:
        continue
    servers[name] = cfg

config = {
    'servers': servers,
    'settings': {
        'mode': 'discover',
        'idleTimeout': 300
    }
}

with open('${CONFIG_DIR}/config.json', 'w') as f:
    json.dump(config, f, indent=2)

print(len(servers))
" 2>/dev/null
  
  IMPORTED=$(python3 -c "
import json
with open('${CONFIG_DIR}/config.json') as f:
    d=json.load(f)
print(len(d.get('servers',{})))
" 2>/dev/null || echo "0")
  
  echo -e "  ${GREEN}✓${NC} Imported ${BOLD}${IMPORTED} servers${NC} from Cursor"
  
  # Replace Cursor config with just mcp-on-demand
  python3 -c "
import json
config = {
    'mcpServers': {
        'on-demand': {
            'command': '${BINARY}'
        }
    }
}
with open('${CURSOR_CONFIG}', 'w') as f:
    json.dump(config, f, indent=2)
" 2>/dev/null
  
  echo -e "  ${GREEN}✓${NC} Updated Cursor config: all servers now go through mcp-on-demand"
  
else
  if [ "$IMPORTED" = "0" ]; then
    # No config found anywhere, create empty
    echo -e "  ${YELLOW}No Cursor config found.${NC}"
    echo -e "  ${DIM}Add servers via the dashboard or paste JSON config.${NC}"
    python3 -c "
import json
config = {'servers': {}, 'settings': {'mode': 'discover', 'idleTimeout': 300}}
with open('${CONFIG_DIR}/config.json', 'w') as f:
    json.dump(config, f, indent=2)
" 2>/dev/null
  fi
fi

# ── Step 3: Generate cache ────────────────────
echo ""
echo -e "${BOLD}[3/4]${NC} Generating tool cache..."

if [ "$IMPORTED" != "0" ]; then
  "${BINARY}" generate 2>/dev/null && echo -e "  ${GREEN}✓${NC} Cache built successfully" || echo -e "  ${YELLOW}⚠${NC} Cache generation had issues (run dashboard to retry)"
else
  echo -e "  ${DIM}Skipped (no servers to cache yet)${NC}"
fi

# ── Step 4: Add Cursor config if not done ─────
echo ""
echo -e "${BOLD}[4/4]${NC} Finalizing..."

# Ensure Cursor config exists with mcp-on-demand
if [ ! -f "$CURSOR_CONFIG" ]; then
  mkdir -p "$(dirname "$CURSOR_CONFIG")"
  python3 -c "
import json
config = {'mcpServers': {'on-demand': {'command': '${BINARY}'}}}
with open('${CURSOR_CONFIG}', 'w') as f:
    json.dump(config, f, indent=2)
" 2>/dev/null
  echo -e "  ${GREEN}✓${NC} Created Cursor config"
fi

# Check PATH
if ! echo "$PATH" | grep -q "${INSTALL_DIR}"; then
  SHELL_RC=""
  if [ -f "$HOME/.zshrc" ]; then SHELL_RC="$HOME/.zshrc"
  elif [ -f "$HOME/.bashrc" ]; then SHELL_RC="$HOME/.bashrc"
  fi
  if [ -n "$SHELL_RC" ]; then
    echo "export PATH=\"${INSTALL_DIR}:\$PATH\"" >> "$SHELL_RC"
    echo -e "  ${GREEN}✓${NC} Added to PATH in ${DIM}${SHELL_RC}${NC}"
  else
    echo -e "  ${YELLOW}Add to PATH:${NC} export PATH=\"${INSTALL_DIR}:\$PATH\""
  fi
fi

# ── Done! ─────────────────────────────────────
echo ""
echo ""
echo -e "${GREEN}  ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${GREEN}  ${BOLD}✓ Installation complete!${NC}"
echo -e "${GREEN}  ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""
echo -e "  ${BOLD}${IMPORTED} MCP servers${NC} imported and ready."
echo ""
echo -e "  ${BOLD}${CYAN}Dashboard:${NC}"
echo -e "  ${BOLD}${CYAN}  ${DASHBOARD_URL}${NC}"
echo ""
echo -e "  ${YELLOW}★ Bookmark this URL! ${NC}${DIM}(it's your control panel)${NC}"
echo ""
echo -e "  ${DIM}Quick commands:${NC}"
echo -e "  ${DIM}  mcp-on-demand dashboard    Open the web dashboard${NC}"
echo -e "  ${DIM}  mcp-on-demand status        Show server status${NC}"
echo -e "  ${DIM}  mcp-on-demand generate      Rebuild tool cache${NC}"
echo ""
echo -e "  ${DIM}Restart Cursor to activate.${NC}"
echo ""

# Auto-open dashboard
if [ "$IMPORTED" != "0" ]; then
  echo -e "  Opening dashboard..."
  "${BINARY}" dashboard &>/dev/null &
  sleep 1
  if command -v open &>/dev/null; then
    open "${DASHBOARD_URL}"
  elif command -v xdg-open &>/dev/null; then
    xdg-open "${DASHBOARD_URL}"
  fi
fi
