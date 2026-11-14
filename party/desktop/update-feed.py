"""Writes borealis-party-latest.json, the desktop apps' update feed (party/desktop/updates.md).

Every <asset>.sig in <dir> names one platform. The download URLs point at <base>/<asset>, by default the release of
<tag> on this fork, so a feed always names the files it was signed with. Windows and the x86_64 AppImage are
required; the arm64 AppImage is optional, as in the release.

Usage: python3 party/desktop/update-feed.py <dir> <version> <tag> [<base url>]
"""

import datetime
import json
import sys
from pathlib import Path

# The updater's platform keys (<os>-<arch>) for the asset names party-apps.yml gives.
PLATFORMS = {
    "windows-x86_64": "borealis-party-windows-x64-setup.exe",
    "linux-x86_64": "borealis-party-linux-x86_64.AppImage",
    "linux-aarch64": "borealis-party-linux-aarch64.AppImage",
}
REQUIRED = ("windows-x86_64", "linux-x86_64")
FEED = "borealis-party-latest.json"


def feed(folder: Path, version: str, base: str) -> dict:
    platforms = {}
    for key, asset in PLATFORMS.items():
        sig = folder / f"{asset}.sig"
        if not sig.is_file():
            continue
        if not (folder / asset).is_file():
            raise SystemExit(f"{asset}.sig without {asset}")
        platforms[key] = {"signature": sig.read_text(encoding="utf-8").strip(), "url": f"{base}/{asset}"}
    missing = [k for k in REQUIRED if k not in platforms]
    if missing:
        raise SystemExit(f"the feed needs these signed: {', '.join(PLATFORMS[k] for k in missing)}")
    return {
        "version": version,
        "notes": f"Borealis Connect {version}",
        "pub_date": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "platforms": platforms,
    }


def main(argv: list[str]) -> None:
    if len(argv) not in (4, 5):
        raise SystemExit(__doc__.strip().splitlines()[-1])
    folder, version, tag = Path(argv[1]), argv[2], argv[3]
    base = argv[4] if len(argv) == 5 else f"https://github.com/fortress-soft/borealis-connect/releases/download/{tag}"
    out = json.dumps(feed(folder, version, base.rstrip("/")), indent=2) + "\n"
    (folder / FEED).write_text(out, encoding="utf-8")
    print(out, end="")


if __name__ == "__main__":
    main(sys.argv)
