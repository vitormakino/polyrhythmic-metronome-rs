#!/usr/bin/env bash
# Builds a local, ad-hoc signed PolyrhythmicMetronome.app from the release
# binary. Needs only Xcode Command Line Tools and Rust; installs nothing.
#
#   scripts/bundle-macos.sh        -> target/bundle/PolyrhythmicMetronome.app
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

bin_name="polyrhythmic-metronome"
app_name="PolyrhythmicMetronome"
bundle_id="io.github.vitormakino.polyrhythmic-metronome"
version="$(cargo metadata --no-deps --format-version 1 \
  | python3 -c 'import json,sys; m=json.load(sys.stdin); print(next(p["version"] for p in m["packages"] if p["name"]=="polyrhythmic-metronome-app"))')"

cargo build --release -p polyrhythmic-metronome-app

app="target/bundle/${app_name}.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "target/release/${bin_name}" "$app/Contents/MacOS/${bin_name}"

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Polyrhythmic Metronome</string>
  <key>CFBundleDisplayName</key><string>Polyrhythmic Metronome</string>
  <key>CFBundleIdentifier</key><string>${bundle_id}</string>
  <key>CFBundleExecutable</key><string>${bin_name}</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${version}</string>
  <key>CFBundleVersion</key><string>${version}</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>LSUIElement</key><false/>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST

codesign -s - --force --deep "$app"
echo "Built $root/$app"
