#!/usr/bin/env bash
# Builds dist/Notary.app and dist/Notary.dmg.
#
#   scripts/bundle.sh
#       Ad-hoc signed. Runs on this Mac only.
#
#   SIGN_ID="Developer ID Application: Your Name (TEAMID)" scripts/bundle.sh
#       Signed for distribution (hardened runtime, secure timestamp).
#
#   SIGN_ID="..." NOTARY_PROFILE=<profile> scripts/bundle.sh
#       Also sends the DMG to Apple for notarization, staples the ticket and checks it with spctl.
#       Create the profile once with: xcrun notarytool store-credentials <profile>
set -euo pipefail

cd "$(dirname "$0")/.."
VERSION=$(grep -m1 '^version' Cargo.toml | sed 's/.*"\(.*\)".*/\1/')
BUILD=$(git rev-list --count HEAD 2>/dev/null || echo 1)
BUNDLE_ID="com.farazfarid.notary"
APP="dist/Notary.app"
DMG="dist/Notary.dmg"
SIGN_ID="${SIGN_ID:--}"
NOTARY_PROFILE="${NOTARY_PROFILE:-}"

if [[ -n "$NOTARY_PROFILE" && "$SIGN_ID" == "-" ]]; then
  echo "NOTARY_PROFILE needs SIGN_ID: Apple only notarizes Developer ID signed code." >&2
  exit 1
fi

TIMESTAMP=(--timestamp=none)
[[ "$SIGN_ID" != "-" ]] && TIMESTAMP=(--timestamp)

cargo build --release

rm -rf dist
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/notary "$APP/Contents/MacOS/Notary"
cp target/release/notary-llm "$APP/Contents/MacOS/notary-llm"
cp assets/AppIcon.icns "$APP/Contents/Resources/AppIcon.icns"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Notary</string>
  <key>CFBundleDisplayName</key><string>Notary</string>
  <key>CFBundleIdentifier</key><string>${BUNDLE_ID}</string>
  <key>CFBundleExecutable</key><string>Notary</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>CFBundleVersion</key><string>${BUILD}</string>
  <key>LSMinimumSystemVersion</key><string>14.4</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.productivity</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSHumanReadableCopyright</key><string>&#169; 2026 Fabian Faraz Farid</string>
  <key>NSMicrophoneUsageDescription</key>
  <string>Notary listens to your microphone so your own words appear in the transcript. Audio never leaves this Mac.</string>
</dict>
</plist>
PLIST

cat > dist/entitlements.plist <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>com.apple.security.device.audio-input</key><true/>
</dict>
</plist>
PLIST

# Sign from the inside out: the helper first, then the app (no --deep).
codesign --force --options runtime "${TIMESTAMP[@]}" --sign "$SIGN_ID" "$APP/Contents/MacOS/notary-llm"
codesign --force --options runtime "${TIMESTAMP[@]}" --entitlements dist/entitlements.plist --sign "$SIGN_ID" "$APP"
codesign --verify --strict --verbose=1 "$APP"

mkdir -p dist/dmg
cp -R "$APP" dist/dmg/
ln -s /Applications dist/dmg/Applications
hdiutil create -quiet -volname "Notary" -srcfolder dist/dmg -ov -format UDZO "$DMG"
rm -rf dist/dmg dist/entitlements.plist
[[ "$SIGN_ID" != "-" ]] && codesign --force "${TIMESTAMP[@]}" --sign "$SIGN_ID" "$DMG"

if [[ -n "$NOTARY_PROFILE" ]]; then
  echo "Sending $DMG to Apple for notarization. This usually takes a few minutes."
  OUT=$(xcrun notarytool submit "$DMG" --keychain-profile "$NOTARY_PROFILE" --wait 2>&1) || true
  echo "$OUT" | grep -E "id:|status:" | head -3
  if ! echo "$OUT" | grep -q "status: Accepted"; then
    ID=$(echo "$OUT" | awk '/id:/ {print $2; exit}')
    echo "Notarization was not accepted. Apple's log:" >&2
    [[ -n "$ID" ]] && xcrun notarytool log "$ID" --keychain-profile "$NOTARY_PROFILE" >&2
    exit 1
  fi
  xcrun stapler staple "$DMG"
  xcrun stapler validate "$DMG"
  spctl -a -t open --context context:primary-signature -vv "$DMG"
fi

echo "Built $APP and $DMG (version $VERSION, build $BUILD, signed with ${SIGN_ID/#-/ad-hoc}${NOTARY_PROFILE:+, notarized})"
