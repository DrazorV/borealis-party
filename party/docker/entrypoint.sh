#!/bin/sh
# Runs the server as PUID:PGID, as linuxserver.io images do, so files in /config belong to your user on the host.
set -eu

PUID="${PUID:-1000}"
PGID="${PGID:-1000}"
CONFIG="${BOREALIS_CONFIG_DIR:-/config}"

if [ -n "${TZ:-}" ] && [ -e "/usr/share/zoneinfo/$TZ" ]; then
    ln -sf "/usr/share/zoneinfo/$TZ" /etc/localtime
    echo "$TZ" > /etc/timezone
fi

if [ "$(id -u)" != "0" ]; then
    # Started with `user:` already set: nothing to switch.
    exec /app/borealis-server "$@"
fi

mkdir -p "$CONFIG"
# Only files that aren't PUID's yet, so a large /config isn't walked on every start.
find "$CONFIG" \( ! -uid "$PUID" -o ! -gid "$PGID" \) -exec chown -h "$PUID:$PGID" {} + 2>/dev/null || true

exec setpriv --reuid="$PUID" --regid="$PGID" --clear-groups /app/borealis-server "$@"
