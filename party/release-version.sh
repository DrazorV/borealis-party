#!/usr/bin/env sh
# Prints the Borealis Connect release version and its kind, from party/VERSION, after checking it against
# party/Cargo.toml. Used by party/release.sh, .github/workflows/connect-apps.yml and connect-image.yml.
#   X.Y.Z    a full release: the apps (Windows, Linux, Android, iOS), the desktop update feed and the server image.
#            It must be party/Cargo.toml's version.
#   X.Y.Z.W  a server and web release: the server image and release notes only. X.Y.Z is party/Cargo.toml's version
#            (the apps' version, which stays 3-part semver); W counts the server releases since that full release.
# Usage: sh party/release-version.sh           prints the version, e.g. 0.3.3.1
#        sh party/release-version.sh --kind    prints full or server
#        sh party/release-version.sh --apps    prints the apps' version (party/Cargo.toml), e.g. 0.3.3
set -eu
cd "$(dirname "$0")"

fail() { echo "party/release-version.sh: $1" >&2; exit 1; }

apps=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1 | tr -d '\r')
[ -n "$apps" ] || fail "no version line in party/Cargo.toml"
[ -f VERSION ] || fail "party/VERSION is missing"
release=$(head -n 1 VERSION | tr -d '\r' | tr -d ' ')

case "$release" in
    "$apps") kind=full ;;
    "$apps".*)
        build=${release#"$apps".}
        case "$build" in
            "" | *[!0-9]* | 0*) fail "party/VERSION is $release: the 4th number must be 1 or more, without leading zeros" ;;
        esac
        kind=server ;;
    *) fail "party/VERSION is $release, but party/Cargo.toml is $apps: VERSION must be $apps or $apps.N" ;;
esac

case "${1:-}" in
    "") echo "$release" ;;
    --kind) echo "$kind" ;;
    --apps) echo "$apps" ;;
    *) fail "usage: sh party/release-version.sh [--kind | --apps]" ;;
esac
