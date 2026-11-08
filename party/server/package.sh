#!/bin/sh
# Builds a release tarball of the server for this machine's architecture. Run from the repository root.
# Needs Rust and Node. Output: party/target/server-release/borealis-party-<version>-<arch>.tar.gz
set -eu

version=$(sed -n 's/^version = "\(.*\)"/\1/p' party/Cargo.toml | head -n 1 | tr -d '\r')
arch=$(uname -m)
target=${CARGO_TARGET_DIR:-party/target}
out=party/target/server-release/borealis-party-$version-$arch

# SKIP_UI=1 reuses an existing ui/dist.
if [ -z "${SKIP_UI:-}" ]; then
    npm --prefix ui ci
    node ui/party/build.mjs
fi
(cd party && cargo build --release -p borealis-server)

rm -rf "$out"
mkdir -p "$out/content"
cp "$target/release/borealis-server" "$out/"
cp -r ui/party/dist "$out/web"
for pack in content/*/; do
    cp -r "$pack" "$out/content/"
done
cp party/server/install.sh party/server/borealis-party.service party/server/server.env.example "$out/"
cp LICENSE-MIT LICENSE-APACHE NOTICE "$out/"
chmod +x "$out/install.sh"
tar -C party/target/server-release -czf "$out.tar.gz" "$(basename "$out")"
echo "$out.tar.gz"
