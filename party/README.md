# Borealis Party server

Borealis Party is Borealis with a multi-user server: every account has its own characters and its own content, and
an admin page manages the accounts. The server runs the Borealis rules engine and stores everything. Browsers, the
Android app and the desktop app are clients.

This version starts from a fresh config folder. It does not read the database or the data folder of the earlier
versions that had groups, and refuses to start on one: move the old config folder away (keep it as a backup) and start
on an empty one.

## Run with Docker

`docker-compose.yml` and `docker-compose.postgres.yml` are in `party/docker`. With SQLite:

```yaml
services:
  borealis-party:
    container_name: "borealis-party"
    image: "ghcr.io/drazorv/borealis-party:latest"
    environment:
      - "TZ=Europe/Athens"
      - "PUID=1026"
      - "PGID=65540"
    network_mode: "bridge"
    ports:
      - "8787:8787/tcp"
    restart: "unless-stopped"
    volumes:
      - "/docker/borealis-party/config:/config"
```

Everything the server keeps is in `/config`: `config.toml`, the SQLite database and the `data` folder with characters
and content. Back up that folder to back up the instance. `PUID` and `PGID` choose who owns those files on the host,
and `TZ` sets the time zone.

Every push to `main` that touches the server or web app builds the image and publishes it as `latest`, `sha-<commit>`
and the version in `party/Cargo.toml` (`.github/workflows/party-image.yml`; start it by hand under Actions > Party
image). To build the image yourself, from the repository root:

```sh
docker build -f party/docker/Dockerfile -t ghcr.io/drazorv/borealis-party:latest .
```

## Settings

On first start the server writes `config.toml` into its config folder, with every setting explained. Edit it and
restart, or set any setting as an environment variable named `BOREALIS__<SECTION>__<KEY>`, which wins over the file.
Put passwords in variables rather than in the file.

| Variable | Default | Meaning |
|---|---|---|
| `BOREALIS__SERVER__BIND` | `0.0.0.0:8787` | Address and port |
| `BOREALIS__SERVER__PUBLIC_URL` | | The address people open, such as `https://party.example.com`; needed for email links |
| `BOREALIS__SERVER__OPEN_REGISTRATION` | `true` | Anyone who can reach the server may ask for an account |
| `BOREALIS__SERVER__REQUIRE_APPROVAL` | `true` | New accounts wait until an admin approves them |
| `BOREALIS__SERVER__MAX_UPLOAD_MB` | `512` | Largest upload |
| `BOREALIS__DATABASE__TYPE` | `sqlite` | `sqlite` or `postgres` |
| `BOREALIS__SQLITE__PATH` | `party.db` | SQLite file, relative to the config folder |
| `BOREALIS__POSTGRES__HOST` | `localhost` | PostgreSQL server |
| `BOREALIS__POSTGRES__PORT` | `5432` | |
| `BOREALIS__POSTGRES__USER` | `borealis` | |
| `BOREALIS__POSTGRES__PASSWORD` | | Required with `postgres` |
| `BOREALIS__POSTGRES__DATABASE` | `borealis_party` | Must exist; the server creates its tables |
| `BOREALIS__POSTGRES__SSLMODE` | `prefer` | `disable`, `prefer` or `require` |
| `BOREALIS__PATHS__DATA` | `data` | Characters and content, relative to the config folder |
| `BOREALIS__SMTP__HOST` | | Mail server; set it to email a confirmation link at sign-up and tell admins about requests |
| `BOREALIS__SMTP__PORT` | `587` | |
| `BOREALIS__SMTP__USER` | | Sign-in name for the mail server, if it needs one |
| `BOREALIS__SMTP__PASSWORD` | | |
| `BOREALIS__SMTP__FROM` | | Sender, `Name <address>` or an address; required with a host |
| `BOREALIS__SMTP__TLS` | `starttls` | `starttls` (port 587), `tls` (port 465) or `none` (trusted relay only) |
| `BOREALIS__INITIAL_ADMIN__USERNAME` | | With the password, creates an admin on a server with no accounts |
| `BOREALIS__INITIAL_ADMIN__PASSWORD` | | |
| `BOREALIS__INITIAL_ADMIN__EMAIL` | | Optional |
| `BOREALIS__LOG__LEVEL` | `info` | `error` to `trace` |

`OPEN_REGISTRATION` and `REQUIRE_APPROVAL` can also be changed on the admin page while the server runs. What is saved
there is kept in the database and wins over the file and the variables.

The config folder itself is `/config` in the container. For a direct install it is set by `BOREALIS_CONFIG_DIR` (one
underscore), which the service file sets to `/var/lib/borealis-party`.

Characters and content packs always stay as files in the data folder, whichever database you choose. The database holds
accounts, sessions, the admin page's settings and its activity log.

## Accounts

Every account is the same: it has its own characters and its own content, as one ordinary Borealis data folder,
`data/users/<account id>/` (`characters/`, `library/`, `settings.json` and so on). Nothing is shared between accounts.
The one difference between accounts is that some are admins.

Sign-up asks for a username, an email address (unique, whatever its case) and a password. Then:

1. With `[smtp]` set up, the server emails a link, and the account goes no further until the link is opened. Without
   it nobody is emailed, and the admin's approval is the check.
2. With approval required (the default), the account waits on the admin page, and admins with an email address get
   an email about it. Signing in meanwhile says what the account is waiting for.

The first account on a new server is its admin, active at once.

The admin page (`/admin`, shown to admins only) approves, rejects, disables and re-enables accounts; resets a password,
either to a temporary one or by asking for a new one at the next sign-in; signs an account out everywhere; makes and
removes admins (there is always at least one); and deletes an account, with its characters and content. The page also
shows statistics, the sign-up settings and an activity log of sign-ups, sign-ins, failed sign-ins and admin actions; the
newest 10,000 entries are kept.

Sign-ups are limited to 10 an hour per client address, and failed sign-ins to 10 per five minutes per address and name.
Behind a reverse proxy every client has the proxy's address, so these limits are then shared.

### Moving from SQLite to PostgreSQL

1. Create an empty PostgreSQL database and a user that owns it.
2. Set `BOREALIS__DATABASE__TYPE=postgres` and the `BOREALIS__POSTGRES__*` variables, then restart.

On that first start the server finds PostgreSQL empty and the SQLite file still in the config folder, and copies
accounts, sign-ins, the admin page's settings and the activity log across in one transaction. Everyone stays signed in.
The log says what was copied, for example `moved to PostgreSQL: users 3, sessions 4, server_settings 1, activity 12`.

The SQLite file is left as it was, so you can go back by setting the type to `sqlite` again. Once PostgreSQL has
accounts the file is never copied again. Delete it when you are happy. To run the copy by hand instead, for example
from another file:

```sh
docker exec borealis-party /app/borealis-server migrate-sqlite --from /config/party.db
```

It refuses if PostgreSQL already has accounts, and saves nothing if anything fails.

## Install on Linux without Docker

Build the release on a Linux machine of the same architecture (needs Rust and Node 20+):

```sh
sh party/server/package.sh
```

Copy the tarball to the server, then:

```sh
tar xzf borealis-party-*.tar.gz
cd borealis-party-*/
sudo ./install.sh
```

The service is `borealis-party`. Its config folder is `/var/lib/borealis-party`, holding `config.toml`, the database and
the data. Passwords and other variables go in `/etc/borealis-party/server.env`. Back up `/var/lib/borealis-party` to
back up everything. Running `install.sh` from a newer release upgrades in place and leaves the data alone.

## Connect

- **Browser:** open `http://<server>:8787`.
- **Android app and desktop app:** on first launch, enter the server's address. A domain name such as
  `party.example.com` means `https://`; an IP address such as `192.168.1.20:8787`, `localhost`, a `.local` name or a
  one-word name such as `raspberrypi:8787` means `http://`. Type the scheme to choose it yourself.

Then sign in or create an account.

If the server later moves to a new address (for example from an IP address to a domain name), change it in the app
under Account > Server address. The app checks that the server there knows the signed-in account, so it stays signed in.

On the open internet, put the server behind a reverse proxy that terminates HTTPS, and use `https://your.domain` as the
address. Plain HTTP is only sensible on a network you trust, because passwords and session tokens would cross it
unencrypted.

New accounts wait for an admin's approval by default. To close sign-up entirely, set
`BOREALIS__SERVER__OPEN_REGISTRATION=false` or switch it off on the admin page. Create the first admin with the
`INITIAL_ADMIN` variables, and other accounts with:

```sh
docker exec -i borealis-party /app/borealis-server add-user <name> [--email <address>] [--admin]
```

The password is read from standard input. For a direct install, run
`sudo -u borealis-party env BOREALIS_CONFIG_DIR=/var/lib/borealis-party /opt/borealis-party/borealis-server add-user ...`.

## Bundles

A `.borealis` bundle made with desktop Borealis's File > Export to bundle can be imported into an account: use
"Import characters" on the Start page, or File > Open bundle.... Borealis's own preview opens, and Accept adds the
characters and content packs to that account only, exactly as desktop Borealis would.

- Making bundles is not offered, by the server or by the party apps. A bundle comes from Borealis itself.
- Device-to-device transfer is not offered either: every device of an account reads the same account on the server.
- A bundle is uploaded as base64, so the largest one is about three quarters of `MAX_UPLOAD_MB`.
- Uploaded bundles wait in the account's `sync/incoming` folder until they are accepted or closed. At most eight wait
  at once (the oldest go first), and any left over are removed the next time the account's workspace opens. So one
  account can hold at most eight uploads' worth of disk there (under 8 x `MAX_UPLOAD_MB`).
- Dropping a `.borealis` file on the window or choosing it with "+" is not supported: those say the file cannot be
  imported. Use "Import characters" or File > Open bundle... instead.

## Offline in the apps

The Android and desktop apps keep a copy of the account's characters and content packs. Without the server they open,
play and edit the characters they have; everything else says "Needs the server. Connect and try again." Back online,
the app sends its edits first, then fetches what changed on the server.

- A character changed only on the device replaces the server's copy.
- Changed on both sides, the newest edit wins (the device's time is corrected to the server's clock). The other copy
  is kept in the account's `sync/trash` folder for 30 days (a device's copy as `<time>-<id>.device.json`), and the
  app's Account page says so. There is no restore button yet: an admin restores a copy by moving it back into the
  account's `characters/` folder as `<id>.json`.
- Deleted on the server: the device drops its copy, unless it edited the character after the delete; then it comes
  back.
- The server's settings win: each sync copies them to the app, less the folders and the machine's own flags.

Signing in and out on an app:

- A session lasts 90 days from its last use (renewed at most once a day), so an app that syncs now and then stays
  signed in; an unused one ends.
- Sign out removes the account's copy from the device. Online the app syncs first. Changes the server still doesn't
  have (made offline, or refused) are listed in a dialog first, with Cancel as the default. Offline, the session on the
  server is not ended (it can't be reached); it ends after 90 days without use, or with "Sign out everywhere".
- Signed out by the server (an admin's "Sign out everywhere", an ended session, a disabled account): the app keeps
  playing offline with its copy and asks to sign in again. The same account picks the copy up and sends its changes.
  Another account signing in on the device removes the earlier copy, after a dialog when it holds unsent changes.
- A password an admin asked to replace comes first when the server can be reached; the sync starts after it.

## How it works

| Rule | Where it is enforced |
|---|---|
| A signed-in account runs commands in its own data folder and in no other | `rpc.rs`, `workspaces.rs` |
| Only commands listed in `backend.rs` are reachable; the rest answer "The server does not offer that." | `backend.rs` |
| Only active accounts sign in; only admins reach `/api/admin` | `auth.rs`, `admin.rs` |
| The machine's settings (folders, updates, AI) are never saved from a client and are reported as fixed values | `settings.rs` |

The HTTP API is small: `/api/auth/*` and `/api/me` for accounts, `/api/admin/*` for the admin page,
`POST /api/rpc/{command}` to run a Borealis command with its arguments as a JSON object, and `GET /api/events` for
the events Borealis sends its window, as server-sent events.

Each account's workspace is opened the way the desktop app opens its data folder, the first time the account does
something, and closed after half an hour without use. Every open workspace holds its own copy of the content in
memory.

## Keeping up with Borealis

This repository is a fork of [TeamBorealis/borealis](https://github.com/TeamBorealis/borealis). To take new upstream work,
use GitHub's "Sync fork" button, or:

```sh
git fetch upstream
git merge upstream/main
sh party/check-upstream.sh
```

Our code lives only in `party/` and `ui/party/`, folders upstream never touches, so merges don't conflict.
`check-upstream.sh` fails if any of upstream's files changes.

A fix we send upstream can be carried here before it is merged: apply exactly the pull request's commits and list each
file with the PR's link in `party/upstream-pending.txt`. The check accepts those files, and once upstream merges the PR and
we merge upstream, it says which lines to remove.

How that works:

- **Backend.** `party/` is its own Cargo workspace. It uses `src-tauri` as a path dependency, and its `[patch]` section swaps
  the `tauri` crates for the webview-free stand-ins in `party/crates`. Upstream's source compiles unchanged.
- **Web app.** `ui/party/vite.config.ts` builds upstream's `src/main.tsx` with three of its imports pointed at `ui/party/entry`,
  and the settings window with its tabs pointed at `ui/party/settings`, which leave out what an account can't change on
  a server. The build fails if upstream renames one of them. `party.css` hides controls that don't apply to an account.
- **Android.** `party/android/party.init.gradle` sets the app id, name and HTTP permission from outside upstream's Gradle files.
- **Tauri.** `party/tauri.party.json` overrides the front end and network policy, and `party/tauri.party.desktop.json`
  the desktop app's name, identifier, update feed and file associations.

After a merge, build and run the tests (on Linux, from `party/`):

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p borealis-server
```

Set `BOREALIS_TEST_POSTGRES_URL` to a database the tests may wipe to run them on PostgreSQL too. A new upstream command stays off the network until it is added to a list in
`party/crates/borealis-server/src/backend.rs`, and an upstream change to `tauri` API usage may need a matching stub in
`party/crates/tauri`.

## Download the apps

Every `party-v<version>` tag builds the phone apps and attaches them to that GitHub release
(`.github/workflows/party-apps.yml`). The names carry no version, so these links always fetch the newest release:

| Asset | Link |
|---|---|
| Android, phones (arm64) | `https://github.com/DrazorV/borealis-party/releases/latest/download/borealis-party-arm64.apk` |
| Android, emulators and x86_64 Chromebooks | `https://github.com/DrazorV/borealis-party/releases/latest/download/borealis-party-x86_64.apk` |
| iOS (only when the iOS secrets are set) | `https://github.com/DrazorV/borealis-party/releases/latest/download/borealis-party-ios.ipa` |
| Checksums | `https://github.com/DrazorV/borealis-party/releases/latest/download/borealis-party-SHA256SUMS.txt` |

To release, set the version in `party/Cargo.toml`, merge to `main`, then tag that commit and push the tag:

```sh
git tag party-v0.1.0
git push origin party-v0.1.0
```

The tag must be `party-v` plus that version, or the workflow stops. Actions > Party apps > Run workflow on `main`
builds the same files as workflow artifacts without a release, to try the build first.

Repository secrets (Settings > Secrets and variables > Actions):

| Secret | Value |
|---|---|
| `PARTY_ANDROID_KEYSTORE_B64` | The PKCS#12 keystore `build-apk.ps1` signs with (`ANDROID_KEYSTORE_PATH`), as base64. Its key alias must be `borealis`, with the store's password |
| `PARTY_ANDROID_KEYSTORE_PASSWORD` | Its password (`ANDROID_KEYSTORE_PASSWORD`) |
| `PARTY_IOS_CERTIFICATE_P12` | Optional. Apple Distribution (or Development) certificate with its private key, a `.p12` as base64 |
| `PARTY_IOS_CERTIFICATE_PASSWORD` | Its password |
| `PARTY_IOS_PROVISIONING_PROFILE` | A provisioning profile for `app.borealis.party` that includes that certificate, as base64 |
| `PARTY_APPLE_TEAM_ID` | The Apple developer team id |

Base64 for a file, without line breaks: `base64 -w0 file` on Linux, `base64 -i file` on macOS, or
`[Convert]::ToBase64String([IO.File]::ReadAllBytes("file"))` in PowerShell.

The Android secrets are required: use the same keystore as the hand builds, because Android only installs an update
signed with the same key. The four iOS secrets go together. Without them the iOS job builds an unsigned app for the
iOS Simulator, kept only as the workflow artifact `party-ios-simulator`, and the release has no iOS app. With them it
builds an ad hoc IPA, which installs only on the devices listed in the profile; set the repository variable
`PARTY_IOS_EXPORT_METHOD` to `debugging` or `app-store-connect` for a profile of that kind.

## Build the Android app

With JDK 17, the Android SDK and the settings listed at the top of the script:

```powershell
party/android/build-apk.ps1
```

Keep the signing key: Android only installs an update signed with the same key.

Known limit: in the Android app the character sheet cannot be saved or shared as a PDF yet (its web view has no
download or share sheet). Open the server in the phone's browser for that.

## Build the desktop app

On Windows, with Rust and the `ui` dependencies installed (`npm --prefix ui ci`), from the repository root:

```powershell
party/desktop/build-desktop.ps1              # target\party-desktop\release\borealis-party.exe
party/desktop/build-desktop.ps1 -Installer   # also an NSIS installer
```

`party/tauri.party.desktop.json` names the app "Borealis Party" with its own identifier, `app.borealis.party`, so it
keeps its data in `%APPDATA%\app.borealis.party`, and the app window never touches desktop Borealis's
`%APPDATA%\app.borealis` on the same computer. The exe still has upstream's `--mcp` and command-line modes; those read
(never write) desktop Borealis's library and search index in `%APPDATA%\app.borealis`, so don't use them. It also leaves out upstream's update feed and file associations, which belong to desktop Borealis.
Like the Android app it is built without device transfer or on-device semantic search. Set `CARGO_HOME` to a short
path such as `C:\c` first, as for the Android app.
