#!/usr/bin/env bash
# Tags a Borealis Party release on main: party-v<version from party/Cargo.toml>, with the Borealis release it is built on
# in the tag message. Checks first; pushes nothing.
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

version=$(sed -n 's/^version = "\(.*\)"/\1/p' party/Cargo.toml | head -n 1 | tr -d '\r')
[ -n "$version" ] || fail "no version in party/Cargo.toml"
tag="party-v$version"
if git rev-parse -q --verify "refs/tags/$tag" >/dev/null || git ls-remote --exit-code --tags origin "refs/tags/$tag" >/dev/null; then
    fail "$tag exists already; raise the version in party/Cargo.toml (and build once so Cargo.lock follows)"
fi

based="Based on Borealis ${base#v}."
if [ "$ahead" -gt 0 ]; then
    based="Based on Borealis ${base#v}, plus $ahead newer Borealis commits up to $(git merge-base HEAD upstream/main | cut -c1-7)."
fi
git tag -a "$tag" -m "Borealis Party $version" -m "$based"
echo "tagged $tag: $based"
echo "push it with: git push origin $tag"
