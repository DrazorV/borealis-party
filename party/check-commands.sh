#!/usr/bin/env bash
# Fails if an upstream command has no decision: every command in upstream's handler list (src-tauri/src/lib.rs) must be
# offered in party/crates/borealis-server/src/backend.rs or listed in party/commands-not-offered.txt.
# Usage: bash party/check-commands.sh
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# The command names inside the first `generate_handler![ ... ])` of a file, without their module path.
handler_names() {
    awk 'index($0, "generate_handler![") { f = 1; next } f && /^[[:space:]]*\]\)/ { exit } f' "$1" |
        tr -d '\r' |
        sed -n 's/^[[:space:]]*\([a-z_0-9:]*\),[[:space:]]*\(\/\/.*\)\{0,1\}$/\1/p' |
        sed 's/.*:://' | sort -u
}

handler_names src-tauri/src/lib.rs >"$tmp/upstream"
handler_names party/crates/borealis-server/src/backend.rs >"$tmp/offered"
tr -d '\r' <party/commands-not-offered.txt | sed 's/#.*//' | awk 'NF { print $1 }' | sort -u >"$tmp/not-offered"

[ -s "$tmp/upstream" ] || { echo "found no commands in src-tauri/src/lib.rs (has upstream's handler list moved?)"; exit 1; }
[ -s "$tmp/offered" ] || { echo "found no commands in backend.rs"; exit 1; }

status=0
for name in $(comm -23 "$tmp/upstream" "$tmp/offered" | comm -23 - "$tmp/not-offered"); do
    echo "new upstream command: $name (offer it in backend.rs or list it in commands-not-offered.txt)"
    status=1
done
for name in $(comm -12 "$tmp/offered" "$tmp/not-offered"); do
    echo "offered and listed as not offered: $name"
    status=1
done
for name in $(comm -23 "$tmp/not-offered" "$tmp/upstream"); do
    echo "no longer an upstream command, remove from commands-not-offered.txt: $name"
done

[ "$status" -eq 0 ] && echo "ok: every upstream command is offered or listed as not offered"
exit "$status"
