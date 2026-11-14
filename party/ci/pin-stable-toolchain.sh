#!/usr/bin/env bash
# CI only: make every cargo under this checkout use the stable toolchain, even where RUSTUP_TOOLCHAIN is not passed.
#
# Upstream pins its toolchain in rust-toolchain.toml. The party workflows choose stable through RUSTUP_TOOLCHAIN,
# but Tauri's mobile builds run cargo from Xcode's "Build Rust Code" phase (and Gradle's rust task) with only the
# TAURI*, WRY*, CARGO_*, RUST_*, PATH and TMPDIR variables (tauri-cli src/mobile/mod.rs, env_vars), so there the pin
# would win and rustup would download it (0.3.2's first iOS run failed on exactly that download).
# A rustup directory override ranks above rust-toolchain.toml and is stored in $RUSTUP_HOME/settings.toml, not in
# the checkout, so no upstream file changes. The check runs cargo the way Tauri's phase does: without
# RUSTUP_TOOLCHAIN, from src-tauri and from the generated Xcode/Gradle project folders.
# Usage (from the repository root): party/ci/pin-stable-toolchain.sh
set -euo pipefail

root=$(git rev-parse --show-toplevel)
toolchain=${PARTY_RUST_TOOLCHAIN:-stable}

rustup override set "$toolchain" --path "$root"

for dir in "$root" "$root/src-tauri" "$root/src-tauri/gen/apple" "$root/src-tauri/gen/android"; do
    [ -d "$dir" ] || continue
    active=$(cd "$dir" && env -u RUSTUP_TOOLCHAIN rustup show active-toolchain)
    echo "$dir: $active"
    case "$active" in
        "$toolchain"-*) ;;
        *) echo "::error::cargo in $dir would use '$active', not $toolchain"; exit 1 ;;
    esac
done
(cd "$root/src-tauri" && env -u RUSTUP_TOOLCHAIN cargo --version)
