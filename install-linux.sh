#!/usr/bin/env bash
set -euo pipefail

PREFIX="${PREFIX:-/usr/local}"
BIN_DIR="$PREFIX/bin"
ICON_DIR="/usr/share/icons/hicolor/256x256/apps"
DESKTOP_DIR="/usr/share/applications"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
BINARY="$SCRIPT_DIR/target/release/s21_hijack"

if [ ! -f "$BINARY" ]; then
    echo "Binary not found at $BINARY"
    echo "Run 'cargo build --release' first."
    exit 1
fi

echo "Installing S21 HiJack..."

install -Dm755 "$BINARY"                            "$BIN_DIR/s21_hijack"
install -Dm644 "$SCRIPT_DIR/assets/icon.png"        "$ICON_DIR/s21_hijack.png"
install -Dm644 "$SCRIPT_DIR/assets/s21_hijack.desktop" "$DESKTOP_DIR/s21_hijack.desktop"

# Refresh icon cache if available
if command -v gtk-update-icon-cache &>/dev/null; then
    gtk-update-icon-cache -f /usr/share/icons/hicolor/ 2>/dev/null || true
fi

# Refresh desktop database if available
if command -v update-desktop-database &>/dev/null; then
    update-desktop-database "$DESKTOP_DIR" 2>/dev/null || true
fi

echo "Installed successfully. You may need to log out and back in for the icon to appear."
