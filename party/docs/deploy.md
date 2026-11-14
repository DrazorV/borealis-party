# Deploying and updating the server

## Updating

Re-pull the image and restart, for example "Update the stack" with re-pull in Portainer, or
`docker compose pull && docker compose up -d`.

### The automatic backup

Before a new version first changes anything, the server writes a backup of its database (0.3.1 and later). At start
it compares its version with the one that last started (the file `last-started-version` in the config folder) and asks
the database which migration steps are waiting. If the version changed, or a step is waiting, it backs up first:

| Database | How | File |
|---|---|---|
| PostgreSQL | `pg_dump --format=custom`, then checked with `pg_restore --list` | `/config/backups/borealis_party-<old>-to-<new>-<UTC time>.dump` |
| SQLite | `VACUUM INTO`, then `PRAGMA quick_check` | `/config/backups/borealis_party-<old>-to-<new>-<UTC time>.sqlite` |

The backup is written under a `.partial` name and renamed once it is complete and checked. Only then do the migrations
run. **If the backup fails, the server stops with the reason and leaves the database as it was.** Fix the cause (disk
space, permissions on `/config/backups`, a `pg_dump` older than the server) and start again.

The log says what happened:

```
backing up the database before the upgrade (version 0.3.0 to 0.3.1; database steps 7 waiting) into /config/backups
backup written: /config/backups/borealis_party-0.3.0-to-0.3.1-20261113T190512Z.dump (1843200 bytes). Copying it off this host is recommended
```

The first start of a release with backups has no record of the version before it, so its file says `unknown-to-<new>`.
A brand-new, empty database is not backed up.

The newest backup's time and size are on the admin page under Statistics > Server.

**Copy the backups off the host.** They sit in the config folder, on the same disk as everything else: a backup job
for `/config` (or at least `/config/backups`) keeps them safe if that disk fails.

The password reaches `pg_dump` only through its environment (`PGPASSWORD`); it is never a command-line argument and
never logged. The Docker image ships the PostgreSQL 18 client (`/usr/lib/postgresql/18/bin`). `pg_dump` refuses a
server newer than itself, so a server above 18 needs a newer image, or `BOREALIS__BACKUP__POSTGRES_BIN` pointing at a
newer client.

### Settings

| Variable | Default | Meaning |
|---|---|---|
| `BOREALIS__BACKUP__DIR` | `backups` | Folder for the backups, relative to the config folder (`/config/backups`) |
| `BOREALIS__BACKUP__KEEP` | `10` | Automatic backups kept; the oldest are deleted after a new one is written. Only files named `borealis_party-<old>-to-<new>-<time>.dump/.sqlite` are ever deleted. `0` keeps all |
| `BOREALIS__BACKUP__ON_UPGRADE` | `auto` | `skip` starts without a backup, for emergencies only |
| `BOREALIS__BACKUP__POSTGRES_BIN` | the PATH (the image sets `/usr/lib/postgresql/18/bin`) | Folder with `pg_dump` and `pg_restore` |

`BOREALIS__BACKUP__ON_UPGRADE=skip` is for when a backup can't be made and the update can't wait. The log then shows
a warning framed in `!!!` lines. Remove the variable again once the server has started.

### Going back to an older version

Older versions keep booting on a newer schema (they ignore tables they don't know), so most rollbacks need only the
older image. To put the data back as it was before an update, restore the database (next section) from the update's
automatic backup, then start the older image. Then edit `last-started-version` in the config folder, or delete it, so
the next update backs up again.

## Restoring the database

The app makes no scheduled backups. Two kinds of backup can bring the data back:

- **The automatic backup before an update** (above): one file per update in `/config/backups`, holding the database
  as it was just before that update's migrations. Use it to undo an update, or as the newest copy when an update went
  wrong.
- **Your own backups of the database host**: for example a nightly whole-machine or whole-container backup of the
  PostgreSQL host (or of `/config` with SQLite) to a NAS, keeping several versions. Use it for anything else: a lost
  disk, a bad day's changes, or a point older than the last update.

Whichever you use, go in this order:

1. **Stop the server**, so nothing writes while you restore (in a stack: stop the container, keep the stack).
2. **Keep what's there now**: `pg_dump --format=custom` of the current database (or a copy of `party.db`), even
   broken, so the restore can be undone.
3. **Restore** (one of the next two subsections).
4. **Start the image that matches the data.** A file named `borealis_party-<old>-to-<new>-<time>` holds `<old>`'s
   data: start `<old>`'s image, or the current one, which upgrades it again (after a new automatic backup). A host
   backup holds whatever version ran when it was taken; the server's log and `last-started-version` from the same
   backup tell which.
5. **Check the schema** (below).

### From the automatic backup

PostgreSQL: restore the dump into the database, as the role that owns it. The image has the client, so it can be the
tool:

```sh
docker run --rm -it -v /docker/borealis-party/config:/config -e PGPASSWORD \
  --entrypoint /usr/lib/postgresql/18/bin/pg_restore ghcr.io/fortress-soft/borealis-connect:latest \
  --host db.example --username borealis_party --dbname borealis_party \
  --clean --if-exists --no-owner /config/backups/borealis_party-0.3.0-to-0.3.1-20261113T190512Z.dump
```

(`-e PGPASSWORD` passes the variable from your shell; set it there first, without writing it into the command.)
`--clean --if-exists` drops each table before it's recreated, so tables a newer version added stay only if the dump
doesn't know them; the next start of a newer image finds them again (each migration checks its own table).

SQLite: copy the `.sqlite` file over `party.db` in the config folder, and delete `party.db-wal` and `party.db-shm`.

### From a backup of the database host

A backup of the whole PostgreSQL host (a VM or container image) brings back **every** database on it, at the time of
that backup. When the host holds other databases too, don't restore it in place:

1. Restore the backup as a **new** machine or container (your hypervisor's or NAS's "restore to a new ID"), with its
   network off or on an address nothing else uses, so no app connects to it by mistake.
2. Start PostgreSQL there and take a dump of the party database only:
   `pg_dump --format=custom --dbname borealis_party --file borealis_party-restore.dump`.
3. Restore that dump into the live database as in the previous subsection (`pg_restore --clean --if-exists
   --no-owner`).
4. Remove the temporary machine.

When the host holds only this server's database, restoring it in place is fine: stop the server, restore the host,
start the host, then the server. With SQLite, restore the config folder's `party.db` (and drop `party.db-wal` and
`party.db-shm`) from the folder backup.

### Checking the schema

After the server is up, `GET /api/health` (next section) says whether the database is at the step this version
expects: `migrations.ok` is `true` and `migrations.pending` is `[]`. Or ask the database:

```sql
SELECT MAX(version) FROM schema_version;
```

The number is the last migration step the data went through. It must not be higher than the running version's
(`migrations.expected` in the health answer): a higher one means the data came from a newer version, so start that
version's image instead. A lower one is upgraded on start, after an automatic backup. When `pending` lists a step below
the recorded one, a table that step creates is missing (a partial restore): the server creates it again on start.

Steps 8 and 10 are empty since 0.3.5. They were unused since 0.3.3: step 8 created the old in-app assistant's
tables (`ai_providers`, `ai_usage`) and step 10 dropped them again. Both numbers stay, so the version history and
`migrations.expected` don't change; a database at step 10 starts with nothing pending.

## Health monitoring

`GET /api/health` tells an uptime checker whether the server is well. It needs no sign-in and carries no secrets (no
paths, names, addresses or settings), and it is never cached.

```json
{
  "status": "ok",
  "version": "0.3.3",
  "uptimeSeconds": 86400,
  "database": { "reachable": true, "kind": "postgres", "latencyMs": 3 },
  "migrations": { "expected": 9, "pending": [], "ok": true },
  "disk": { "freeBytes": 41234567168, "totalBytes": 107374182400, "freeShare": 0.384, "ok": true },
  "events": { "subscribers": 4 },
  "errors": { "windowSeconds": 300, "answers": 812, "serverErrors": 0, "rate": 0.0, "ok": true }
}
```

| Field | Meaning |
|---|---|
| `status` | `ok`; `degraded` when the disk or the error rate warns; `down` when the database can't be reached (5 s) or isn't at this version's migration step |
| `database` | whether a query answered, the database kind, and how long it took |
| `migrations` | this version's last step, the steps not applied, and whether there are none |
| `disk` | free space on the disk holding the config folder (the database file, data and backups); `ok` is false under 5 % free. `null` where the platform doesn't tell |
| `events` | open live-update streams (`/api/events`): roughly the apps and browser tabs connected now |
| `errors` | API answers in the last 5 minutes and how many were server errors (5xx); `ok` is false at 5 % or more once there are at least 20 answers |

The HTTP status is **503 when `status` is `down`** and 200 otherwise, so:

- a plain HTTP check of `https://<your server>/api/health` alerts when the server or its database is down;
- a keyword check for `"status":"ok"` on the same address also alerts on low disk or many server errors.

Any self-hosted or hosted uptime monitor that can fetch an address works: point it at `/api/health` every minute or
so, with a timeout of about 10 s. Check through the public address to cover the proxy and tunnel too, or the local one
to watch the server alone. The container's own health check (`borealis-server health`, used by the image) only asks
whether the server answers, so a database outage shows in `/api/health`, not as an unhealthy container that keeps
restarting.
