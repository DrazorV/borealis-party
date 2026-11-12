#!/usr/bin/env bash
# Merges a Borealis release into the current branch, then runs the fork's checks.
# Party releases are built on Borealis releases (tags vX.Y.Z), not on upstream's main between releases.
# Usage: bash party/sync-upstream.sh [vX.Y.Z]   (default: the newest Borealis release)
# Run it from Git Bash on Windows; inside WSL the line endings make every file look changed.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

if [ -n "$(git status --porcelain)" ]; then
    echo "the working tree has changes; commit or stash them first"
    exit 1
fi

git fetch upstream --tags --quiet
tag=${1:-$(git tag -l 'v[0-9]*' --sort=-v:refname | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' | head -n 1)}
if [ -z "$tag" ] || ! git rev-parse -q --verify "refs/tags/$tag" >/dev/null; then
    echo "no Borealis release tag ${tag:-found}"
    exit 1
fi

if git merge-base --is-ancestor "$tag" HEAD; then
    echo "Borealis $tag is already merged"
else
    echo "merging Borealis $tag into $(git rev-parse --abbrev-ref HEAD)"
    if ! git merge --no-ff --no-edit -m "Merge Borealis $tag" "$tag"; then
        echo
        echo "the merge stopped on conflicts. Upstream's side wins in upstream files (party/upstream-pending.txt lists our"
        echo "carried PR patches). Resolve, commit, then run this script again to check."
        exit 1
    fi
fi

version=$(sed -n 's/^[[:space:]]*"version":[[:space:]]*"\([^"]*\)".*/\1/p' src-tauri/tauri.conf.json | head -n 1)
if [ "$version" != "${tag#v}" ]; then
    echo "note: src-tauri/tauri.conf.json says $version, not ${tag#v} (newer upstream work is merged too)"
fi

echo
status=0
sh party/check-upstream.sh || status=1
echo
bash party/check-commands.sh || status=1
echo
if [ "$status" -eq 0 ]; then
    echo "next: build and test (party/README.md, \"Releasing\"), then bash party/release.sh"
fi
exit "$status"
