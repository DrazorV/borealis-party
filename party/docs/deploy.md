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
older image. To put the data back as it was before an update:

1. Stop the server.
2. PostgreSQL: restore the dump into the database, as the role that owns it. The image has the client, so it can be
   the tool:

   ```sh
   docker run --rm -it -v /docker/borealis-party/config:/config -e PGPASSWORD \
     --entrypoint /usr/lib/postgresql/18/bin/pg_restore ghcr.io/fortress-soft/meadhall:latest \
     --host db.example --username borealis_party --dbname borealis_party \
     --clean --if-exists --no-owner /config/backups/borealis_party-0.3.0-to-0.3.1-20261113T190512Z.dump
   ```

   (`-e PGPASSWORD` passes the variable from your shell; set it there first, without writing it into the command.)

   SQLite: copy the `.sqlite` file over `party.db` in the config folder, and delete `party.db-wal` and `party.db-shm`.
3. Start the older image. Then edit `last-started-version`, or delete it, so the next update backs up again.
