#!/usr/bin/env bash
set -euo pipefail

# TODO: Update when repo is renamed
REPO="adamthewilliam/agentika-grep"
INSTALL_DIR="${HOME}/.local/bin"
BINARY_NAME="grepika"

# ── Platform check ──────────────────────────────────────────────────
OS="$(uname -s)"
ARCH="$(uname -m)"

if [ "$OS" != "Darwin" ] || [ "$ARCH" != "arm64" ]; then
    echo "Error: This installer only supports macOS on Apple Silicon (arm64)."
    echo ""
    echo "Detected: ${OS} ${ARCH}"
    echo ""
    echo "For other platforms, install from source (requires Rust 1.91+):"
    echo "  cargo install --git https://github.com/${REPO}"
    exit 1
fi

# ── Fetch latest release version ────────────────────────────────────
echo "Fetching latest release..."
LATEST_TAG=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
    | grep '"tag_name"' | head -1 | cut -d '"' -f 4)

if [ -z "$LATEST_TAG" ]; then
    echo "Error: Could not determine latest release version."
    echo "Check https://github.com/${REPO}/releases for available versions."
    exit 1
fi

echo "Latest version: ${LATEST_TAG}"

# ── Download ────────────────────────────────────────────────────────
ASSET_NAME="${BINARY_NAME}-aarch64-apple-darwin.tar.gz"
DOWNLOAD_URL="https://github.com/${REPO}/releases/download/${LATEST_TAG}/${ASSET_NAME}"

TMPDIR_INSTALL="$(mktemp -d)"
trap 'rm -rf "$TMPDIR_INSTALL"' EXIT

echo "Downloading ${ASSET_NAME}..."
if ! curl -fsSL -o "${TMPDIR_INSTALL}/${ASSET_NAME}" "$DOWNLOAD_URL"; then
    echo "Error: Failed to download from:"
    echo "  ${DOWNLOAD_URL}"
    echo ""
    echo "Make sure a release with asset '${ASSET_NAME}' exists."
    exit 1
fi

# ── Extract & install ───────────────────────────────────────────────
echo "Extracting..."
tar -xzf "${TMPDIR_INSTALL}/${ASSET_NAME}" -C "$TMPDIR_INSTALL"

mkdir -p "$INSTALL_DIR"
mv "${TMPDIR_INSTALL}/${BINARY_NAME}" "${INSTALL_DIR}/${BINARY_NAME}"
chmod +x "${INSTALL_DIR}/${BINARY_NAME}"

# Remove macOS quarantine flag so Gatekeeper doesn't block the binary
xattr -d com.apple.quarantine "${INSTALL_DIR}/${BINARY_NAME}" 2>/dev/null || true

echo "Installed ${BINARY_NAME} to ${INSTALL_DIR}/${BINARY_NAME}"

# ── PATH guidance ───────────────────────────────────────────────────
case ":${PATH}:" in
    *":${INSTALL_DIR}:"*) ;;
    *)
        echo ""
        echo "WARNING: ${INSTALL_DIR} is not in your PATH."
        echo ""
        echo "Add it by running:"
        echo ""
        SHELL_NAME="$(basename "$SHELL")"
        if [ "$SHELL_NAME" = "zsh" ]; then
            echo "  echo 'export PATH=\"\${HOME}/.local/bin:\${PATH}\"' >> ~/.zshrc"
            echo "  source ~/.zshrc"
        elif [ "$SHELL_NAME" = "bash" ]; then
            echo "  echo 'export PATH=\"\${HOME}/.local/bin:\${PATH}\"' >> ~/.bashrc"
            echo "  source ~/.bashrc"
        else
            echo "  export PATH=\"\${HOME}/.local/bin:\${PATH}\""
        fi
        ;;
esac

echo ""
echo "Done! Run '${BINARY_NAME} --help' to get started."
