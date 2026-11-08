#!/bin/sh
# Fails if the fork has changed any upstream file beyond the agreed hook. Our code belongs in party/ and ui/party/.
# Usage: git fetch upstream && sh party/check-upstream.sh
set -eu

allowed='src-tauri/src/state.rs'
base=$(git merge-base upstream/main HEAD)
changed=$(git diff --name-only "$base" -- . ':(exclude)party' ':(exclude)ui/party')

status=0
for file in $changed; do
    if [ "$file" != "$allowed" ]; then
        echo "upstream file changed: $file"
        status=1
    fi
done

lines=$(git diff --numstat "$base" -- "$allowed" | awk '{print $1 + $2}')
if [ "${lines:-0}" -gt 3 ]; then
    echo "$allowed: expected only the 3-line party hook, found $lines changed lines"
    status=1
fi

[ "$status" -eq 0 ] && echo "ok: upstream files are untouched apart from the hook in $allowed"
exit "$status"
