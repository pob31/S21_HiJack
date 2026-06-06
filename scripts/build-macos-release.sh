#!/usr/bin/env bash
#
# Build, sign, notarize, and package a distributable macOS release of S21 HiJack.
#
# Produces, under dist/:
#   S21 HiJack.app                                  signed + stapled app bundle
#   s21_hijack-v<version>-macos-universal.dmg       signed + notarized + stapled
#   s21_hijack-v<version>-macos-universal.dmg.sha256 checksum
#
# The binary is a universal (arm64 + x86_64) build so it runs natively on both
# Apple Silicon and Intel Macs. Fonts and the web UI are embedded in the binary
# at compile time; only the on-disk `locales/` tree is staged (the app scans for
# `locales/` beside its executable — see src/ui/help.rs::locale_dirs).
#
# The version in the artifact name comes from Cargo.toml, so it matches the
# `tag_name` the in-app update check compares against on GitHub.
#
# Usage:
#   scripts/build-macos-release.sh                 # full build + sign + notarize
#   SKIP_BUILD=1   scripts/build-macos-release.sh  # reuse existing target/*/release
#   SKIP_NOTARIZE=1 scripts/build-macos-release.sh # build + sign + dmg, no notarize
#
# Environment:
#   DEVELOPER_ID     codesign identity. Default: the sole "Developer ID
#                    Application" identity in the login keychain.
#   NOTARY_PROFILE   notarytool keychain profile name (default: s21-notary).
#                    Create once with:
#                      xcrun notarytool store-credentials s21-notary \
#                        --apple-id <you@example.com> \
#                        --team-id <TEAMID> \
#                        --password <app-specific-password>
#
# Requirements: Rust toolchain with aarch64- and x86_64-apple-darwin targets,
# plus the Xcode command line tools (codesign, notarytool, stapler, hdiutil,
# sips, iconutil, lipo).

set -euo pipefail

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_DIR"

APP_NAME="S21 HiJack"
BIN_NAME="s21_hijack"
BUNDLE_ID="com.pob31.s21hijack"
NOTARY_PROFILE="${NOTARY_PROFILE:-s21-notary}"

# ── Resolve version (Cargo.toml) ──────────────────────────────────────────
VERSION="$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"(.*)".*/\1/')" || true
if [ -z "$VERSION" ]; then
    echo "error: could not read version from Cargo.toml" >&2
    exit 1
fi
PKG="${BIN_NAME}-v${VERSION}-macos-universal"

echo "==> Building $APP_NAME v${VERSION} (universal: arm64 + x86_64)"

# ── Resolve signing identity ──────────────────────────────────────────────
if [ -z "${DEVELOPER_ID:-}" ]; then
    DEVELOPER_ID="$(security find-identity -v -p codesigning \
        | sed -nE 's/.*"(Developer ID Application: .*)"/\1/p' | head -n1)"
fi
if [ -z "$DEVELOPER_ID" ]; then
    echo "error: no 'Developer ID Application' identity found in the keychain." >&2
    echo "       set DEVELOPER_ID, or import your Developer ID certificate." >&2
    exit 1
fi
echo "    signing identity: $DEVELOPER_ID"

# ── Build both architectures ──────────────────────────────────────────────
if [ "${SKIP_BUILD:-0}" != "1" ]; then
    if ! command -v cargo >/dev/null 2>&1; then
        echo "error: cargo not found — install the Rust toolchain (https://rustup.rs)" >&2
        exit 1
    fi
    cargo build --release --target aarch64-apple-darwin
    cargo build --release --target x86_64-apple-darwin
fi

ARM_BIN="target/aarch64-apple-darwin/release/${BIN_NAME}"
X86_BIN="target/x86_64-apple-darwin/release/${BIN_NAME}"
for b in "$ARM_BIN" "$X86_BIN"; do
    if [ ! -f "$b" ]; then
        echo "error: release binary not found at $b" >&2
        echo "       run without SKIP_BUILD=1, or build both targets first." >&2
        exit 1
    fi
done

# ── Assemble the .app bundle ──────────────────────────────────────────────
APP_DIR="dist/${APP_NAME}.app"
echo "==> Assembling $APP_DIR"
rm -rf "$APP_DIR"
mkdir -p "$APP_DIR/Contents/MacOS" "$APP_DIR/Contents/Resources"

# Universal binary.
lipo -create -output "$APP_DIR/Contents/MacOS/${BIN_NAME}" "$ARM_BIN" "$X86_BIN"
strip "$APP_DIR/Contents/MacOS/${BIN_NAME}"

# Info.plist with the version substituted in.
sed "s/@VERSION@/${VERSION}/g" packaging/macos/Info.plist.in \
    > "$APP_DIR/Contents/Info.plist"

# Locale JSONs must live in Resources — codesign treats anything under
# Contents/MacOS as code and rejects unsigned data files there. The app scans
# for `locales/` beside its executable (src/ui/help.rs::locale_dirs), so
# symlink that path to the Resources copy; the symlink resolves at runtime and
# is sealed cleanly by the signature.
cp -R assets/locales "$APP_DIR/Contents/Resources/locales"
ln -s ../Resources/locales "$APP_DIR/Contents/MacOS/locales"

# Icon: build a .icns from the 768px source at package time (no binary in git).
# 768px cleanly covers every slot up to 512@1x / 256@2x; the 512@2x (1024px)
# slot is a mild 1.33x upscale, still sharper than omitting the largest size.
ICON_SRC="assets/icon.jpeg"
ICONSET="$(mktemp -d)/${BIN_NAME}.iconset"
mkdir -p "$ICONSET"
for sz in 16 32 128 256 512; do
    # -s format png forces real PNG output; without it sips keeps the source's
    # JPEG encoding under a .png name and iconutil rejects the iconset.
    sips -s format png -z $sz  $sz  "$ICON_SRC" --out "$ICONSET/icon_${sz}x${sz}.png"    >/dev/null
    dbl=$((sz * 2))
    sips -s format png -z $dbl $dbl "$ICON_SRC" --out "$ICONSET/icon_${sz}x${sz}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP_DIR/Contents/Resources/${BIN_NAME}.icns"
rm -rf "$(dirname "$ICONSET")"

# ── Code sign (hardened runtime, secure timestamp) ────────────────────────
echo "==> Signing app bundle"
codesign --force --options runtime --timestamp \
    --entitlements packaging/macos/entitlements.plist \
    --sign "$DEVELOPER_ID" "$APP_DIR"
codesign --verify --deep --strict --verbose=2 "$APP_DIR"

# ── Build the DMG ─────────────────────────────────────────────────────────
DMG="dist/${PKG}.dmg"
echo "==> Creating $DMG"
rm -f "$DMG"
DMG_STAGE="$(mktemp -d)/dmg"
mkdir -p "$DMG_STAGE"
cp -R "$APP_DIR" "$DMG_STAGE/"
ln -s /Applications "$DMG_STAGE/Applications"
hdiutil create -volname "$APP_NAME" -srcfolder "$DMG_STAGE" \
    -ov -format UDZO "$DMG" >/dev/null
rm -rf "$(dirname "$DMG_STAGE")"

echo "==> Signing DMG"
codesign --force --timestamp --sign "$DEVELOPER_ID" "$DMG"

# ── Notarize + staple ─────────────────────────────────────────────────────
if [ "${SKIP_NOTARIZE:-0}" != "1" ]; then
    echo "==> Submitting to Apple notary service (profile: $NOTARY_PROFILE)"
    xcrun notarytool submit "$DMG" \
        --keychain-profile "$NOTARY_PROFILE" --wait

    echo "==> Stapling notarization ticket"
    xcrun stapler staple "$DMG"
    # The app's cdhash is part of the notarized DMG submission, so its ticket
    # is now available; staple the loose .app too for fully offline launch.
    xcrun stapler staple "$APP_DIR" || true

    echo "==> Gatekeeper assessment"
    spctl -a -t open --context context:primary-signature -vvv "$DMG" || true
    stapler validate "$DMG"
else
    echo "==> SKIP_NOTARIZE=1 — DMG is signed but not notarized/stapled."
fi

# ── Checksum ──────────────────────────────────────────────────────────────
( cd dist && shasum -a 256 "${PKG}.dmg" > "${PKG}.dmg.sha256" )

SIZE="$(du -h "$DMG" | cut -f1)"
echo ""
echo "Done:"
echo "  $DMG  ($SIZE)"
echo "  ${DMG}.sha256"
echo "  $APP_DIR"
