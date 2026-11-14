# Content updates (0.3.3)

## Before 0.3.3

Upstream checks the user's index files when the desktop app starts (`src-tauri/src/content_auto.rs`). The party server
never started that check for an account, so `content_update_status` stayed `idle`. The "Automatic Updates" setting was
saved but did nothing, 5etools sources were never checked, and the status bar's "Last checked for content updates at
…" was only the time the page loaded. Additional Content also showed each 5etools source twice (the party's
5etools card and upstream's "Import from an address"), with an Update in each, plus upstream's "Update content files".

## Now

- **One button.** Additional Content, Update content: **Update all content** runs the job command
  `update_all_content` (`content_update.rs`, one of `jobs::JOBS`). It updates the index files (`update_indexes`), then
  each 5etools source (`update_5etools_url`, with the bestiary capture), through the same address checks. A part that
  fails goes to `failed`, and the others still run. It answers
  `{ indexes, sources, updated, failed, message, checkedAt, auto }`, with a message such as
  "2 index files and 1 5etools source checked · 1 updated".
- **Each source once.** Upstream's "Update content files" and "Import from an address" sections are hidden
  (`ui/party/content/content.css`). A 5etools source is listed only in the Import section's 5etools card, with a small
  Update and Remove. Clear downloaded content (the index files' downloads) moved to the Update content card.
- **Automatically.** When an account's workspace opens (`Workspaces::get`), `content_update::schedule` waits 10 s and
  starts `update_all_content` as the account's job when:
  - the account's "Update content automatically" setting is on (upstream's `autoContentUpdate`, on by default; the
    Update content card and Settings, Data files change it);
  - the last check is at least 24 h old;
  - the account has an index file or a 5etools source.

  A job the user already runs is left alone. It never blocks a request.
- **The last check** is kept in `users/<account>/party-content-update.json` (`{ at, result }`), so the 24 h count
  across restarts. `content_update_info` answers `{ auto, everyHours, indexes, sources, last }` for the card.
- **The status bar.** `content_update_status` answers upstream's shape from the server's own check. The bar shows
  "Checking for content updates..." while an automatic check runs, then its outcome. A check the user started shows
  its result on the card instead.

Tests: `content_update_tests.rs` (summary, throttle, one run over both kinds with one failing, the job over HTTP, the
automatic start when due and not again within a day, and off), `UpdateAllCard.test.tsx`, `ImportedFrom.test.tsx`, and
the rig's `update-all.spec.ts` at 1440x900 and 384x832.
