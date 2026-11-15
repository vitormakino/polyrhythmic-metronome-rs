#!/usr/bin/env bash
# Smoke test on the iOS Simulator through the Xcode project: builds the app,
# installs and launches it, saves a screenshot to target/ios-sim/screen.png,
# and fails if the app is not running 5 s after launch. Shuts the simulator
# down at the end. Needs Xcode and the aarch64-apple-ios-sim Rust target.
#
#   tools/ios-sim.sh                 # first available iPhone simulator
#   tools/ios-sim.sh "iPhone 18 Pro" # by name or UDID
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

out="target/ios-sim"
derived="$out/DerivedData"
configuration="Debug"
mkdir -p "$out"

# Pick the simulator: the argument (name or UDID), else the first available
# iPhone.
udid="$(xcrun simctl list devices available --json | python3 -I -c '
import json, sys
want = sys.argv[1]
for runtime, devices in json.load(sys.stdin)["devices"].items():
    if "iOS" not in runtime:
        continue
    for d in devices:
        if want in (d["name"], d["udid"]) or (not want and d["name"].startswith("iPhone")):
            print(d["udid"])
            sys.exit(0)
sys.exit("no available iOS simulator matches " + repr(want or "iPhone"))
' "${1:-}")"
echo "simulator: $udid"

shutdown() { xcrun simctl shutdown "$udid" >/dev/null 2>&1 || true; }
trap shutdown EXIT

xcodebuild \
  -project ios/PolyrhythmicMetronome.xcodeproj \
  -scheme PolyrhythmicMetronome \
  -configuration "$configuration" \
  -sdk iphonesimulator \
  -destination "id=$udid" \
  -derivedDataPath "$derived" \
  -quiet \
  build

app="$derived/Build/Products/$configuration-iphonesimulator/PolyrhythmicMetronome.app"
bundle_id="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$app/Info.plist")"

xcrun simctl bootstatus "$udid" -b >/dev/null
xcrun simctl install "$udid" "$app"
xcrun simctl launch "$udid" "$bundle_id"
sleep 5

# A failed screenshot is not a failed app: warn and still check that it runs.
if xcrun simctl io "$udid" screenshot "$out/screen.png" >/dev/null 2>&1; then
  echo "screenshot: $out/screen.png"
else
  echo "warning: no screenshot taken" >&2
fi

# A running app has a UIKitApplication job with a pid in launchd.
if xcrun simctl spawn "$udid" launchctl list \
  | awk -v id="UIKitApplication:$bundle_id" 'index($3, id) == 1 && $1 ~ /^[0-9]+$/ { found = 1 } END { exit !found }'; then
  echo "ok: $bundle_id is running"
else
  echo "error: $bundle_id is not running 5 s after launch" >&2
  exit 1
fi
