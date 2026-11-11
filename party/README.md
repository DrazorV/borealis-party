# Borealis Party server

Borealis Party is Borealis with a multi-user server: Dungeon Master and player accounts, groups, and a party view for the
DM. The server runs the Borealis rules engine and stores everything. Browsers, the Android app and the desktop app are
clients.

## Install on Linux

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

The service is `borealis-party`. It listens on port 8787 and keeps its data in `/var/lib/borealis-party`. Settings are in
`/etc/borealis-party/server.env`. Back up `/var/lib/borealis-party` to back up everything.

Running `install.sh` from a newer release upgrades in place and leaves the data alone.

## Connect

- **Browser:** open `http://<server>:8787`.
- **Android app:** on first launch, enter `<server>:8787`.

Then sign in or create an account.

On the open internet, put the server behind a reverse proxy that terminates HTTPS, and use `https://your.domain` as the
address. Plain HTTP is only sensible on a network you trust, because passwords and session tokens would cross it
unencrypted.

Set `BOREALIS_OPEN_REGISTRATION=false` if strangers can reach the server, and create accounts with:

```sh
sudo -u borealis-party /opt/borealis-party/borealis-server add-user <name> <dm|player>
```

The password is read from standard input. The command needs the same `BOREALIS_*` settings as the service, or the default
paths.

## How it works

| Rule | Where it is enforced |
|---|---|
| Players own characters; they create, edit and delete them | `party/crates/borealis-server/src/rpc.rs` |
| One character per group, a character in at most one group | `members` table in `db.rs` |
| A DM sees a member's character and cannot change it | `groups.rs`, `view` |
| Only DMs add content, from their Library | `rpc.rs`, `check_access` |
| Only commands listed in `backend.rs` are reachable | `backend.rs` |
| Each Application Setting has one owner: the account, the group's DM (house rules), the DM's library, or the server | `party/crates/borealis-server/src/settings.rs` |

Content follows the character's folder. A character outside a group sees the bundled SRD. A character in a group sees the
bundled SRD plus the DM's library, minus the packs the DM turned off for that group. See the layout comment at the top of
`party/crates/borealis-server/src/workspaces.rs`.

## Keeping up with Borealis

This repository is a fork of [TeamBorealis/borealis](https://github.com/TeamBorealis/borealis). To take new upstream work,
use GitHub's "Sync fork" button, or:

```sh
git fetch upstream
git merge upstream/main
sh party/check-upstream.sh
```

Our code lives only in `party/` and `ui/party/`, folders upstream never touches, so merges don't conflict. The one exception
is a 3-line hook in `src-tauri/src/state.rs`. `check-upstream.sh` fails if anything else in upstream's files changes.

How that works:

- **Backend.** `party/` is its own Cargo workspace. It uses `src-tauri` as a path dependency, and its `[patch]` section swaps
  the `tauri` crates for the webview-free stand-ins in `party/crates`. Upstream's source compiles unchanged.
- **Web app.** `ui/party/vite.config.ts` builds upstream's `src/main.tsx` with three of its imports pointed at `ui/party/entry`,
  and the settings window with its tabs pointed at `ui/party/settings`, which leave out what an account can't change on
  a server. The build fails if upstream renames one of them. `party.css` hides controls that don't apply to an account.
- **Android.** `party/android/party.init.gradle` sets the app id, name and HTTP permission from outside upstream's Gradle files.
- **Tauri.** `party/tauri.party.json` overrides the front end and network policy.

After a merge, build and run the tests. A new upstream command stays off the network until it is added to a list in
`party/crates/borealis-server/src/backend.rs`, and an upstream change to `tauri` API usage may need a matching stub in
`party/crates/tauri`.

## Build the Android app

With JDK 17, the Android SDK and the settings listed at the top of the script:

```powershell
party/android/build-apk.ps1
```

Keep the signing key: Android only installs an update signed with the same key.
