# Borealis Party server

Borealis Party is Borealis with a multi-user server: accounts that run groups as Dungeon Master and play in
others, a party view for the DM, and an admin page. The server runs the Borealis rules engine and stores everything. Browsers, the Android app and the desktop app are
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
| `BOREALIS__INITIAL_ADMIN__EMAIL` | | Optional. The old `BOREALIS__INITIAL_DM__*` names still work |
| `BOREALIS__LOG__LEVEL` | `info` | `error` to `trace` |

`OPEN_REGISTRATION` and `REQUIRE_APPROVAL` can also be changed on the admin page while the server runs. What is saved
there is kept in the database and wins over the file and the variables.

The config folder itself is `/config` in the container. For a direct install it is set by `BOREALIS_CONFIG_DIR` (one
underscore), which the service file sets to `/var/lib/borealis-party`.

Characters and content packs always stay as files in the data folder, whichever database you choose. The database holds
accounts, sessions, groups, the admin page's settings and its activity log.

## Accounts

Every account can run groups and play in them. Whoever creates a group is its Dungeon Master; the people they invite
are its players. Each account has its own *My characters* and its own content *Library*.

Sign-up asks for a username, an email address (unique, whatever its case) and a password. Then:

1. With `[smtp]` set up, the server emails a link, and the account goes no further until the link is opened. Without
   it nobody is emailed, and the admin's approval is the check.
2. With approval required (the default), the account waits on the admin page, and admins with an email address get
   an email about it. Signing in meanwhile says what the account is waiting for.

The first account on a new server is its admin, active at once. When a server from before this existed upgrades, every
account stays active and the oldest one becomes the admin, so nobody is locked out.

The admin page (`/admin`, shown to admins only) approves, rejects, disables and re-enables accounts; resets a password,
either to a temporary one or by asking for a new one at the next sign-in; signs an account out everywhere; makes and
removes admins (there is always at least one); and deletes an account. Deleting takes the account's characters out of
other groups first and deletes the groups it runs, whose characters go back to their players. The page also shows
statistics, the sign-up settings and an activity log of sign-ups, sign-ins, failed sign-ins, groups and admin actions;
the newest 10,000 entries are kept.

Sign-ups are limited to 10 an hour per client address, and failed sign-ins to 10 per five minutes per address and name.
Behind a reverse proxy every client has the proxy's address, so these limits are then shared.

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

New accounts wait for an admin's approval by default. To close sign-up entirely, set
`BOREALIS__SERVER__OPEN_REGISTRATION=false` or switch it off on the admin page. Create the first admin with the
`INITIAL_ADMIN` variables, and other accounts with:

```sh
docker exec -i borealis-party /app/borealis-server add-user <name> [--email <address>] [--admin]
```

The password is read from standard input. For a direct install, run
`sudo -u borealis-party env BOREALIS_CONFIG_DIR=/var/lib/borealis-party /opt/borealis-party/borealis-server add-user ...`.

## How it works

| Rule | Where it is enforced |
|---|---|
| A group's DM is its creator; its players are the people they invited | `rpc.rs`, `group_role` |
| Characters are changed by their owners only, in My characters or a group they play in | `rpc.rs`, `check_access` |
| One character per player per group, a character in at most one group | `members` table in `db.rs` |
| A DM sees a member's character and cannot change it | `groups.rs`, `view` |
| Content is added in a Library; only a group's DM chooses its packs | `rpc.rs`, `check_access` |
| Only the group's DM manages it and its invitations; nobody joins their own group | `groups.rs` |
| Only active accounts sign in; only admins reach `/api/admin` | `auth.rs`, `admin.rs` |
| Only commands listed in `backend.rs` are reachable | `backend.rs` |
| Each Application Setting has one owner: the account, the group's DM (house rules), the DM's library, or the server | `party/crates/borealis-server/src/settings.rs` |
| In a group, characters follow the DM's house rules: their own are off, and what the rules don't allow is taken out (with a note to the player) when the character opens | `workspaces.rs` `Workspace::call`, `backend.rs` `apply_group_rules` |

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
