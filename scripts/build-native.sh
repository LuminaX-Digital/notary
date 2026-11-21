#!/usr/bin/env bash
# Builds the native helper to target/release/notary-mac (arm64, macOS 14.4 and newer).
set -euo pipefail

cd "$(dirname "$0")/.."
mkdir -p target/release

swiftc -O -swift-version 5 \
  -target arm64-apple-macos14.4 \
  native/notary-mac/main.swift \
  -o target/release/notary-mac \
  -framework AppKit \
  -framework ApplicationServices \
  -framework AVFoundation \
  -framework CoreAudio \
  -framework CoreGraphics \
  -framework EventKit \
  -framework Foundation

echo "Built target/release/notary-mac"
