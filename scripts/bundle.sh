#!/bin/sh
# Build a real macOS app: target/bundle/mide.app
#
#   scripts/bundle.sh                build for this Mac's architecture
#   scripts/bundle.sh --universal    one binary for Apple Silicon and Intel
#   scripts/bundle.sh --install      also copy it to /Applications and add a `mide` command
#
# Signing: ad-hoc by default (fine on your own Mac). For distribution set
# DEVELOPER_ID_APP="Developer ID Application: Your Name (TEAMID)" and it is signed with the
# hardened runtime and a secure timestamp, ready for notarization (see scripts/pkg.sh).
set -e
cd "$(dirname "$0")/.."
command -v cargo >/dev/null 2>&1 || . "$HOME/.cargo/env"

UNIVERSAL=0; INSTALL=0
for a in "$@"; do
    case "$a" in
        --universal) UNIVERSAL=1 ;;
        --install) INSTALL=1 ;;
        *) echo "usage: scripts/bundle.sh [--universal] [--install]" >&2; exit 2 ;;
    esac
done

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' crates/app/Cargo.toml | head -1)

if [ "$UNIVERSAL" = 1 ]; then
    rustup target add aarch64-apple-darwin x86_64-apple-darwin >/dev/null
    cargo build --release -p mide --target aarch64-apple-darwin
    cargo build --release -p mide --target x86_64-apple-darwin
    mkdir -p target/universal
    lipo -create -output target/universal/mide \
        target/aarch64-apple-darwin/release/mide target/x86_64-apple-darwin/release/mide
    BIN=target/universal/mide
else
    cargo build --release -p mide
    BIN=target/release/mide
fi

APP=target/bundle/mide.app
# If an old bundle can't be deleted (a running copy, or files someone else owns),
# move it aside instead of failing.
rm -rf "$APP" 2>/dev/null || true
if [ -e "$APP" ]; then
    mv "$APP" "$APP.stale.$$" || { echo "Cannot replace $APP; quit mide and remove it, then retry" >&2; exit 1; }
    echo "Note: moved an undeletable old bundle to $APP.stale.$$ (safe to delete later)"
fi
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/mide"
cp assets/macos/mide.icns "$APP/Contents/Resources/mide.icns"
cp assets/macos/mide-cli "$APP/Contents/Resources/mide-cli"
cp LICENSE "$APP/Contents/Resources/LICENSE"
[ -f THIRD_PARTY_LICENSES.md ] && cp THIRD_PARTY_LICENSES.md "$APP/Contents/Resources/"
sed "s/__VERSION__/$VERSION/g" assets/macos/Info.plist > "$APP/Contents/Info.plist"

if [ -n "$DEVELOPER_ID_APP" ]; then
    codesign --force --deep --options runtime --timestamp \
        --entitlements assets/macos/entitlements.plist --sign "$DEVELOPER_ID_APP" "$APP"
    echo "Signed with: $DEVELOPER_ID_APP"
else
    # Ad-hoc: a stable identity on this Mac, but other Macs will warn (see docs/install).
    codesign --force --deep --sign - "$APP"
fi
echo "Built $APP (version $VERSION, $(lipo -archs "$APP/Contents/MacOS/mide"))"

if [ "$INSTALL" = 1 ]; then
    rm -rf /Applications/mide.app
    cp -R "$APP" /Applications/mide.app
    /System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f /Applications/mide.app
    mkdir -p "$HOME/.local/bin"
    ln -sf /Applications/mide.app/Contents/Resources/mide-cli "$HOME/.local/bin/mide"
    echo "Installed /Applications/mide.app and ~/.local/bin/mide"
    case ":$PATH:" in *":$HOME/.local/bin:"*) ;; *) echo "Add ~/.local/bin to your PATH to use the 'mide' command." ;; esac
fi
