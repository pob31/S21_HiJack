#!/usr/bin/env bash
set -euo pipefail

PREFIX="${PREFIX:-/usr/local}"
BIN_DIR="$PREFIX/bin"
ICON_DIR="/usr/share/icons/hicolor/256x256/apps"
DESKTOP_DIR="/usr/share/applications"
MIME_DIR="/usr/share/mime/packages"
UDEV_DIR="/etc/udev/rules.d"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

# Locate the binary: next to this script first (the release tarball layout),
# then the in-repo build output (after `cargo build --release`).
if [ -f "$SCRIPT_DIR/s21_hijack" ]; then
    BINARY="$SCRIPT_DIR/s21_hijack"
elif [ -f "$SCRIPT_DIR/target/release/s21_hijack" ]; then
    BINARY="$SCRIPT_DIR/target/release/s21_hijack"
else
    echo "Binary not found next to this script or at target/release/s21_hijack"
    echo "Build it first with 'cargo build --release', or run from the release tarball."
    exit 1
fi

# Assets live next to this script in both layouts (repo root / tarball root).
ASSETS="$SCRIPT_DIR/assets"

echo "Installing S21 HiJack..."

install -Dm755 "$BINARY"                    "$BIN_DIR/s21_hijack"
install -Dm644 "$ASSETS/icon.png"           "$ICON_DIR/s21_hijack.png"
install -Dm644 "$ASSETS/s21_hijack.desktop" "$DESKTOP_DIR/s21_hijack.desktop"

# MIME type for .s21show show files (enables double-click-to-open).
if [ -f "$ASSETS/s21_hijack-mime.xml" ]; then
    install -Dm644 "$ASSETS/s21_hijack-mime.xml" "$MIME_DIR/s21_hijack.xml"
fi

# udev rule for non-root Stream Deck (HID) access. Optional hardware — a
# missing rules directory is not fatal.
if [ -f "$ASSETS/udev/40-elgato-streamdeck.rules" ] && [ -d "$UDEV_DIR" ]; then
    install -Dm644 "$ASSETS/udev/40-elgato-streamdeck.rules" \
        "$UDEV_DIR/40-elgato-streamdeck.rules"
    if command -v udevadm &>/dev/null; then
        udevadm control --reload 2>/dev/null || true
        udevadm trigger 2>/dev/null || true
    fi
fi

# Refresh icon cache if available
if command -v gtk-update-icon-cache &>/dev/null; then
    gtk-update-icon-cache -f /usr/share/icons/hicolor/ 2>/dev/null || true
fi

# Refresh desktop database if available
if command -v update-desktop-database &>/dev/null; then
    update-desktop-database "$DESKTOP_DIR" 2>/dev/null || true
fi

# Refresh MIME database if available
if command -v update-mime-database &>/dev/null; then
    update-mime-database /usr/share/mime 2>/dev/null || true
fi

echo "Installed successfully. You may need to log out and back in for the icon to appear."
