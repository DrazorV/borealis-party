#!/bin/sh
# Installs or upgrades the Borealis Connect service from an unpacked release. Run as root from the release folder.
# Characters, accounts and content in /var/lib/borealis-party are never touched.
set -eu

if [ "$(id -u)" -ne 0 ]; then
    echo "Run this as root (sudo ./install.sh)." >&2
    exit 1
fi

here=$(cd "$(dirname "$0")" && pwd)

if ! id borealis-party >/dev/null 2>&1; then
    useradd --system --home-dir /var/lib/borealis-party --shell /usr/sbin/nologin borealis-party
fi

systemctl stop borealis-party 2>/dev/null || true

install -d /opt/borealis-party
rm -rf /opt/borealis-party/web /opt/borealis-party/content
install -m 0755 "$here/borealis-server" /opt/borealis-party/borealis-server
cp -r "$here/web" "$here/content" /opt/borealis-party/
cp "$here/LICENSE-MIT" "$here/LICENSE-APACHE" "$here/NOTICE" /opt/borealis-party/

install -d /etc/borealis-party
if [ ! -e /etc/borealis-party/server.env ]; then
    install -m 0640 -g borealis-party "$here/server.env.example" /etc/borealis-party/server.env
fi

install -m 0644 "$here/borealis-party.service" /etc/systemd/system/borealis-party.service
systemctl daemon-reload
systemctl enable --now borealis-party

echo "Borealis Connect is running. Settings: /var/lib/borealis-party/config.toml (passwords: /etc/borealis-party/server.env)"
echo "Logs: journalctl -u borealis-party -f"
