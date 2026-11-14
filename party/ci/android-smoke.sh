#!/bin/sh
# Android smoke test (Connect apps workflow, android-smoke job): install the debug APK on the running emulator,
# launch it, and pass when its first screen (the server address or the sign-in form) shows and nothing crashed.
# Usage: party/ci/android-smoke.sh <apk> <out-dir>
set -eu
apk=$1
out=$2
pkg=app.borealis.party
here=$(dirname "$0")
mkdir -p "$out"

adb wait-for-device
adb install -r -g "$apk"
adb logcat -c || true
component=$(adb shell cmd package resolve-activity --brief "$pkg" | tail -n 1 | tr -d '\r')
echo "launching $component"
adb shell am start -W -n "$component"

# The debug build's web view can be inspected: its DevTools socket names the app's process.
pid=""
socket=""
i=0
while [ $i -lt 60 ]; do
  pid=$(adb shell pidof "$pkg" | tr -d '\r' || true)
  if [ -n "$pid" ]; then
    socket=$(adb shell cat /proc/net/unix | tr -d '\r' | grep -o "webview_devtools_remote_$pid" | head -n 1 || true)
    [ -n "$socket" ] && break
  fi
  i=$((i + 1))
  sleep 2
done

status=0
if [ -z "$pid" ]; then
  echo "::error::the app is not running after launch"
  status=1
elif [ -z "$socket" ]; then
  echo "::error::the app's web view never opened (no DevTools socket for pid $pid)"
  status=1
else
  adb forward tcp:9222 "localabstract:$socket"
  node "$here/android-smoke-page.mjs" 9222 'Borealis Connect|Server address|Sign in' || status=1
  adb forward --remove tcp:9222 || true
fi

# Still running, and no crash on record.
if [ -z "$(adb shell pidof "$pkg" | tr -d '\r' || true)" ]; then
  echo "::error::the app stopped after it launched"
  status=1
fi
adb logcat -b crash -d > "$out/crash.txt" || true
if grep -q "$pkg" "$out/crash.txt"; then
  echo "::error::the app crashed:"
  cat "$out/crash.txt"
  status=1
fi
adb exec-out screencap -p > "$out/screen.png" || true
adb logcat -d > "$out/logcat.txt" || true
exit $status
