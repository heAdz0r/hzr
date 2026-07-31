#!/bin/bash
# Install RTK from a local release build (builds from source, no network download).

set -euo pipefail

INSTALL_DIR="${1:-$HOME/.cargo/bin}"
INSTALL_PATH="${INSTALL_DIR}/rtk"
BINARY_PATH="./target/release/rtk"

if ! command -v cargo &>/dev/null; then
    echo "error: cargo not found"
    echo "install Rust: https://rustup.rs"
    exit 1
fi

echo "installing to: $INSTALL_DIR"
if [ -f "$BINARY_PATH" ] && [ -z "$(find src/ Cargo.toml Cargo.lock -newer "$BINARY_PATH" -print -quit 2>/dev/null)" ]; then
    echo "binary is up to date"
else
    echo "building rtk (release)..."
    cargo build --release
fi

mkdir -p "$INSTALL_DIR"
install -m 755 "$BINARY_PATH" "$INSTALL_PATH"

echo "installed: $INSTALL_PATH"
echo "version: $("$INSTALL_PATH" --version)"

# Also sync /usr/local/bin/rtk if it exists and is writable (no rdesync on PATH-priority conflict)
USR_LOCAL="/usr/local/bin/rtk"
if [ -f "$USR_LOCAL" ] && [ -w "$USR_LOCAL" ]; then
    install -m 755 "$BINARY_PATH" "$USR_LOCAL"
    echo "synced:    $USR_LOCAL"
elif [ -w "/usr/local/bin" ]; then
    install -m 755 "$BINARY_PATH" "$USR_LOCAL"
    echo "installed: $USR_LOCAL"
else
    echo "note: cannot write $USR_LOCAL (run with sudo or use rtk build sh)"
fi

case ":$PATH:" in
    *":$INSTALL_DIR:"*) ;;
    *) echo
       echo "warning: $INSTALL_DIR is not in your PATH"
       echo "add this to your shell profile:"
       echo "  export PATH=\"\$PATH:$INSTALL_DIR\""
       ;;
esac
