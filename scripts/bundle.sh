#!/usr/bin/env bash
# Builds dist/Notary.app and dist/Notary.dmg.
#
#   scripts/bundle.sh                      ad-hoc signed, runs on this Mac
#   SIGN_ID="Developer ID Application: Your Name (TEAMID)" scripts/bundle.sh
#                                          signed for distribution
#
# Notarizing a Developer ID build is a separate step:
#   xcrun notarytool submit dist/Notary.dmg --keychain-profile <profile> --wait
#   xcrun stapler staple dist/Notary.dmg
set -euo pipefail

cd "$(dirname "$0")/.."
VERSION=$(grep -m1 '^version' Cargo.toml | sed 's/.*"\(.*\)".*/\1/')
BUNDLE_ID="com.luminaxdigital.notary"
APP="dist/Notary.app"
SIGN_ID="${SIGN_ID:--}"

cargo build --release

rm -rf dist
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/notary "$APP/Contents/MacOS/Notary"
cp target/release/notary-llm "$APP/Contents/MacOS/notary-llm"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Notary</string>
  <key>CFBundleDisplayName</key><string>Notary</string>
  <key>CFBundleIdentifier</key><string>${BUNDLE_ID}</string>
  <key>CFBundleExecutable</key><string>Notary</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>CFBundleVersion</key><string>${VERSION}</string>
  <key>LSMinimumSystemVersion</key><string>14.4</string>
  <key>NSHighResolutionCapable</key><true/>
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

codesign --force --options runtime --sign "$SIGN_ID" "$APP/Contents/MacOS/notary-llm"
codesign --force --options runtime --entitlements dist/entitlements.plist --sign "$SIGN_ID" "$APP"
codesign --verify --strict --verbose=1 "$APP"

mkdir -p dist/dmg
cp -R "$APP" dist/dmg/
ln -s /Applications dist/dmg/Applications
hdiutil create -quiet -volname "Notary" -srcfolder dist/dmg -ov -format UDZO dist/Notary.dmg
rm -rf dist/dmg dist/entitlements.plist

echo "Built $APP and dist/Notary.dmg (version $VERSION, signed with ${SIGN_ID/#-/ad-hoc})"
