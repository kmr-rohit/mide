#!/bin/sh
# Install mide on macOS with one command, no sudo, no Gatekeeper dialog:
#
#   curl -fsSL https://raw.githubusercontent.com/kmr-rohit/mide/main/scripts/install.sh | sh
#
# It downloads the latest release (checking its checksum), copies mide.app to /Applications
# (or ~/Applications if that is not writable) and links a `mide` command into ~/.local/bin or
# /usr/local/bin. Files fetched by curl carry no quarantine flag, so macOS does not block them.
#
#   sh install.sh --uninstall      remove what this script installed
#
# Environment (mostly for testing): MIDE_REPO, MIDE_VERSION, MIDE_APPS_DIR, MIDE_BIN_DIR,
# MIDE_DMG_FILE (use a local .dmg instead of downloading).
set -eu

REPO="${MIDE_REPO:-kmr-rohit/mide}"
say() { printf '%s\n' "$*"; }
die() { printf 'mide install: %s\n' "$*" >&2; exit 1; }

[ "$(uname -s)" = "Darwin" ] || die "mide is a macOS app; this is $(uname -s)."

# Where things go.
APPS="${MIDE_APPS_DIR:-/Applications}"
if [ -z "${MIDE_APPS_DIR:-}" ] && [ ! -w "$APPS" ]; then
    APPS="$HOME/Applications"
fi
if [ -n "${MIDE_BIN_DIR:-}" ]; then
    BIN="$MIDE_BIN_DIR"
elif [ -w /usr/local/bin ]; then
    BIN=/usr/local/bin
else
    BIN="$HOME/.local/bin"
fi

if [ "${1:-}" = "--uninstall" ]; then
    # Only touch the places this run would install into; with no overrides that is the default set.
    if [ -n "${MIDE_APPS_DIR:-}" ]; then APP_DIRS="$APPS"; else APP_DIRS="$APPS /Applications $HOME/Applications"; fi
    if [ -n "${MIDE_BIN_DIR:-}" ]; then BIN_DIRS="$BIN"; else BIN_DIRS="$BIN /usr/local/bin $HOME/.local/bin"; fi
    for d in $APP_DIRS; do
        if [ -d "$d/mide.app" ]; then
            rm -rf "$d/mide.app" 2>/dev/null && say "removed $d/mide.app" || say "could not remove $d/mide.app (installed with sudo? try: sudo rm -rf $d/mide.app)"
        fi
    done
    for d in $BIN_DIRS; do
        # Only links that point into a mide.app: never a file someone else put there.
        case "$(readlink "$d/mide" 2>/dev/null)" in
            */mide.app/Contents/Resources/mide-cli)
                rm -f "$d/mide" 2>/dev/null && say "removed $d/mide" || say "could not remove $d/mide (try: sudo rm $d/mide)" ;;
        esac
    done
    say "Settings stay in ~/.config/mide (delete it to remove them)."
    exit 0
fi

# verify FILE SHA256_URL: stop on a mismatch; carry on (with a warning) if the release has no checksum file.
verify() {
    if curl -fsSL -o "$TMP/sum" "$2"; then
        want=$(cut -d' ' -f1 "$TMP/sum")
        got=$(shasum -a 256 "$1" | cut -d' ' -f1)
        [ "$want" = "$got" ] || die "checksum mismatch (expected $want, got $got); not installing."
        say "Checksum OK."
    else
        say "Warning: no checksum file found for this release; continuing."
    fi
}

TMP=$(mktemp -d)
MOUNT=""
cleanup() {
    [ -n "$MOUNT" ] && hdiutil detach "$MOUNT" -quiet >/dev/null 2>&1 || true
    rm -rf "$TMP"
}
trap cleanup EXIT INT TERM

if [ -n "${MIDE_DMG_FILE:-}" ]; then
    DMG="$MIDE_DMG_FILE"
else
    VERSION="${MIDE_VERSION:-}"
    if [ -z "$VERSION" ]; then
        # /releases/latest redirects to /releases/tag/vX.Y.Z
        TAG=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$REPO/releases/latest" | sed 's#.*/tag/##')
        [ -n "$TAG" ] || die "could not find the latest release of $REPO."
        VERSION="${TAG#v}"
    fi
    BASE="https://github.com/$REPO/releases/download/v$VERSION"
    DMG="$TMP/mide.dmg"
    say "Downloading mide $VERSION ..."
    if curl -fsSL -o "$DMG" "$BASE/mide-$VERSION-universal.dmg" 2>/dev/null; then
        verify "$DMG" "$BASE/mide-$VERSION-universal.dmg.sha256"
    else
        # Releases before the disk image existed only ship the installer package (needs sudo).
        PKG="$TMP/mide.pkg"
        say "No disk image in this release; using the installer package (asks for your password) ..."
        curl -fsSL -o "$PKG" "$BASE/mide-$VERSION-universal.pkg" || die "could not download $BASE/mide-$VERSION-universal.pkg"
        verify "$PKG" "$BASE/mide-$VERSION-universal.pkg.sha256"
        sudo installer -pkg "$PKG" -target / || die "the installer failed."
        say ""
        say "Done. Open a project:  mide ~/code/my-project   (or open mide from Launchpad)"
        exit 0
    fi
fi

MOUNT=$(hdiutil attach -nobrowse -readonly -mountrandom "$TMP" "$DMG" | awk -F'\t' 'END {print $NF}')
[ -d "$MOUNT/mide.app" ] || die "the disk image does not contain mide.app."

mkdir -p "$APPS"
rm -rf "$APPS/mide.app"
ditto "$MOUNT/mide.app" "$APPS/mide.app"
# Belt and braces: a copy that came through a browser may carry the quarantine flag.
xattr -dr com.apple.quarantine "$APPS/mide.app" 2>/dev/null || true
say "Installed $APPS/mide.app"

CLI="$APPS/mide.app/Contents/Resources/mide-cli"
if [ -x "$CLI" ]; then
    mkdir -p "$BIN"
    ln -sf "$CLI" "$BIN/mide"
    say "Linked $BIN/mide"
    case ":$PATH:" in
        *":$BIN:"*) ;;
        *) say "Add $BIN to your PATH to use the \`mide\` command:  echo 'export PATH=\"$BIN:\$PATH\"' >> ~/.zshrc" ;;
    esac
fi

say ""
say "Done. Open a project:  mide ~/code/my-project   (or open mide from Launchpad)"
