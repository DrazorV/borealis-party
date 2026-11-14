# In-app updates for the desktop apps

The Windows app (the NSIS installer, `borealis-party-windows-x64-setup.exe`) and the Linux AppImages update
themselves. A little after it starts, and every six hours while it stays open, the app reads the update feed of the
fork's latest release. When that names a newer version, a bar offers **Install and restart**. The bar is under the
account bar, or at the top of the sign-in screen. The app downloads the file for its system, checks its signature
against the public key it was built with, installs it and starts the new version. Nothing is installed without that
click, and a file with a wrong signature is refused.

- **Windows:** the installer runs on its own (per user, no administrator prompt), closes the app and opens the new
  version.
- **Linux AppImage:** the AppImage file is replaced where it is, and the app restarts.
- **Not updated in the app:** the `.deb` (update it the way it was installed), the MSI, the phone apps and a
  development build. These never show the bar.

Characters and settings are not touched: they live in the app's data folder (`%APPDATA%\app.borealis.party`,
`~/.local/share/app.borealis.party`), not next to the program.

## How it is put together

All of it is in the fork's own files; upstream's code is used as it is.

- `party/tauri.party.desktop.json` gives upstream's updater plugin the party feed
  (`https://github.com/fortress-soft/borealis-party/releases/latest/download/borealis-party-latest.json`) and the party
  public key (`plugins.updater`), and lets the main window use the plugin's `check` and `download_and_install`
  (`app.security.capabilities`, after upstream's own `default` and `splash`). With a real key, upstream registers the
  plugin for a copy that can replace itself (an NSIS install, an AppImage) and leaves it out otherwise.
- The desktop app's version is the party version from `party/Cargo.toml`, set at build time (`build-desktop.ps1` and
  `party-apps.yml`). Up to 0.1.1 the desktop apps called themselves 1.4.0 (upstream's version).
- `ui/party/api/desktopUpdate.ts` and `ui/party/pages/DesktopUpdate.tsx`: the check and the bar. Upstream's own
  update notice is not used: it asks upstream's GitHub about upstream's version.
- `party-apps.yml`: with the key, the desktop jobs sign the installer and the AppImages (`.sig` beside each), and the
  release job writes the feed with `party/desktop/update-feed.py`. Its links name the files of that tag. Without the
  key the apps are released as before, with no feed, and installed apps simply find no update.

## The key

The private key signs every update. If it is lost, installed apps can't take updates any more: each one needs one
manual install of a build with a new public key. If it leaks, someone could sign updates the apps would accept.

- Kept at `%USERPROFILE%\bp-desktop-updater\borealis-party-updater.key`, its password in
  `borealis-party-updater.password.txt` beside it, and the public key in `borealis-party-updater.key.pub`. Never commit
  or print them. Back the folder up somewhere safe.
- For the release workflow, add two repository secrets on GitHub (Settings > Secrets and variables > Actions):
  `PARTY_UPDATER_KEY` (the whole content of the `.key` file) and `PARTY_UPDATER_KEY_PASSWORD` (the password).
- A new key means a new `plugins.updater.pubkey` in `tauri.party.desktop.json`, and one manual install everywhere.

## The first update-capable release

Apps installed before this change have no updater, and they call themselves 1.4.0. Install the first release with the
updater by hand once. Windows may say a newer version is already installed: let it replace that version, or
uninstall Borealis Party first (your characters stay in the data folder). After that, every release arrives in the
app.

## Trying it on one machine

A test copy with its own name and data folder, so an installed Borealis Party is never touched:

1. Write a config (`test-0.0.1.json`) with `"version": "0.0.1"`, `"productName": "Borealis Party Update Test"`,
   `"mainBinaryName": "borealis-party-updatetest"`, `"identifier": "app.borealis.party.updatetest"`,
   `"bundle": {"createUpdaterArtifacts": true}` and `plugins.updater` with
   `"endpoints": ["http://127.0.0.1:8798/borealis-party-latest.json"]` and `"dangerousInsecureTransportProtocol": true`.
   Make the same file with `0.0.2`.
2. With `TAURI_SIGNING_PRIVATE_KEY` and `_PASSWORD` set, build both:
   `party/desktop/build-desktop.ps1 -Installer -Config <test-0.0.x.json>`, copying `target\party-desktop\release\bundle\nsis`
   away after each.
3. In a folder, put the 0.0.2 installer and its `.sig` under the release names
   (`borealis-party-windows-x64-setup.exe` and `.sig`). Add an empty `borealis-party-linux-x86_64.AppImage` with any
   `.sig`, as the script wants both systems. Run
   `python party/desktop/update-feed.py <folder> 0.0.2 test http://127.0.0.1:8798`, then serve the folder with
   `python -m http.server 8798 --bind 127.0.0.1`.
4. Install 0.0.1 (`<setup>.exe /S`) and start it. About 15 seconds later the sign-in screen offers 0.0.2. Install and
   restart opens 0.0.2.
5. Uninstall the test copy from Settings > Apps, and remove `%APPDATA%\app.borealis.party.updatetest` and
   `%LOCALAPPDATA%\app.borealis.party.updatetest`.
