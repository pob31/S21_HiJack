#!/usr/bin/env bash
set -euo pipefail

# Remove a system-wide S21 HiJack install (the inverse of install-linux.sh).
# Use the same PREFIX you installed with, e.g. `sudo PREFIX=/usr ./uninstall-linux.sh`.

PREFIX="${PREFIX:-/usr/local}"
BIN_DIR="$PREFIX/bin"
ICON_DIR="/usr/share/icons/hicolor/256x256/apps"
DESKTOP_DIR="/usr/share/applications"
MIME_DIR="/usr/share/mime/packages"
UDEV_DIR="/etc/udev/rules.d"

echo "Removing S21 HiJack..."

rm -f "$BIN_DIR/s21_hijack"
rm -f "$ICON_DIR/s21_hijack.png"
rm -f "$DESKTOP_DIR/s21_hijack.desktop"
rm -f "$MIME_DIR/s21_hijack.xml"
rm -f "$UDEV_DIR/40-elgato-streamdeck.rules"

# Refresh the desktop / icon / MIME caches so the entry disappears cleanly.
if command -v gtk-update-icon-cache &>/dev/null; then
    gtk-update-icon-cache -f /usr/share/icons/hicolor/ 2>/dev/null || true
fi
if command -v update-desktop-database &>/dev/null; then
    update-desktop-database "$DESKTOP_DIR" 2>/dev/null || true
fi
if command -v update-mime-database &>/dev/null; then
    update-mime-database /usr/share/mime 2>/dev/null || true
fi
if command -v udevadm &>/dev/null; then
    udevadm control --reload 2>/dev/null || true
fi

echo "Removed. Per-user preferences and show files were left untouched."
