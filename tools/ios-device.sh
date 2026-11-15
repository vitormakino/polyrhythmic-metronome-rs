#!/usr/bin/env bash
# Builds, signs and installs the app on an iPhone paired with this Mac, over
# the cable or the network, then opens it. Needs the one-time setup in the
# README (Apple Account in Xcode, ios/Signing.xcconfig, Developer Mode, the
# phone paired once over USB with "Connect via network" on).
#
#   tools/ios-device.sh                    # a paired iPhone in reach
#   tools/ios-device.sh "My iPhone"        # by name or identifier
#   tools/ios-device.sh --if-due           # only if the last install is 6+ days old
#   tools/ios-device.sh --install-schedule # check once a day (launchd agent)
#   tools/ios-device.sh --remove-schedule  # stop the daily check
#
# A free account's install expires after 7 days; --if-due reinstalls a day
# before that and shows a notification when the phone could not be reached.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

out="target/ios-device"
derived="$out/DerivedData"
stamp="$out/last-install"
configuration="Release"
renew_after_days=6
label="io.github.vitormakino.polyrhythmic-metronome.ios-reinstall"
agent="$HOME/Library/LaunchAgents/$label.plist"
mkdir -p "$out"

notify() {
  osascript -e "display notification \"$1\" with title \"Polyrhythmic Metronome\"" >/dev/null 2>&1 || true
}

install_schedule() {
  cat >"$agent" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>$label</string>
  <key>ProgramArguments</key>
  <array>
    <string>/bin/bash</string>
    <string>$root/tools/ios-device.sh</string>
    <string>--if-due</string>
  </array>
  <key>StartCalendarInterval</key>
  <dict><key>Hour</key><integer>12</integer><key>Minute</key><integer>0</integer></dict>
  <key>EnvironmentVariables</key>
  <dict><key>PATH</key><string>$HOME/.cargo/bin:/usr/local/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin</string></dict>
  <key>StandardOutPath</key><string>$root/$out/schedule.log</string>
  <key>StandardErrorPath</key><string>$root/$out/schedule.log</string>
</dict>
</plist>
EOF
  launchctl bootout "gui/$(id -u)/$label" >/dev/null 2>&1 || true
  launchctl bootstrap "gui/$(id -u)" "$agent"
  echo "scheduled: every day at 12:00 (or at the next wake), reinstalling when due"
  echo "log: $root/$out/schedule.log"
}

remove_schedule() {
  launchctl bootout "gui/$(id -u)/$label" >/dev/null 2>&1 || true
  rm -f "$agent"
  echo "schedule removed"
}

if_due=false
want=""
case "${1:-}" in
  --install-schedule) install_schedule; exit 0 ;;
  --remove-schedule) remove_schedule; exit 0 ;;
  --if-due) if_due=true ;;
  "") ;;
  *) want="$1" ;;
esac

if $if_due && [[ -f "$stamp" ]]; then
  age_days=$(( ($(date +%s) - $(cat "$stamp")) / 86400 ))
  if (( age_days < renew_after_days )); then
    echo "$(date '+%F %T') installed $age_days day(s) ago, nothing to do"
    exit 0
  fi
fi
echo "$(date '+%F %T') installing"

# On failure in the scheduled run, tell the user what to do.
fail() {
  echo "error: $1" >&2
  if $if_due; then
    notify "Could not reinstall on the iPhone: $1. Unlock it on the same Wi-Fi, or connect the cable, then run tools/ios-device.sh."
  fi
  exit 1
}

[[ -f ios/Signing.xcconfig ]] \
  || fail "ios/Signing.xcconfig is missing (copy ios/Signing.xcconfig.example and set your Team ID)"

# Pick the iPhone: the argument (name or identifier), else a paired one the Mac
# can reach now (cable or network), else the first paired one.
devices_json="$out/devices.json"
xcrun devicectl list devices --json-output "$devices_json" >/dev/null 2>&1 \
  || fail "could not list devices"
read -r device_id hw_udid < <(python3 -I -c '
import json, sys
want = sys.argv[2]
found = []
for d in json.load(open(sys.argv[1]))["result"]["devices"]:
    p = d.get("properties", {})
    hw = p.get("hardware", {})
    name = p.get("state", {}).get("name", "")
    # Real, paired iPhones only (simulators are listed too).
    if (hw.get("platform") != "iOS" or hw.get("reality") != "physical"
            or p.get("connection", {}).get("pairingState") != "paired"):
        continue
    if not want or want in (name, d["identifier"], hw.get("udid")):
        reachable = p["connection"].get("state") == "connected"
        found.append((not reachable, d["identifier"], hw.get("udid", "")))
if not found:
    sys.exit(1)
_, identifier, udid = min(found, key=lambda f: f[0])
print(identifier, udid)
' "$devices_json" "$want") || fail "no paired iPhone${want:+ named $want}"
echo "device: $device_id (pass a name to pick another: tools/ios-device.sh \"My iPhone\")"

xcodebuild \
  -project ios/PolyrhythmicMetronome.xcodeproj \
  -scheme PolyrhythmicMetronome \
  -configuration "$configuration" \
  -destination "id=$hw_udid" \
  -derivedDataPath "$derived" \
  -allowProvisioningUpdates \
  -quiet \
  build || fail "the build or signing failed (open Xcode once if your account needs attention)"

app="$derived/Build/Products/$configuration-iphoneos/PolyrhythmicMetronome.app"
bundle_id="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$app/Info.plist")"

xcrun devicectl device install app --device "$device_id" "$app" \
  || fail "the iPhone did not accept the install"
date +%s >"$stamp"
echo "installed: $bundle_id"

# Opening needs the phone unlocked; the install already counts.
xcrun devicectl device process launch --device "$device_id" "$bundle_id" >/dev/null 2>&1 \
  || echo "note: installed, but could not open the app (is the iPhone locked?)"
