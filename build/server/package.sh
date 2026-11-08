#!/bin/sh
# Builds a release tarball of the server for this machine's architecture. Run from the repository root.
# Needs Rust and Node. Output: target/server-release/borealis-party-<version>-<arch>.tar.gz
set -eu

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1 | tr -d '\r')
arch=$(uname -m)
target=${CARGO_TARGET_DIR:-target}
out=target/server-release/borealis-party-$version-$arch

# SKIP_UI=1 reuses an existing ui/dist.
if [ -z "${SKIP_UI:-}" ]; then
    npm --prefix ui ci
    npm --prefix ui run build
fi
cargo build --release -p borealis-server

rm -rf "$out"
mkdir -p "$out/content"
cp "$target/release/borealis-server" "$out/"
cp -r ui/dist "$out/web"
for pack in content/*/; do
    cp -r "$pack" "$out/content/"
done
cp build/server/install.sh build/server/borealis-party.service build/server/server.env.example "$out/"
cp LICENSE-MIT LICENSE-APACHE NOTICE "$out/"
chmod +x "$out/install.sh"
tar -C target/server-release -czf "$out.tar.gz" "$(basename "$out")"
echo "$out.tar.gz"
