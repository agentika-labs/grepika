#!/bin/bash
# agentika-grep installer
set -e

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

info() { echo -e "${GREEN}[INFO]${NC} $1"; }
warn() { echo -e "${YELLOW}[WARN]${NC} $1"; }
error() { echo -e "${RED}[ERROR]${NC} $1"; exit 1; }

# Check for cargo
if ! command -v cargo &> /dev/null; then
    error "cargo is not installed. Please install Rust: https://rustup.rs"
fi

# Check Rust version
RUST_VERSION=$(rustc --version | grep -oE '[0-9]+\.[0-9]+')
REQUIRED_VERSION="1.75"

if [ "$(printf '%s\n' "$REQUIRED_VERSION" "$RUST_VERSION" | sort -V | head -n1)" != "$REQUIRED_VERSION" ]; then
    error "Rust $REQUIRED_VERSION or higher is required. Current version: $RUST_VERSION"
fi

info "Installing agentika-grep..."

# Build release
cargo build --release

# Install to cargo bin
cargo install --path .

info "Installation complete!"
echo ""
echo "Usage:"
echo "  agentika-grep --mcp --root /path/to/project  # Start MCP server"
echo "  agentika-grep search 'pattern'               # Search in current directory"
echo "  agentika-grep index                          # Index the codebase"
echo "  agentika-grep --help                         # Show all options"
