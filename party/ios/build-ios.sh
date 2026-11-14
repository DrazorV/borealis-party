#!/usr/bin/env bash
# Builds the party iOS app. macOS only (Xcode and the iOS SDK). Run from the repository root.
#   party/ios/build-ios.sh                    signed IPA for iPhones and iPads (arm64), default export method
#   party/ios/build-ios.sh --sim              debug app for the iOS Simulator on Apple silicon (no signing)
#   party/ios/build-ios.sh --debug            debug IPA for a device
#   party/ios/build-ios.sh --export-method app-store-connect --build-number 1004000
#
# Needs Xcode, Node 22 with the ui dependencies (npm --prefix ui ci) and the Rust targets aarch64-apple-ios and, for
# --sim, aarch64-apple-ios-sim. A device build is signed: set APPLE_DEVELOPMENT_TEAM (automatic signing with the Mac's
# own certificates), or IOS_CERTIFICATE, IOS_CERTIFICATE_PASSWORD and IOS_MOBILE_PROVISION as upstream's release does
# (Tauri imports them into a temporary keychain). The profile must be for the app id app.borealis.party.
#
# The result is copied to target/party-ios/: Borealis Party.ipa, or Borealis Party.app for --sim (named after the Tauri
# productName, which stays "Borealis Party"; the home screen shows CFBundleDisplayName, "Borealis Connect").
#
# party/tauri.party.ios.json gives the app its own id, app.borealis.party, and merges party/ios/Info.plist (name, plain
# HTTP, local network text) into upstream's Info.plist. Tauri writes those into upstream's Xcode project while it builds;
# this script puts upstream's tracked files back afterwards, so party/check-upstream.sh stays ok.
set -euo pipefail

target=aarch64
debug=false
export_method=""
build_number=""
while [ $# -gt 0 ]; do
    case "$1" in
        --sim) target=aarch64-sim; debug=true ;;
        --debug) debug=true ;;
        --export-method) export_method="$2"; shift ;;
        --build-number) build_number="$2"; shift ;;
        -h|--help) sed -n '2,18p' "$0"; exit 0 ;;
        *) echo "unknown option: $1" >&2; exit 2 ;;
    esac
    shift
done

[ "$(uname -s)" = "Darwin" ] || { echo "iOS apps build only on macOS (Xcode)." >&2; exit 1; }
[ -f party/tauri.party.ios.json ] || { echo "Run from the repository root." >&2; exit 1; }

# Upstream's project as checked out, to restore after the build. Left alone if someone has changed it on purpose.
gen=src-tauri/gen/apple
restore=false
if git diff --quiet -- "$gen"; then restore=true; fi
cleanup() { if $restore; then git checkout -- "$gen"; fi; }
trap cleanup EXIT

args=(ios build --target "$target" --ci --config party/tauri.party.json --config party/tauri.party.ios.json)
$debug && args+=(--debug)
[ -n "$export_method" ] && args+=(--export-method "$export_method")
[ -n "$build_number" ] && args+=(--config "{\"bundle\":{\"iOS\":{\"bundleVersion\":\"$build_number\"}}}")
# Without upstream's default features: device-link (device-to-device transfer, which a party app doesn't offer) and
# llama (on-device semantic search). The characters and rules come from the server. As party/android/build-apk.ps1.
args+=(-- --no-default-features)

started="$(mktemp)"
node ui/node_modules/@tauri-apps/cli/tauri.js "${args[@]}"

out=target/party-ios
rm -rf "$out"
mkdir -p "$out"
if [ "$target" = aarch64-sim ]; then
    app=$(find "$gen/build" -name '*.app' -type d -newer "$started" -prune 2>/dev/null | head -n 1)
    [ -n "$app" ] || { echo "No .app under $gen/build" >&2; exit 1; }
    cp -R "$app" "$out/Borealis Party.app"
    plist="$out/Borealis Party.app/Info.plist"
else
    ipa=$(find "$gen/build" -name '*.ipa' -type f -newer "$started" 2>/dev/null | head -n 1)
    [ -n "$ipa" ] || { echo "No .ipa under $gen/build" >&2; exit 1; }
    cp "$ipa" "$out/Borealis Party.ipa"
    unzip -q "$ipa" -d "$out/check"
    plist=$(find "$out/check/Payload" -maxdepth 2 -name Info.plist | head -n 1)
fi
rm -f "$started"

# What party/ios/Info.plist and tauri.party.ios.json must have changed; a Tauri that stops merging them fails here.
id=$(plutil -extract CFBundleIdentifier raw "$plist")
name=$(plutil -extract CFBundleDisplayName raw "$plist" 2>/dev/null || echo "")
http=$(plutil -extract NSAppTransportSecurity.NSAllowsArbitraryLoads raw "$plist" 2>/dev/null || echo "")
rm -rf "$out/check"
echo "Built: $id \"$name\" (plain HTTP allowed: ${http:-no})"
[ "$id" = app.borealis.party ] || { echo "Bundle id is $id, expected app.borealis.party" >&2; exit 1; }
[ "$name" = "Borealis Connect" ] || { echo "Display name is \"$name\", expected Borealis Connect" >&2; exit 1; }
[ "$http" = true ] || { echo "NSAllowsArbitraryLoads is not set: home-network servers would be refused" >&2; exit 1; }
ls -d "$out"/*
