# Borealis Party server

Borealis Party is Borealis with a multi-user server: every account has its own characters and its own content, and
an admin page manages the accounts. The server runs the Borealis rules engine and stores everything. Browsers, the
Android and iOS apps and the desktop app are clients.

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
| `BOREALIS__SERVER__MAX_UPLOAD_MB` | `512` | Largest upload, in MB |
| `BOREALIS__SERVER__PUBLIC_UPLOAD_MB` | `95` | Largest upload through the public address, in MB (see [Upload size](#upload-size)) |
| `BOREALIS__SERVER__APP_DOWNLOAD` | the latest GitHub release | Folder URL the website offers the apps from; `off` offers none |
| `BOREALIS__SERVER__TRUSTED_PROXIES` | | Reverse proxies allowed to name the client, such as `172.20.20.100`; commas between entries (see [Security and proxies](#security-and-proxies)) |
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
- **Android, iOS and desktop apps:** on first launch, enter the server's address. A domain name such as
  `party.example.com` means `https://`; an IP address such as `192.168.1.20:8787`, `localhost`, a `.local` name or a
  one-word name such as `raspberrypi:8787` means `http://`. Type the scheme to choose it yourself.

Then sign in or create an account.

A browser on the website sees a small, dismissible note on the sign-in page and the Start page, recommending the app
for its system from the `server.app_download` folder (by default
`https://github.com/DrazorV/borealis-party/releases/latest/download`), with the address to enter in the app:

- Android: `borealis-party-arm64.apk`.
- Windows: `borealis-party-windows-x64-setup.exe`.
- Linux: `borealis-party-linux-x86_64.AppImage`, and `borealis-party-linux-amd64.deb` as a second link; on ARM (when
  the browser says so) `borealis-party-linux-aarch64.AppImage` and `borealis-party-linux-arm64.deb`.
- iPhone and iPad: no download. A release's `borealis-party-ios.ipa`, when the iOS secrets are set, is an ad hoc build
  that installs only on the devices listed in its provisioning profile, so the note says there is nothing to install.
- macOS: no build yet; the note says so.

Each note links to the release page for other systems; inside the apps, and on other systems, there is none. To offer
your own builds, point `BOREALIS__SERVER__APP_DOWNLOAD` at a folder holding the same file names; `off` hides the note.
The file names are in `ui/party/api/appOffer.ts`.

If the server later moves to a new address (for example from an IP address to a domain name), change it in the app
under Account > Server address. The app checks that the server there knows the signed-in account, so it stays signed in.

On the open internet, put the server behind a reverse proxy that terminates HTTPS, and use `https://your.domain` as the
address. Plain HTTP is only sensible on a network you trust, because passwords and session tokens would cross it
unencrypted.

### Upload size

Files go up as base64 in one request, which is a third larger than the file, so the largest file is about three
quarters of the request limit. Two limits apply:

- `MAX_UPLOAD_MB` (512) is the server's own. On the home network (a `localhost`, `192.168.x.x`, `10.x.x.x` or
  `172.16-31.x.x` address, a `.local` or `.lan` name, or a Tailscale `100.64-127.x.x` address) it is the only one, so
  files up to about 383 MB get through. A reverse proxy there must allow as much (Nginx: `client_max_body_size 512m;`).
- `PUBLIC_UPLOAD_MB` (95) is the limit through the public address. A proxy or tunnel in front of the server can stop
  requests long before the server would: Cloudflare's free plan refuses any request over 100 MB, which is about 71 MB
  of file. Set it to your proxy's cap, less a little room. It is never more than `MAX_UPLOAD_MB`.

The server reports both limits (`/api/server`, `uploads`). The app checks a file against them before it reads it,
and says how large it may be and that the home-network address takes more, instead of trying and failing. Upload
errors are put in words too: a refused size, a timeout (Cloudflare gives up after 100 seconds, while the server may
still be finishing, so look before trying again), a server that is restarting, and a lost connection.

New accounts wait for an admin's approval by default. To close sign-up entirely, set
`BOREALIS__SERVER__OPEN_REGISTRATION=false` or switch it off on the admin page. Create the first admin with the
`INITIAL_ADMIN` variables, and other accounts with:

```sh
docker exec -i borealis-party /app/borealis-server add-user <name> [--email <address>] [--admin]
```

The password is read from standard input. For a direct install, run
`sudo -u borealis-party env BOREALIS_CONFIG_DIR=/var/lib/borealis-party /opt/borealis-party/borealis-server add-user ...`.

## Security and proxies

### Addresses the server downloads from

Any account can give the server an address to download from: an index (Indexes tab, or the index a shared bundle's
packs came from) or a 5etools folder. So the server checks each address before it saves or fetches it, and refuses:

- addresses on this machine or a private network: `127.x`, `10.x`, `172.16-31.x`, `192.168.x`, `100.64-127.x`
  (carrier and Tailscale), `169.254.x` (link-local, cloud metadata), IPv6 `::1`, `fc00::/7`, `fe80::/10`, reserved
  ranges, and IPv4 written as IPv6;
- local names: `localhost`, one-word names, `.local`, `.lan`, `.home`, `.internal`, `.home.arpa` and similar;
- names that resolve to any such address;
- the server's own `public_url` host;
- addresses with a user name or password (`https://user:pw@...`) or an access key in the query (`?token=`, `?key=`,
  `?signature=`, ...): the address is saved, shown, and travels inside bundles.

The account is told why ("The server does not download from 192.168.1.1: it is on a home or office network").
Every file an index or a 5etools folder lists is checked the same way when it is fetched, so are indexes saved before
this check existed, so an index can't point the server at the network either. The Indexes tab shows Borealis's own
sentence for a refused index file; the server log has the reason.

This check alone is not enough: Borealis's HTTPS client looks the name up again when it connects and follows
redirects, so a name that answers differently the second time, or a redirect, still gets through. The firewall rule
below closes that, and it is the real protection.

### Runbook: keep the server's container off the home network

Done on the Docker host (for a direct install, the same rules with `-m owner --uid-owner borealis-party` in the
`OUTPUT` chain instead). The example addresses are the reference setup: home network `172.20.20.0/24`, PostgreSQL on
`172.20.20.247`, DNS (Pi-hole) on `172.20.20.2`. Use yours.

1. Give the server a network of its own, so the rule touches no other container. In the stack:

   ```yaml
   services:
     borealis-party:
       networks: [borealis]
       # remove network_mode: "bridge"
   networks:
     borealis:
       ipam:
         config:
           - subnet: 172.30.248.0/24
   ```

2. Allow what the server needs on the home network, then refuse the rest of it. Docker leaves the `DOCKER-USER`
   chain to you; rules there run before Docker's own:

   ```sh
   NET=172.30.248.0/24
   iptables -N BOREALIS-EGRESS 2>/dev/null || iptables -F BOREALIS-EGRESS
   iptables -C DOCKER-USER -s $NET -j BOREALIS-EGRESS 2>/dev/null ||
     iptables -I DOCKER-USER -s $NET -j BOREALIS-EGRESS
   iptables -A BOREALIS-EGRESS -m conntrack --ctstate ESTABLISHED,RELATED -j RETURN
   iptables -A BOREALIS-EGRESS -d 172.20.20.247 -p tcp --dport 5432 -j RETURN      # PostgreSQL
   iptables -A BOREALIS-EGRESS -d 172.20.20.2 -p udp --dport 53 -j RETURN          # DNS
   iptables -A BOREALIS-EGRESS -d 172.20.20.2 -p tcp --dport 53 -j RETURN
   # A mail relay on the home network needs its own line here (tcp 587 or 465).
   for range in 10.0.0.0/8 172.16.0.0/12 192.168.0.0/16 100.64.0.0/10 169.254.0.0/16 127.0.0.0/8 0.0.0.0/8 224.0.0.0/4 240.0.0.0/4; do
     iptables -A BOREALIS-EGRESS -d $range -j REJECT --reject-with icmp-admin-prohibited
   done
   # The Docker host itself, reached through the network's gateway, is INPUT, not DOCKER-USER:
   iptables -C INPUT -s $NET -m conntrack --ctstate NEW -j REJECT --reject-with icmp-admin-prohibited 2>/dev/null ||
     iptables -I INPUT -s $NET -m conntrack --ctstate NEW -j REJECT --reject-with icmp-admin-prohibited
   ```

   With IPv6 on in Docker, add the same with `ip6tables` for `fc00::/7`, `fe80::/10` and `::1`.

3. Make it survive a restart: put step 2 in a script and run it from a systemd unit ordered
   `After=docker.service` with `Requires=docker.service` (Docker recreates `DOCKER-USER` empty when it starts), or
   use `netfilter-persistent` if it loads after Docker.

4. Check from the server's network. The image has no download tool, so use a throwaway container on the same
   network (`docker network ls` shows its full name, usually `<stack>_borealis`). The first two must fail at once, the
   last must print a page:

   ```sh
   docker run --rm --network <stack>_borealis curlimages/curl:8.11.1 -sS -m 5 http://172.20.20.1/
   docker run --rm --network <stack>_borealis curlimages/curl:8.11.1 -sS -m 5 http://172.20.20.100:81/
   docker run --rm --network <stack>_borealis curlimages/curl:8.11.1 -sS -m 5 https://example.com/
   ```

   Then, in the app, Indexes > Update: indexes on the internet still update.

### Real client addresses behind a proxy

Sign-up attempts are limited per client address, and failed sign-ins per client and user name. Behind a reverse
proxy every request comes from the proxy, so without the setting below there is one sign-up limit for everyone, and
ten wrong passwords from anyone lock that user name for everyone for five minutes.

`server.trusted_proxies` (`BOREALIS__SERVER__TRUSTED_PROXIES`, commas between entries) lists the proxies, as
addresses or ranges. Only a request whose connecting address is in the list may name the client: the server reads
`CF-Connecting-IP` (set by Cloudflare), otherwise the right-most `X-Forwarded-For` entry that is not itself a listed
proxy (entries left of it are whatever the client sent, and are ignored). From any other address both headers are
ignored, and the server logs once that it saw them and which setting to change. Empty, the default, means no proxy is
believed, which is safe but gives the shared limits above. IPv6 clients are counted per /64.

A client that reaches the proxy without going through Cloudflare (on the home network) can write its own
`CF-Connecting-IP`. That only lets it choose its own rate-limit bucket on the home network.

### Runbook: production behind Cloudflare and Nginx Proxy Manager

1. Find the address the server sees for the proxy: the admin page's activity log shows the client of each sign-in.
   Today every entry shows the proxy's address (in the reference setup `172.20.20.100`, Nginx Proxy Manager; if NPM
   runs on the same Docker host, a `172.x` bridge address instead).
2. Add to the stack's environment, and update the stack:

   ```
   BOREALIS__SERVER__TRUSTED_PROXIES=172.20.20.100
   ```

   List only proxies you run. Cloudflare's tunnel connects to NPM, not to the server, so it needs no entry.
3. Sign in once from a phone on mobile data and once from the home network: the activity log now shows the phone's
   public address and the computer's home address, not the proxy's.
4. Lock the server's port so only the proxy can reach it, on the Docker host. A published port is DNAT'ed, so match
   the original port:

   ```sh
   RULE="-p tcp -m conntrack --ctorigdstport 8787 --ctdir ORIGINAL ! -s 172.20.20.100 -j REJECT"
   iptables -C DOCKER-USER $RULE 2>/dev/null || iptables -I DOCKER-USER $RULE
   ```

   Keep it with the egress rules (step 3 above). The container's own health check runs inside it and is not affected.
   After this, home-network devices use the server's name (a local DNS record that points it at the proxy, as with
   Pi-hole) instead of `http://<docker host>:8787`. The app counts a domain name as the public address, so for a
   file bigger than `PUBLIC_UPLOAD_MB` allows, add an NPM proxy host for a home-only name (for example `party.lan`) or
   leave a LAN address allowed in that rule: `! -s 172.20.20.0/24` instead keeps the port open to the home network
   only, where the trusted proxy setting still stops anyone else from naming a client.
5. Check from another home device: `curl -m 5 http://<docker host>:8787/api/server` is refused, and the site still
   opens through its name.

## Bundles

A `.borealis` bundle made with desktop Borealis's File > Export to bundle can be imported into an account: use
"Import characters" on the Start page, or File > Open bundle.... Borealis's own preview opens, and Accept adds the
characters and content packs to that account only, exactly as desktop Borealis would.

- File > Export to bundle... (the desktop app and the browser) makes one from the account, with Borealis's own builder,
  and saves it as a download. To hand someone all of an account's content, untick every character and choose "All my
  content packs": that is every pack in the account's library, imported homebrew and packs from indexes alike, in
  `Borealis content.borealis`. They open it in desktop Borealis or in their own account with File > Open bundle....
  Packs from an index keep updating: each records its index, and accepting the bundle in a party account also adds
  those indexes to that account, so its content updates follow them. Packs the account already had unchanged add
  nothing, so an index it removed stays removed. Desktop Borealis takes the packs only, as a snapshot. The iOS app
  offers it as More > Share as a file, through the share sheet, up to 48 MiB per bundle. The Android app does not
  offer it: its web view can't download or share the file and its own backend has no bundle commands, so Android
  will send bundles through the server instead (share codes, planned). The export dialog marks packs whose
  pack info says they are not redistributable; it does not leave them out.
- The character sheet PDF on Android (Save and Share) is made by the app from its own copy of the character, after a
  sync, and goes to the system save picker or share sheet.
- Device-to-device transfer is not offered either: every device of an account reads the same account on the server.
- A bundle is uploaded as base64, so the largest one is about three quarters of the upload limit: about 383 MB on the
  home network, and about 71 MB through Cloudflare's free plan (see [Upload size](#upload-size)). A whole-library
  bundle over that has to be opened on the home-network address.
- Uploaded bundles wait in the account's `sync/incoming` folder until they are accepted or closed. At most eight wait
  at once (the oldest go first), and any left over are removed the next time the account's workspace opens. So one
  account can hold at most eight uploads' worth of disk there (under 8 x `MAX_UPLOAD_MB`).
- Dropping a `.borealis` file on the window or choosing it with "+" is not supported: those say the file cannot be
  imported. Use "Import characters" or File > Open bundle... instead.

## Open with, and web links, in the apps

- **Open with / Share to Borealis Party** (Android and iOS): the app asks first, in its own dialog, "Import
  Hero.dnd5e into alice?", and Import sends the file to that account on the server, as if it had been chosen there:
  a `.dnd5e` (or character `.json`) to the character import, Aurora `.xml`, `.index` and `.zip` to the content import
  preview, 5etools-format `.json` homebrew to Additional Content's 5etools import. Cancel drops it. The upload size
  limits above apply, with the same messages.
- A `.borealis` opened on Android goes up to the server and opens the Open bundle window, as File > Open bundle...
  does. The party apps are built without device transfer, and without it iOS (and the desktop app) would drop an
  opened `.borealis` without a word, so the iOS app doesn't offer to open `.borealis` files: use File > Open bundle....
- Offline, an opened file stays on the device and the app says it will import it when it is connected; the online
  app takes it when it starts. (On Android a file opened while the app was closed is taken on the next online start.)
- Web links: the apps open Borealis's own pages as before, and any other web page (this server's pages, the app
  downloads, a link on a party page) in the system browser. If the device can't, a dialog shows the address with
  Copy link.

## Offline in the apps

The Android, iOS and desktop apps keep a copy of the account's characters and content packs. Without the server they open,
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

This repository is a fork of [TeamBorealis/borealis](https://github.com/TeamBorealis/borealis), kept as a separate port
by agreement with the Borealis team: features that need the user's own device stay in Borealis, and releases are
coordinated so web-relevant work moves over here safely. Fixes that belong in Borealis itself go to them as pull
requests.

On the fork, `.github/workflows/party-ci.yml` checks every push and pull request: no upstream file changed, every
upstream command has a decision, the party web app, and the server on SQLite and PostgreSQL. Upstream's own CI
(`ci.yml`) is turned off here (Actions > CI > Disable workflow): upstream runs it on the same, unchanged upstream code,
and on the fork it rebuilt everything from scratch for about an hour. Turn it back on there if it is ever needed.

Each party release is built on a Borealis release (a `vX.Y.Z` tag), not on upstream's `main` between releases. To take
one, from Git Bash:

```sh
bash party/sync-upstream.sh          # the newest Borealis release, or name one: bash party/sync-upstream.sh v1.4.1
```

It merges the tag and runs `check-upstream.sh` and `check-commands.sh`. Don't use GitHub's "Sync fork" button, which
merges upstream's `main`.

### Releasing

After Borealis releases:

1. `bash party/sync-upstream.sh`, and resolve what it reports.
2. For each new upstream command `check-commands.sh` names, decide: offer it in
   `party/crates/borealis-server/src/backend.rs` (only JSON, base64 or backend-issued tokens, never a path from the
   client), or list it in `party/commands-not-offered.txt` under the reason. Look over upstream's release notes for new
   menu rows or pages that need hiding on a server.
3. Build and test (below), and try the web app and the Android app against a test server.
4. Raise `version` in `party/Cargo.toml`, build once so `Cargo.lock` follows, commit, and push `main`. The push
   builds the Docker image.
5. `bash party/release.sh` tags `party-v<version>` with the Borealis release in its message. Push the tag, then make
   the GitHub release from it with the APK and the server archive, starting the notes with "Based on Borealis X.Y.Z".

A party-only fix between Borealis releases is released the same way, skipping step 1. If `main` has to carry Borealis
work newer than its last release, `release.sh --unreleased` allows it and says so in the tag.

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
- **iOS.** `party/tauri.party.ios.json` sets the app id and merges `party/ios/Info.plist` (name, plain HTTP, local
  network text) into upstream's Info.plist while Tauri builds; `party/ios/build-ios.sh` puts upstream's Xcode project
  back afterwards.
- **Tauri.** `party/tauri.party.json` overrides the front end and network policy, and `party/tauri.party.desktop.json`
  the desktop app's name, identifier, update feed and file associations.

After a merge, build and run the tests (on Linux, from `party/`):

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p borealis-server
```

Set `BOREALIS_TEST_POSTGRES_URL` to a database the tests may wipe to run them on PostgreSQL too. A new upstream command stays off the network until it is added to a list in
`party/crates/borealis-server/src/backend.rs` (`check-commands.sh` names each one), and an upstream change to `tauri` API usage may need a matching stub in
`party/crates/tauri`.

## Download the apps

Every `party-v<version>` tag builds the phone and desktop apps and attaches them to that GitHub release
(`.github/workflows/party-apps.yml`). The names carry no version, so these links always fetch the newest release
(each one is `https://github.com/DrazorV/borealis-party/releases/latest/download/` followed by the asset name):

| App | Asset |
|---|---|
| Android, phones (arm64) | `borealis-party-arm64.apk` |
| Android, emulators and x86_64 Chromebooks | `borealis-party-x86_64.apk` |
| iOS (only when the iOS secrets are set) | `borealis-party-ios.ipa` |
| Windows x64, installer for the current user | `borealis-party-windows-x64-setup.exe` |
| Windows x64, MSI (only when it built) | `borealis-party-windows-x64.msi` |
| Linux x86_64, AppImage | `borealis-party-linux-x86_64.AppImage` |
| Linux x86_64, Debian and Ubuntu | `borealis-party-linux-amd64.deb` |
| Linux arm64, AppImage (only when it built) | `borealis-party-linux-aarch64.AppImage` |
| Linux arm64, Debian and Ubuntu (only when it built) | `borealis-party-linux-arm64.deb` |
| Desktop update feed, with a `.sig` per installer and AppImage (only with the updater key) | `borealis-party-latest.json` |
| Checksums of all of them | `borealis-party-SHA256SUMS.txt` |

The desktop apps are built as `party/desktop/build-desktop.ps1 -Installer` builds them and keep their data in
`app.borealis.party`, apart from desktop Borealis. They are not code-signed. Windows SmartScreen therefore shows
"Windows protected your PC" with "Unknown publisher" when the installer starts; choose More info > Run anyway.
Some browsers also warn that the file is not commonly downloaded. The AppImage needs `chmod +x` before it runs. The
Linux packages leave out desktop Borealis's file types, so both can be installed side by side
(`party/tauri.party.linux.json`).

The Windows installer and the AppImages update themselves from the newest release: the app offers Install and
restart when a newer version is out. The `.deb` and the MSI don't. See `party/desktop/updates.md`, including the one
manual install needed to get there from 0.1.1 or older.

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
| `PARTY_UPDATER_KEY` | Optional. The desktop update signing key, the whole `.key` file (`party/desktop/updates.md`). Without it there is no update feed |
| `PARTY_UPDATER_KEY_PASSWORD` | Its password |

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

### Android smoke run

A scripted check of the Android app on the emulator (AVD `BalanceLab_Tablet`, started if none is running), from the
repository root, with Node 22 or later and WSL:

```powershell
party/android/smoke-open-with.ps1 -Build   # build the x86_64 debug APK first
party/android/smoke-open-with.ps1          # with the debug APK already built
```

It sets the screen to 1080x2340 at 450 dpi (384x832 CSS), starts a throwaway server in WSL
(`party/android/smoke/server.sh`: SQLite in a new `/tmp` folder, an account `smoke` with a random password; never
the production server), installs the APK fresh and signs in through the app. Then it opens a `.dnd5e`, an Aurora
`.xml`, a 5etools `.json` and a `.borealis` with the app the way the Files app does (a `content://` URI from
Downloads), and checks the "Import ... into smoke?" dialog, Cancel, and what each import did on the server. Last, a
tap on a web link has to reach the system browser. It drives the page over the WebView's DevTools socket, so it
needs a debug APK. Afterwards it stops the server and removes its folder, deletes the pushed files, clears the app's
data and resets the screen size (`-Keep` leaves all of that for a look around). Screenshots and `results.json` are
written to `target/party-android-smoke`. The server binary is `-ServerBin`, by default
`~/bp-accounts-target/release/borealis-server` in WSL.

## Build the iOS app

Only on a Mac, with Xcode, the `ui` dependencies (`npm --prefix ui ci`) and the Rust target `aarch64-apple-ios`
(`aarch64-apple-ios-sim` for the Simulator), from the repository root:

```sh
party/ios/build-ios.sh                # signed IPA, target/party-ios/Borealis Party.ipa
party/ios/build-ios.sh --sim          # debug app for the iOS Simulator, target/party-ios/Borealis Party.app
party/ios/build-ios.sh --export-method app-store-connect --build-number 1004000
```

A device build is signed: set `APPLE_DEVELOPMENT_TEAM`, or `IOS_CERTIFICATE`, `IOS_CERTIFICATE_PASSWORD` and
`IOS_MOBILE_PROVISION` (a profile for `app.borealis.party`). Like the Android app it is built without device transfer
or on-device semantic search, and the script fails if the built app lacks the party id, name or HTTP setting.

- Plain HTTP (`192.168.1.20:8787`, `.local` names, a typed `http://`) is allowed with `NSAllowsArbitraryLoads`, as on
  Android. The first connection to the home network asks for the Local Network permission; allow it.
- In the iOS app, Share on the character sheet hands the server's PDF to the share sheet, so "Save to Files" works
  from there. Opening a file with the app from Files imports it into the account after asking (see
  [Open with](#open-with-and-web-links-in-the-apps)); `.borealis` files are not offered there. Not yet tried on an
  iPhone.

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

## Phone UI tests

`party/ui-tests` is a Playwright suite for the web app on phones: Chromium as an Android phone at 384x832 and 832x384
(a Samsung S26 Ultra), and WebKit as an iPhone at 390x844, all through upstream's `?mock=mobile` phone layer. It starts
its own throwaway server on a free port of 127.0.0.1, with a new temporary config folder and SQLite database, and seeds
an admin and a player account; it never talks to another server. It checks that sign-in works with the on-screen
keyboard open, that the main pages don't scroll sideways, that dialogs open and close, and that the import and export
entry points can be reached. It passes before every release (from 0.1.5).

Build the web app (`node party/build.mjs` in `ui/`) and the server (`cargo build -p borealis-server` in `party/`) first,
then, from the repository root:

```sh
npm --prefix party/ui-tests ci
npm --prefix party/ui-tests run install-browsers
npm --prefix party/ui-tests test
```

The suite looks for the server in `party/target` (or `$CARGO_TARGET_DIR`); `BOREALIS_SERVER_BIN` names another binary.
On Windows with the server built in WSL, set `BOREALIS_SERVER_WSL_BIN` to its Linux path instead (from Git Bash, also
`MSYS_NO_PATHCONV=1`). `test:android` and `test:ios` run one phone only.

Controls smaller than a comfortable tap target (44 px on iOS, 48 px on Android) and content cut off at the screen's
edge are reported, not failed, in `party/ui-tests/findings.md`, with the tests marked `test.fail` for fixes that are
planned. CI runs the suite as the "phone UI" job of `party-ci.yml` and adds `findings.md` to the run's summary.

## Measurements

How long the big imports take, and how large the files really are, so we know whether the server needs background
jobs (a step over about 80 s, because Cloudflare gives up after 100 s with a 524) or chunked uploads (files over
about 70 MB). Measured 2026-11-12 on server 0.1.3 for release 0.1.4.

### Through the public address, without signing in

From the home network, pinned to Cloudflare's public address (`curl --resolve`), because the home DNS sends the name
straight to the proxy. Random bytes, a few requests spaced out:

| What | Result |
| --- | --- |
| Largest request Cloudflare passes | exactly 100 MiB: 104,857,600 bytes reach the server, one byte more gets Cloudflare's own `413 Payload Too Large` HTML page after about 1.3 MB was sent |
| Who answered | below the cap the server's own JSON (`401` from `/api/rpc/*`, which refuses before reading the body; `400` from `/api/auth/login`, which reads the whole body first and fails to parse it, so no sign-in is tried); over the cap Cloudflare, so the proxy's 512m and the server's 512 MB are never the limit through the public address |
| Upload speed, home network → Cloudflare → tunnel → server | 99 MiB read in full by the server in 4.0 s, about 26 MB/s (10 MiB 0.75 s, 60 MiB 2.8 s); this is bounded by the home line, so a phone outside is bounded by its own upload speed instead |
| `/api/server`, time to first byte | 806 ms on a cold connection (TLS), then 46-50 ms |

`PUBLIC_UPLOAD_MB = 95` (MiB) is therefore right: 95 MiB of request is below the cap, which leaves files of about
71 MB.

### On the server itself, no proxy

`party/tools/measure-imports.mjs` against a throwaway local server (release build, SQLite, WSL), signed in as its
first account. Wall-clock seconds for one request:

| Step | Request | Time |
| --- | --- | --- |
| `update_indexes`, 5 community Aurora indexes (4 found, 1 gone), first download | 0.2 MB | **194-204 s** |
| `update_indexes`, the same, again with nothing changed | | **65 s** |
| `stage_5etools_url`, the full 5etools data repository | | 10.6 s |
| `import_5etools`, all 128 sources (17,032 elements, 127 packs) | | 7.5 s |
| `preview_import_pack`, a 36.2 MB Aurora zip (1,873 files, stored) | 48.3 MB | 1.8 s |
| `install_import_preview`, the same (28,867 elements) | | 2.9 s |
| `bundle_export`, the whole library (132 packs) | 26.0 MB back | 2.1 s |
| `stage_bundle` + `bundle_open` + `bundle_accept`, that export into an empty library | 26.0 MB | 0.4 + 0.2 + 2.0 s |

Sizes:

- Whole-library export with everything above: **19.5 MB** (26 MB as base64). The `content.borealis` in use today is
  4.9 MB.
- The Aurora content folder: 41 MB of XML, 36.2 MB as a stored zip, 7.7 MB deflated.

`update_indexes` is the slow one: it fetches each file of every index from the index host, one after another, and
checks every file again on the next update even when nothing changed. Everything else is a few seconds and bound by
the server's disk and CPU.

### Through the public address, signed in

Run by the owner on 2026-11-12 against server 0.1.3 through Cloudflare. To repeat it: against a throwaway account made for it and approved on the admin page (it writes into that
account's library and removes what it added when it ends):

```sh
node party/tools/measure-imports.mjs --server https://party.example.com --via-cloudflare \
  --index <index URL> [--index <index URL> ...] --repeat-update \
  --5etools-url <5etools data repository URL> \
  --aurora-zip <a zip of Aurora content, about 40 MB> \
  --json measure-public.json
```

It asks for the username and the password (not shown) and never prints or saves them. `--via-cloudflare` looks the
name up over DNS-over-HTTPS so the run goes through Cloudflare even on the home network. `--help` lists the other
options (`--bundle FILE` uses a given bundle instead of the export it makes).

| Step | Time | Status |
| --- | --- | --- |
| `update_indexes`, first download (4 community Aurora indexes) | 90.0 s, then about 201 s on the server | **504** from the proxy at 90 s while the server kept going (background jobs, I1, fix this) |
| `update_indexes`, again | 67.7 s | ok |
| `stage_5etools_url` | 9.1 s (`scan_5etools_folder` 0.5 s) | ok |
| `import_5etools` | 19.8 s | ok |
| `preview_import_pack` + `install_import_preview` | not measured | |
| `bundle_export` (size) | 4.5 s | ok |
| `stage_bundle` + `bundle_open` + `bundle_accept` | not measured | |

### What it means for 0.1.4

- **Background jobs (I1): needed, for index updates.** The first download of a set of indexes took over three minutes
  on the server alone, and a plain Update with nothing new took 65 s. Through Cloudflare the app gets a 524 at 100 s
  while the server keeps going, and trying again starts the same work twice. `update_indexes` (and with it the
  5etools downloads, `stage_5etools_url` and `update_5etools_url`, which depend on another host's speed) should run
  as a job. The other imports are well under the limit and can stay one request.
- **Chunked upload (I2): not needed.** The largest real files are well under the cap: a 36 MB Aurora zip (48 MB
  request) and a 19.5 MB whole-library export (26 MB request), against 100 MiB per request through Cloudflare. It
  comes back only if a real file or export goes over about 70 MB.

### Long downloads run as jobs (0.1.4)

`update_indexes`, `stage_5etools_url` and `update_5etools_url` now run as background jobs
(`crates/borealis-server/src/jobs.rs`, client `ui/party/api/overrides/jobs.ts`). The app sends `job_start`, which
answers at once, and then asks `job_status` every 0.5 to 5 seconds until the job ends, so no request waits long enough
for Cloudflare's 524. The pages work as before, and progress still comes through the commands' own events.

- Each account runs one job at a time. If another job is running, the app waits for it to finish and then starts its own.
- A job keeps running if the app is closed, and its workspace stays open until it ends.
- Each job writes a record to `users/<account>/sync/jobs/<id>.json`, which is kept for a day. If the server stops
  during a job, the next start reports "The server restarted during this import; run it again."
- Job ids are random, and an account can see only its own jobs. Every download still goes through the address guard.
- An app connected to a server older than 0.1.4 makes the plain call instead.
