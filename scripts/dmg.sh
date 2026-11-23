#!/bin/sh
# Build a drag-to-Applications disk image: dist/mide-<version>-<arch>.dmg
#
#   scripts/bundle.sh [--universal]     first (this packages target/bundle/mide.app)
#   scripts/dmg.sh
#
# With DEVELOPER_ID_APP set the image is signed; with NOTARY_PROFILE also set it is
# notarized and stapled (same variables as scripts/pkg.sh).
set -e
cd "$(dirname "$0")/.."

APP=target/bundle/mide.app
[ -d "$APP" ] || { echo "no $APP: run scripts/bundle.sh first" >&2; exit 1; }
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' crates/app/Cargo.toml | head -1)
ARCHS=$(lipo -archs "$APP/Contents/MacOS/mide")
case "$ARCHS" in *arm64*x86_64*|*x86_64*arm64*) ARCH=universal ;; *) ARCH=$ARCHS ;; esac

W=target/dmg
rm -rf "$W"; mkdir -p "$W/stage" dist
ditto "$APP" "$W/stage/mide.app"
ln -s /Applications "$W/stage/Applications"

OUT="dist/mide-$VERSION-$ARCH.dmg"
rm -f "$OUT"
hdiutil create -volname "mide $VERSION" -srcfolder "$W/stage" -fs HFS+ -format UDZO -ov "$OUT" >/dev/null

[ -n "$DEVELOPER_ID_APP" ] && codesign --sign "$DEVELOPER_ID_APP" --timestamp "$OUT"
if [ -n "$NOTARY_PROFILE" ] && [ -n "$DEVELOPER_ID_APP" ]; then
    echo "Notarizing (this takes a few minutes)..."
    xcrun notarytool submit "$OUT" --keychain-profile "$NOTARY_PROFILE" --wait
    xcrun stapler staple "$OUT"
fi

( cd dist && shasum -a 256 "$(basename "$OUT")" > "$(basename "$OUT").sha256" )
echo "Built $OUT ($(du -h "$OUT" | cut -f1))"
cat "$OUT.sha256"
[ -z "$DEVELOPER_ID_APP" ] && echo "Note: unsigned. Other Macs will show an 'unidentified developer' prompt."
exit 0
