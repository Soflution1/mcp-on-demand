#!/bin/bash
# Install mcp-on-demand - single binary MCP proxy
# Usage: curl -fsSL https://raw.githubusercontent.com/Soflution1/mcp-on-demand/main/install.sh | bash

set -e

REPO="Soflution1/mcp-on-demand"
INSTALL_DIR="${HOME}/.local/bin"

# Detect platform
OS=$(uname -s | tr '[:upper:]' '[:lower:]')
ARCH=$(uname -m)

case "${OS}-${ARCH}" in
  darwin-arm64)  ASSET="mcp-on-demand-macos-arm.tar.gz" ;;
  darwin-x86_64) ASSET="mcp-on-demand-macos-intel.tar.gz" ;;
  linux-x86_64)  ASSET="mcp-on-demand-linux-amd64.tar.gz" ;;
  *)
    echo "Unsupported platform: ${OS}-${ARCH}"
    echo "Download manually from https://github.com/${REPO}/releases"
    exit 1
    ;;
esac

# Get latest release URL
LATEST=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" | grep "browser_download_url.*${ASSET}" | cut -d '"' -f 4)

if [ -z "$LATEST" ]; then
  echo "Error: Could not find release for ${ASSET}"
  exit 1
fi

echo "Downloading mcp-on-demand..."
echo "  Platform: ${OS}/${ARCH}"
echo "  URL: ${LATEST}"

# Download and extract
mkdir -p "${INSTALL_DIR}"
curl -fsSL "${LATEST}" | tar xz -C "${INSTALL_DIR}"
chmod +x "${INSTALL_DIR}/mcp-on-demand"

echo ""
echo "Installed to ${INSTALL_DIR}/mcp-on-demand"

# Check PATH
if ! echo "$PATH" | grep -q "${INSTALL_DIR}"; then
  echo ""
  echo "Add to your PATH:"
  echo "  export PATH=\"${INSTALL_DIR}:\$PATH\""
  echo ""
  echo "Add this line to ~/.zshrc or ~/.bashrc to make it permanent."
fi

echo ""
echo "Verify: mcp-on-demand version"
echo "Status: mcp-on-demand status"
