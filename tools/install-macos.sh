#!/usr/bin/env bash
# tools/install-macos.sh
# macOS native messaging host installer for tur-overlay.
#
# Installs the native messaging host manifest so the browser extension
# can launch the tur-overlay daemon.
#
# Usage:
#   ./tools/install-macos.sh                   # interactive (choose targets)
#   ./tools/install-macos.sh --all             # install for all known browsers
#   ./tools/install-macos.sh --chrome          # Chrome only
#   ./tools/install-macos.sh --firefox         # Firefox only
#   ./tools/install-macos.sh --uninstall       # remove all manifests
#
# The daemon binary is built via `cargo build` and symlinked / copied
# into each browser's native messaging host path.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

MANIFEST_NAME="com.tur.overlay.json"

# ── Browser manifest destinations ────────────────────────────────────────────

declare -A BROWSER_DEST
declare -A BROWSER_NAME

# Chrome
BROWSER_DEST["chrome"]="$HOME/Library/Application Support/Google/Chrome/NativeMessagingHosts"
BROWSER_NAME["chrome"]="Google Chrome"

# Chromium
BROWSER_DEST["chromium"]="$HOME/Library/Application Support/Chromium/NativeMessagingHosts"
BROWSER_NAME["chromium"]="Chromium"

# Brave
BROWSER_DEST["brave"]="$HOME/Library/Application Support/BraveSoftware/Brave-Browser/NativeMessagingHosts"
BROWSER_NAME["brave"]="Brave"

# Firefox
BROWSER_DEST["firefox"]="$HOME/Library/Application Support/Mozilla/NativeMessagingHosts"
BROWSER_NAME["firefox"]="Firefox"

# ── Helpers ──────────────────────────────────────────────────────────────────

print_usage() {
    echo "Usage: $0 [OPTIONS]"
    echo ""
    echo "Options:"
    echo "  --all            Install for all known browsers"
    echo "  --chrome         Google Chrome only"
    echo "  --chromium       Chromium only"
    echo "  --brave          Brave only"
    echo "  --firefox        Firefox only"
    echo "  --uninstall      Remove all installed manifests (does not remove binary)"
    echo "  --help           Show this help"
    echo ""
    echo "Without options, you will be prompted to select browsers interactively."
}

is_darwin() {
    [[ "$(uname)" == "Darwin" ]]
}

if ! is_darwin; then
    echo "error: this script is for macOS only" >&2
    exit 1
fi

# ── Build the binary ─────────────────────────────────────────────────────────

build_binary() {
    echo "==> Building tur-overlay daemon for macOS..."
    cd "$PROJECT_DIR"
    cargo build --release --manifest-path crates/host/Cargo.toml
    local binary="$PROJECT_DIR/target/release/tur-overlay-host"
    if [ ! -f "$binary" ]; then
        echo "error: build succeeded but binary not found at $binary" >&2
        exit 1
    fi
    echo "==> Built: $binary"
    echo "$binary"
}

# ── Native messaging host manifest generators ────────────────────────────────

generate_chrome_manifest() {
    local binary_path="$1"
    cat <<EOF
{
  "name": "com.tur.overlay",
  "description": "tur — overlay AI download assistant",
  "path": "$binary_path",
  "type": "stdio",
  "allowed_origins": [
    "chrome-extension://YOUR_CHROME_EXTENSION_ID/"
  ]
}
EOF
}

generate_firefox_manifest() {
    local binary_path="$1"
    cat <<EOF
{
  "name": "com.tur.overlay",
  "description": "tur — overlay AI download assistant",
  "path": "$binary_path",
  "type": "stdio",
  "allowed_extensions": ["tur@tur-overlay"]
}
EOF
}

# ── Install for one browser ──────────────────────────────────────────────────

install_for_browser() {
    local browser="$1"
    local binary_path="$2"
    local dest="${BROWSER_DEST[$browser]}"

    if [ -z "$dest" ]; then
        echo "error: unknown browser '$browser'" >&2
        return 1
    fi

    echo "==> Installing for ${BROWSER_NAME[$browser]}..."

    mkdir -p "$dest"

    if [[ "$browser" == "firefox" ]]; then
        generate_firefox_manifest "$binary_path" > "$dest/$MANIFEST_NAME"
    else
        generate_chrome_manifest "$binary_path" > "$dest/$MANIFEST_NAME"
    fi

    echo "    Manifest written: $dest/$MANIFEST_NAME"
    echo "    Binary path:      $binary_path"
    echo "    Done."
}

# ── Uninstall ────────────────────────────────────────────────────────────────

uninstall_all() {
    echo "==> Removing tur-overlay native messaging host manifests..."
    for browser in "${!BROWSER_DEST[@]}"; do
        local dest="${BROWSER_DEST[$browser]}"
        local manifest="$dest/$MANIFEST_NAME"
        if [ -f "$manifest" ]; then
            rm "$manifest"
            echo "    Removed: $manifest"
        fi
    done
    echo "==> Uninstall complete. Binary left in place."
}

# ── Interactive selection ────────────────────────────────────────────────────

interactive_select() {
    echo "Select browsers to install for (enter comma-separated numbers, or 'all'):"
    echo ""
    local i=0
    local names=()
    for key in "chrome" "chromium" "brave" "firefox"; do
        i=$((i + 1))
        names+=("$key")
        echo "  $i) ${BROWSER_NAME[$key]}"
    done
    echo ""
    echo "  a) All"
    echo "  q) Cancel"
    echo ""
    read -rp "Choice: " choice

    if [[ "$choice" == "q" ]]; then
        echo "Cancelled."
        exit 0
    fi

    # Build once before any installations
    local binary_path
    binary_path="$(build_binary)"

    if [[ "$choice" == "a" ]] || [[ "$choice" == "all" ]]; then
        for key in "${names[@]}"; do
            install_for_browser "$key" "$binary_path"
        done
        return
    fi

    IFS=',' read -ra selections <<< "$choice"
    for sel in "${selections[@]}"; do
        sel="${sel// /}"
        local idx=$((sel - 1))
        if [ "$idx" -ge 0 ] && [ "$idx" -lt "${#names[@]}" ]; then
            install_for_browser "${names[$idx]}" "$binary_path"
        else
            echo "warning: invalid selection '$sel'" >&2
        fi
    done
}

# ── Main ─────────────────────────────────────────────────────────────────────

if ! is_darwin; then
    echo "error: this script is for macOS only" >&2
    exit 1
fi

MODE="${1:-interactive}"

case "$MODE" in
    --all)
        echo "==> Installing for all browsers..."
        echo ""
        local bp
        bp="$(build_binary)"
        for key in "chrome" "chromium" "brave" "firefox"; do
            install_for_browser "$key" "$bp"
        done
        echo ""
        echo "==> All done! Restart your browser(s) to activate the native host."
        ;;
    --chrome|--chromium|--brave|--firefox)
        browser="${MODE#--}"
        local bp2
        bp2="$(build_binary)"
        install_for_browser "$browser" "$bp2"
        echo ""
        echo "==> Done! Restart ${BROWSER_NAME[$browser]} to activate the native host."
        ;;
    --uninstall)
        uninstall_all
        ;;
    --help|-h)
        print_usage
        ;;
    *)
        interactive_select
        ;;
esac
