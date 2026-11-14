# Release notes template

Copy this for each GitHub release (`party-v<version>`). Keep what applies, delete the rest.

```markdown
Based on Borealis X.Y.Z.

## What's new

- ...

## Updating the server

Re-pull the image and restart (in Portainer: "Update the stack" with re-pull).

<!-- With migrations: -->
**This release adds database migrations** (steps N to M: ...). They run by themselves when the new server starts.

1. **An automatic backup is written first**, to `/config/backups/borealis_party-<old>-to-<new>-<time>.dump`
   (`.sqlite` on SQLite). If it fails, the server stops and the database is left as it was; the log says why.
   Copying the backup off the host is recommended.
2. After the first start, `SELECT version FROM schema_version;` lists `1` to `M`.
3. Going back to the previous version: [Going back to an older version](https://github.com/fortress-soft/meadhall/blob/main/party/docs/deploy.md#going-back-to-an-older-version).

<!-- Without migrations: -->
No database changes. The server still writes an automatic backup on the first start of the new version, to
`/config/backups`; copying it off the host is recommended.

## Apps

- Android: `BorealisParty-<version>-arm64.apk` installs over the earlier build.
- Desktop: updates itself (Windows setup.exe, AppImage).
- iPhone: ...
```
