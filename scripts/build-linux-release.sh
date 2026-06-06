#!/usr/bin/env bash
#
# Build + package a distributable Linux release of S21 HiJack.
#
# Produces, under dist/:
#   s21_hijack-v<version>-linux-<arch>.tar.gz          binary + assets + installer
#   s21_hijack-v<version>-linux-<arch>.tar.gz.sha256   checksum
#
# The tarball bundles the (stripped) release binary, install-linux.sh /
# uninstall-linux.sh, and the desktop / MIME / icon / udev assets — everything
# an operator needs to run or system-install the app with no Rust toolchain.
# The version in the artifact name comes from Cargo.toml, so it matches the
# `tag_name` the in-app update check compares against on GitHub.
#
# Usage:
#   scripts/build-linux-release.sh           # native arch, full release build
#   SKIP_BUILD=1 scripts/build-linux-release.sh   # repackage an existing
#                                                 # target/release/s21_hijack
#
# Build requirements: a Rust toolchain plus libudev-dev + pkg-config (the
# hidapi Stream Deck driver links against libudev). Packaging uses tar,
# sha256sum, install and (optionally) strip.

set -euo pipefail

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_DIR"

# ── Resolve version (Cargo.toml) and target arch ──────────────────────────
# `|| true` keeps `set -e`/pipefail from aborting on the assignment if grep
# finds no match, so the explicit empty-version diagnostic below is reachable.
VERSION="$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"(.*)".*/\1/')" || true
if [ -z "$VERSION" ]; then
    echo "error: could not read version from Cargo.toml" >&2
    exit 1
fi
ARCH="$(uname -m)"
PKG="s21_hijack-v${VERSION}-linux-${ARCH}"
BIN="target/release/s21_hijack"

echo "==> Building S21 HiJack v${VERSION} for linux/${ARCH}"

# ── Build (unless reusing an existing binary) ─────────────────────────────
if [ "${SKIP_BUILD:-0}" != "1" ]; then
    if ! command -v cargo >/dev/null 2>&1; then
        echo "error: cargo not found — install the Rust toolchain (https://rustup.rs)" >&2
        exit 1
    fi
    cargo build --release
fi

if [ ! -f "$BIN" ]; then
    echo "error: release binary not found at $BIN" >&2
    echo "       run without SKIP_BUILD=1, or 'cargo build --release' first." >&2
    exit 1
fi

# ── Stage the package tree ────────────────────────────────────────────────
STAGE="dist/${PKG}"
echo "==> Staging $STAGE"
rm -rf "$STAGE"
mkdir -p "$STAGE/assets/udev"

install -Dm755 "$BIN" "$STAGE/s21_hijack"
# Strip the staged copy for a smaller download; target/release stays intact.
if command -v strip >/dev/null 2>&1; then
    strip "$STAGE/s21_hijack" || true
fi

install -Dm755 install-linux.sh                       "$STAGE/install-linux.sh"
install -Dm755 uninstall-linux.sh                     "$STAGE/uninstall-linux.sh"
install -Dm644 assets/icon.png                        "$STAGE/assets/icon.png"
install -Dm644 assets/s21_hijack.desktop              "$STAGE/assets/s21_hijack.desktop"
install -Dm644 assets/s21_hijack-mime.xml             "$STAGE/assets/s21_hijack-mime.xml"
install -Dm644 assets/udev/40-elgato-streamdeck.rules "$STAGE/assets/udev/40-elgato-streamdeck.rules"
install -Dm644 README.md                              "$STAGE/README.md"
install -Dm644 LICENSE-MIT                            "$STAGE/LICENSE-MIT"
install -Dm644 LICENSE-APACHE                         "$STAGE/LICENSE-APACHE"

# ── Tarball + checksum ────────────────────────────────────────────────────
TARBALL="dist/${PKG}.tar.gz"
echo "==> Creating $TARBALL"
tar -C dist -czf "$TARBALL" "$PKG"
( cd dist && sha256sum "${PKG}.tar.gz" > "${PKG}.tar.gz.sha256" )

SIZE="$(du -h "$TARBALL" | cut -f1)"
echo ""
echo "Done:"
echo "  $TARBALL  ($SIZE)"
echo "  ${TARBALL}.sha256"
echo ""
echo "Install on a target machine:"
echo "  tar xzf ${PKG}.tar.gz && cd ${PKG} && sudo ./install-linux.sh"
