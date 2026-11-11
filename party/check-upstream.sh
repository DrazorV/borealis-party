#!/bin/sh
# Fails if the fork has changed any upstream file beyond the agreed hook. Our code belongs in party/ and ui/party/
# (and our own workflows, .github/workflows/party-*.yml).
# Files of an open upstream pull request listed in party/upstream-pending.txt are accepted too.
# Usage: git fetch upstream && sh party/check-upstream.sh
set -eu

allowed='src-tauri/src/state.rs'
pending="$(dirname "$0")/upstream-pending.txt"
base=$(git merge-base upstream/main HEAD)
changed=$(git diff --name-only "$base" -- . ':(exclude)party' ':(exclude)ui/party' ':(glob,exclude).github/workflows/party-*.yml')

# "<path> <PR url>" lines, comments and blank lines skipped
pending_pr() {
    [ -f "$pending" ] || return 1
    grep -v '^[[:space:]]*#' "$pending" | awk -v f="$1" '$1 == f { print $2; found = 1 } END { exit !found }'
}

status=0
for file in $changed; do
    if [ "$file" = "$allowed" ]; then
        continue
    fi
    if pr=$(pending_pr "$file"); then
        echo "pending upstream PR: $file ($pr)"
    else
        echo "upstream file changed: $file"
        status=1
    fi
done

if [ -f "$pending" ]; then
    for file in $(grep -v '^[[:space:]]*#' "$pending" | awk 'NF { print $1 }'); do
        if git diff --quiet "$base" -- "$file"; then
            echo "no longer differs from upstream, remove from upstream-pending.txt: $file"
        fi
    done
fi

lines=$(git diff --numstat "$base" -- "$allowed" | awk '{print $1 + $2}')
if [ "${lines:-0}" -gt 3 ]; then
    echo "$allowed: expected only the 3-line party hook, found $lines changed lines"
    status=1
fi

[ "$status" -eq 0 ] && echo "ok: upstream files are untouched apart from the hook in $allowed"
exit "$status"
