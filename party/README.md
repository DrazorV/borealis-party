# Borealis Party server

Borealis Party is Borealis with a multi-user server: Dungeon Master and player accounts, groups, and a party view for the
DM. The server runs the Borealis rules engine and stores everything. Browsers, the Android app and the desktop app are
clients.

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

To build the image yourself, from the repository root:

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
| `BOREALIS__SERVER__OPEN_REGISTRATION` | `true` | Anyone who can reach the server may sign up |
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
| `BOREALIS__INITIAL_DM__USERNAME` | | With the password, creates a DM on a server with no accounts |
| `BOREALIS__INITIAL_DM__PASSWORD` | | |
| `BOREALIS__LOG__LEVEL` | `info` | `error` to `trace` |

The config folder itself is `/config` in the container. For a direct install it is set by `BOREALIS_CONFIG_DIR` (one
underscore), which the service file sets to `/var/lib/borealis-party`.

Characters and content packs always stay as files in the data folder, whichever database you choose. The database holds
accounts, sessions and groups.

### Moving from SQLite to PostgreSQL

1. Create an empty PostgreSQL database and a user that owns it.
2. Set `BOREALIS__DATABASE__TYPE=postgres` and the `BOREALIS__POSTGRES__*` variables, then restart.

On that first start the server finds PostgreSQL empty and the old SQLite file still in the config folder, and copies
accounts, sign-ins, groups, invitations and memberships across in one transaction. Everyone stays signed in. The log
says what was copied, for example `moved to PostgreSQL: users 3, sessions 4, party_groups 1, invites 0, members 2`.

The SQLite file is left as it was, so you can go back by setting the type to `sqlite` again. Once PostgreSQL has
accounts the file is never copied again. Delete it when you are happy. To run the copy by hand instead, for example
from another file:

```sh
docker exec borealis-party /app/borealis-server migrate-sqlite --from /config/party.db
```

It refuses if PostgreSQL already has accounts or groups, and saves nothing if anything fails.

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
- **Android app:** on first launch, enter `<server>:8787`.

Then sign in or create an account.

On the open internet, put the server behind a reverse proxy that terminates HTTPS, and use `https://your.domain` as the
address. Plain HTTP is only sensible on a network you trust, because passwords and session tokens would cross it
unencrypted.

If strangers can reach the server, set `BOREALIS__SERVER__OPEN_REGISTRATION=false`. Create the first DM with the
`INITIAL_DM` variables, and other accounts with:

```sh
docker exec -i borealis-party /app/borealis-server add-user <name> <dm|player>
```

The password is read from standard input. For a direct install, run
`sudo -u borealis-party env BOREALIS_CONFIG_DIR=/var/lib/borealis-party /opt/borealis-party/borealis-server add-user ...`.

## How it works

| Rule | Where it is enforced |
|---|---|
| Players own characters; they create, edit and delete them | `party/crates/borealis-server/src/rpc.rs` |
| One character per group, a character in at most one group | `members` table in `db.rs` |
| A DM sees a member's character and cannot change it | `groups.rs`, `view` |
| Only DMs add content, from their Library | `rpc.rs`, `check_access` |
| Only commands listed in `backend.rs` are reachable | `backend.rs` |

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
- **Web app.** `ui/party/vite.config.ts` builds upstream's `src/main.tsx` with three of its imports pointed at `ui/party/entry`.
  The build fails if upstream renames one of them. `party.css` hides controls that don't apply to an account.
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
