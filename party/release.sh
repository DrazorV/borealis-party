#!/usr/bin/env bash
# Tags a Borealis Connect release on main: party-v<party/VERSION>, with the Borealis release it is built on in the tag
# message. Checks first; pushes nothing. party/VERSION (party/release-version.sh) says which kind:
#   X.Y.Z    a full release, the apps too; it must be party/Cargo.toml's version.
#   X.Y.Z.W  a server and web release: the server image and release notes, no apps (party-apps.yml skips them).
# Usage: bash party/release.sh [--unreleased]
#   --unreleased  allow Borealis commits that are newer than its last release (say why in the release notes)
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

unreleased=0
case "${1:-}" in
    "") ;;
    --unreleased) unreleased=1 ;;
    *) echo "usage: bash party/release.sh [--unreleased]"; exit 2 ;;
esac

fail() { echo "$1"; exit 1; }

[ -z "$(git status --porcelain)" ] || fail "the working tree has changes; commit them first"
[ "$(git rev-parse --abbrev-ref HEAD)" = main ] || fail "release from main"
git fetch origin --quiet
git fetch upstream --tags --quiet
[ "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)" ] ||
    fail "main and origin/main differ; push (or pull) first, so the image and the tag are the same commit"

# The newest Borealis release in HEAD, and the newest there is.
releases=$(git tag -l 'v[0-9]*' --sort=-v:refname | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' || true)
base=""
for tag in $releases; do
    if git merge-base --is-ancestor "$tag" HEAD; then base=$tag; break; fi
done
[ -n "$base" ] || fail "no Borealis release is merged; run bash party/sync-upstream.sh"
newest=$(echo "$releases" | head -n 1)
if [ "$newest" != "$base" ]; then
    echo "warning: built on Borealis $base, but $newest is out (bash party/sync-upstream.sh takes it)"
fi

# Borealis commits after that release, i.e. upstream work that is in HEAD but not in $base.
ahead=$(comm -12 <(git rev-list "$base..HEAD" | sort) <(git rev-list "$base..upstream/main" | sort) | wc -l | tr -d ' ')
if [ "$ahead" -gt 0 ] && [ "$unreleased" -eq 0 ]; then
    fail "main has $ahead Borealis commits newer than $base; wait for their next release, or pass --unreleased"
fi

sh party/check-upstream.sh >/dev/null || fail "party/check-upstream.sh fails; run it to see why"
bash party/check-commands.sh >/dev/null || fail "party/check-commands.sh fails; run it to see why"

version=$(sh party/release-version.sh) || fail "party/VERSION does not fit party/Cargo.toml (see above)"
kind=$(sh party/release-version.sh --kind)
tag="party-v$version"
if git rev-parse -q --verify "refs/tags/$tag" >/dev/null || git ls-remote --exit-code --tags origin "refs/tags/$tag" >/dev/null; then
    if [ "$kind" = full ]; then
        fail "$tag exists already; for a server and web release set party/VERSION to $version.1, or for a full release raise the version in party/Cargo.toml and party/VERSION (and build once so Cargo.lock follows)"
    fi
    fail "$tag exists already; raise the 4th number in party/VERSION"
fi

based="Based on Borealis ${base#v}."
if [ "$ahead" -gt 0 ]; then
    based="Based on Borealis ${base#v}, plus $ahead newer Borealis commits up to $(git merge-base HEAD upstream/main | cut -c1-7)."
fi
title="Borealis Connect $version"
[ "$kind" = full ] || title="$title (server and web only)"
git tag -a "$tag" -m "$title" -m "$based"
echo "tagged $tag ($kind release): $based"
echo "push it with: git push origin $tag"
