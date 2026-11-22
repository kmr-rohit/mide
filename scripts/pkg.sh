#!/bin/sh
# Build the installer: dist/mide-<version>-<arch>.pkg
#
#   scripts/pkg.sh                 for this Mac's architecture (unsigned: fine for testing)
#   scripts/pkg.sh --universal     Apple Silicon + Intel
#
# To publish so that anyone can double-click it without a Gatekeeper warning, you need an
# Apple Developer ID (a paid developer account). Set, then run the same command:
#
#   DEVELOPER_ID_APP="Developer ID Application: Your Name (TEAMID)"
#   DEVELOPER_ID_INSTALLER="Developer ID Installer: Your Name (TEAMID)"
#   NOTARY_PROFILE=mide-notary    # created once with: xcrun notarytool store-credentials mide-notary
#
# The app and the package are then signed, the package is notarized and stapled.
set -e
cd "$(dirname "$0")/.."

UNIVERSAL=""
[ "$1" = "--universal" ] && UNIVERSAL="--universal"

./scripts/bundle.sh $UNIVERSAL
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' crates/app/Cargo.toml | head -1)
ARCH=$([ -n "$UNIVERSAL" ] && echo universal || uname -m)
HOSTS=$([ -n "$UNIVERSAL" ] && echo "arm64,x86_64" || echo "$(uname -m)")

W=target/pkg
rm -rf "$W"; mkdir -p "$W/root/Applications" "$W/scripts" "$W/out" dist
ditto target/bundle/mide.app "$W/root/Applications/mide.app"

# Installed after the app is in place: the `mide` command.
cat > "$W/scripts/postinstall" <<'POST'
#!/bin/sh
VOL="${3:-/}"
mkdir -p "$VOL/usr/local/bin" 2>/dev/null
ln -sf /Applications/mide.app/Contents/Resources/mide-cli "$VOL/usr/local/bin/mide" 2>/dev/null
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f /Applications/mide.app >/dev/null 2>&1
exit 0
POST
chmod +x "$W/scripts/postinstall"

# Always install into /Applications, even if another copy exists elsewhere.
pkgbuild --analyze --root "$W/root" "$W/component.plist" >/dev/null
/usr/libexec/PlistBuddy -c "Set :0:BundleIsRelocatable false" "$W/component.plist"

pkgbuild --root "$W/root" --component-plist "$W/component.plist" \
    --identifier dev.mide.editor --version "$VERSION" --install-location / \
    --scripts "$W/scripts" "$W/out/mide-component.pkg" >/dev/null

cat > "$W/distribution.xml" <<XML
<?xml version="1.0" encoding="utf-8"?>
<installer-gui-script minSpecVersion="2">
    <title>mide $VERSION</title>
    <welcome file="welcome.html"/>
    <license file="license.txt"/>
    <conclusion file="conclusion.html"/>
    <options customize="never" require-scripts="false" hostArchitectures="$HOSTS"/>
    <domains enable_localSystem="true"/>
    <volume-check><allowed-os-versions><os-version min="12.0"/></allowed-os-versions></volume-check>
    <pkg-ref id="dev.mide.editor"/>
    <choices-outline><line choice="default"><line choice="dev.mide.editor"/></line></choices-outline>
    <choice id="default"/>
    <choice id="dev.mide.editor" visible="false"><pkg-ref id="dev.mide.editor"/></choice>
    <pkg-ref id="dev.mide.editor" version="$VERSION" onConclusion="none">mide-component.pkg</pkg-ref>
</installer-gui-script>
XML

OUT="dist/mide-$VERSION-$ARCH.pkg"
SIGN=""
[ -n "$DEVELOPER_ID_INSTALLER" ] && SIGN="--sign $DEVELOPER_ID_INSTALLER --timestamp"
productbuild --distribution "$W/distribution.xml" --resources assets/macos/pkg-resources \
    --package-path "$W/out" $SIGN "$OUT" >/dev/null

if [ -n "$NOTARY_PROFILE" ] && [ -n "$DEVELOPER_ID_INSTALLER" ]; then
    echo "Notarizing (this takes a few minutes)..."
    xcrun notarytool submit "$OUT" --keychain-profile "$NOTARY_PROFILE" --wait
    xcrun stapler staple "$OUT"
fi

( cd dist && shasum -a 256 "$(basename "$OUT")" > "$(basename "$OUT").sha256" )
echo
echo "Built $OUT ($(du -h "$OUT" | cut -f1))"
cat "$OUT.sha256"
pkgutil --check-signature "$OUT" 2>&1 | sed -n '1,3p'
[ -z "$DEVELOPER_ID_INSTALLER" ] && echo "Note: unsigned. Other Macs will show an 'unidentified developer' prompt (see docs/install)."
exit 0
