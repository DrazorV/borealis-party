# Parties API (Borealis Party 0.2.1)

The server side of parties: `party/crates/borealis-server/src/parties.rs`. A party is rows in the database only.
Characters never leave their owner's folder (`data/users/<owner>/characters/`), and nobody but the owner writes them.

Every route needs `Authorization: Bearer <token>` of an **active** account, except `POST /api/join/preview`.
Requests and answers are JSON. Field names are camelCase.

## Words

- **DM-capable**: the account switched on "Dungeon Master tools" itself (`POST /api/me/roles/dm`). No admin is
  involved. Only DM-capable accounts can create parties.
- **DM of P**: the account that created party P. Every DM check is this per-party relation, read from the database on
  every request.
- **Member**: a character in a party, keyed by `(ownerId, characterId)`. A character is in 0 or 1 party.
- **Player in P**: an account with at least one character in P. The DM of P can also be a player in P with their own
  character (D4); they still get the DM view of P.

## Errors

Same shape as the rest of the server: `{ "code": "...", "message": "..." }`. The message is for people; act on the
code.

| HTTP | code | When |
|---|---|---|
| 400 | `invalid_argument` | A malformed body or value: a bad name, a character id that isn't `c_` + 32 hex digits, uses or days out of range |
| 401 | `unauthorized` | Not signed in, or the account isn't active (a pending, rejected or disabled account has no working session) |
| 403 | `dm_required` | Creating a party without the DM capability |
| 403 | `password_change_required` | As everywhere else |
| 404 | `not_found` | **Anything you may not see, or that doesn't exist.** One answer for both: a party you're not in, an invite that isn't yours, an expired or used-up invite, an unknown code, an offer of another party. The server also answers 404 to every route here when the parties kill switch is off |
| 409 | `conflict` | The character is already in a party, or already offered; the DM capability can't be switched off while you still run parties |
| 429 | `too_many_requests` | Too many invites (50 a day per account), parties (20 per account), or wrong join codes |

An outsider never gets 403 for a party: always 404.

## Objects

**PartySummary**

```json
{
  "id": "0f6c…",                 // uuid
  "name": "Tuesday Party",
  "ruleset": "dnd2024",          // "dnd2024" | "dnd2014"
  "role": "dm",                  // "dm" | "player": your role in it (the DM of P is always "dm")
  "dmName": "drazor",            // the DM's username
  "memberCount": 4,
  "myCharacters": ["c_…"],       // your own characters in it (the DM's own PC too)
  "createdAt": 1762970000,       // seconds since the epoch
  "updatedAt": 1762970000
}
```

**Member** (a roster card, always made by one projection, `card_view_for` in `hub.rs`)

What you see depends on who you are (plan 3.5, decision D6):

| Field | DM of the party | Owner | Another member |
|---|---|---|---|
| `characterId`, `ownerName`, `mine`, `dmCharacter`, `joinedAt`, `build` (always null) | yes | yes | yes |
| `name`, `level`, `classes`, `ruleset`, `state` | yes | yes | yes |
| `ownerId` | yes | yes | no |
| `online` (the owner has the app open) | yes | no | no |
| `stats`, `readAt` | yes | yes | no |
| `band` (during a combat only; absent otherwise, and always in 0.2.x) | no | no | yes |

```json
{
  "characterId": "c_0123456789abcdef0123456789abcdef",
  "ownerName": "alice",
  "ownerId": "8a1e…",
  "online": true,
  "mine": false,
  "dmCharacter": false,
  "name": "Mira",                // null when the file can't be read
  "level": 3,                    // null when the file can't be read
  "classes": [{ "name": "Fighter", "level": 3 }],   // main class first; [] until the card was read with content
  "ruleset": "dnd2024",          // null when the file can't be read
  "state": "ok",                 // "ok" | "unreadable" (file there but not readable) | "missing" (file gone)
  "stats": {                     // null until the owner plays it or the DM views it (then kept on the server)
    "hp": 17, "hpMax": 28, "tempHp": 0,
    "armorClass": 16, "initiative": 2, "speed": 30, "passivePerception": 13,
    "spellDcs": [{ "class": "Wizard", "dc": 14 }],
    "exhaustion": 0, "concentration": "Bless", "inspiration": false
  },
  "readAt": 1762970000,          // when `stats` were read, seconds
  "band": "bloodied",            // another member, in a combat only: "unhurt" | "bloodied" (at or under half) | "down" | null
  "build": null,
  "joinedAt": 1762970000
}
```

A card is read again from the owner's open session after every command that changes the character (damage, healing,
rests, spell slots, concentration, exhaustion, levels, gear …), so the DM's roster is live. It is kept in memory and
in `<data>/party-cards/`, outside every account's folder; nothing is written into the character.

**DmInvite** (what the DM sees of an invite they made)

```json
{
  "id": "5b2d…",
  "kind": "username",            // "username" | "code"
  "username": "alice",           // kind "username" only, as the DM typed it (lower case)
  "usesLeft": 3,                 // kind "code" only
  "createdAt": 1762970000,
  "expiresAt": 1763574800
}
```

A username invite shows exactly the same whether that account exists or not, and whether the invitee declined it.
Invites that are used up or expired are not listed.

**Offer** (DM view: a player offered this character for the party; the DM's projection of what its saved file says)

```json
{
  "id": "a7c1…",
  "inviteId": "5b2d…",
  "ownerName": "alice",
  "characterId": "c_…",
  "name": "Mira", "level": 3, "ruleset": "dnd2024", "state": "ok",
  "createdAt": 1762970000
}
```

**MyInvite** (what an invitee sees)

```json
{
  "id": "5b2d…",                 // use with /api/invites/{id}/…
  "kind": "username",            // "username": sent to your username; "code": a join code you entered
  "party": { "name": "Tuesday Party", "dmName": "drazor", "ruleset": "dnd2024" },
  "expiresAt": 1763574800,
  "offer": null                  // or { "id": "a7c1…", "characterId": "c_…", "createdAt": … } while you have one pending
}
```

The party id is not shown until you are in it.

## DM capability

| Route | Body | Answer |
|---|---|---|
| `GET /api/me/roles` | | `{ "dm": false }` |
| `POST /api/me/roles/dm` | `{}` | `{ "dm": true }`. Switches it on; idempotent |
| `DELETE /api/me/roles/dm` | | `{ "dm": false }`. 409 `conflict` while you still run parties (delete them first) |

(`POST`/`DELETE` rather than the plan's `PUT`: the server's CORS allows GET, POST, PATCH and DELETE only.)

## Parties

| Route | Who | Body | Answer |
|---|---|---|---|
| `GET /api/parties` | anyone | | `{ "dm": bool, "parties": [PartySummary] }`: every party you run or have a character in, newest first |
| `POST /api/parties` | DM-capable | `{ "name": "Tuesday Party", "ruleset"?: "dnd2024" }` | PartyDetail. 403 `dm_required` without the capability; 429 at 20 parties |
| `GET /api/parties/{id}` | DM or player of it | | PartyDetail |
| `PATCH /api/parties/{id}` | DM of it | `{ "name"?: "…", "ruleset"?: "dnd2014" }` | PartyDetail |
| `DELETE /api/parties/{id}` | DM of it | | `{ "deleted": true }`. Its invites, offers and memberships go too; character files are untouched |
| `GET /api/parties/{id}/roster` | DM or player of it | | `{ "members": [Member] }` |

**PartyDetail** = PartySummary plus `"members": [Member]`, and for the DM only `"invites": [DmInvite]` and
`"offers": [Offer]` (absent for players).

Names: 1 to 60 characters after trimming, no control characters.

## Invites (DM side)

| Route | Who | Body | Answer |
|---|---|---|---|
| `POST /api/parties/{id}/invites` | DM of it | `{ "kind": "username", "username": "alice", "days"?: 7 }` | `{ "sent": true, "invite": DmInvite }`, **the same answer whether or not the account exists**. Inviting the same name again renews the invite (same id) |
| `POST /api/parties/{id}/invites` | DM of it | `{ "kind": "code", "uses"?: 1, "days"?: 7 }` | `{ "invite": DmInvite, "code": "K7QM-3XTR-9BWD-HN2F", "link": "https://…/join#K7QM3XTR9BWDHN2F" }`. The code is shown **only here, once**; only its hash is stored. `link` is null when the server has no `public_url` |
| `DELETE /api/parties/{id}/invites/{invite}` | DM of it | | `{ "deleted": true }` |
| `POST /api/parties/{id}/members` | DM of it | `{ "characterId": "c_…" }` | Member. Adds **your own** character (D4) without an invite |
| `POST /api/parties/{id}/offers/{offer}/accept` | DM of it | `{}` | Member. Takes one use of the invite; 409 when the character joined another party meanwhile |
| `DELETE /api/parties/{id}/offers/{offer}` | DM of it | | `{ "deleted": true }`: turn the offer down |
| `DELETE /api/parties/{id}/members/{ownerId}/{characterId}` | DM of it, or the character's owner | | `{ "removed": true }`. The owner withdraws; the DM removes. The next fetch by that player answers 404 if it was their last character in the party |

`uses`: 1 to 50 (default 1). `days`: 1 to 7 (default 7). Codes are 16 characters (80 bits) of Crockford base32; when
typed, case, spaces and dashes don't matter, and `O`/`I`/`L` read as `0`/`1`/`1`.

## Invites (player side)

| Route | Body | Answer |
|---|---|---|
| `GET /api/invites` | | `{ "invites": [MyInvite] }`: invites to your username, and join codes you entered, that are still open |
| `POST /api/invites/{invite}/accept` | `{ "characterId": "c_…" }` | `{ "party": PartySummary }`. Your character joins at once and the invite is used |
| `POST /api/invites/{invite}/offer` | `{ "characterId": "c_…" }` | `{ "offer": { "id", "characterId", "createdAt" } }`. The DM decides; one offer per invite, and one per character |
| `DELETE /api/invites/{invite}/offer` | | `{ "deleted": true }`: withdraw your offer |
| `POST /api/invites/{invite}/decline` | `{}` | `{ "declined": true }`. It leaves your list; the DM is not told |

The character must be yours (`ownerId` is always the signed-in account), match `c_` + 32 hex digits, exist as a saved
character in your folder, and be in no party. The DM can't browse other players' characters: say so in the UI.

## Join codes

| Route | Auth | Body | Answer |
|---|---|---|---|
| `POST /api/join` | yes | `{ "code": "K7QM-3XTR-9BWD-HN2F" }` | `{ "invite": MyInvite }`. Keeps the code on your account; it then shows in `GET /api/invites`, where you accept or offer. Uses are taken only when a character joins |
| `POST /api/join` | yes | `{ "code": "…", "characterId": "c_…" }` | `{ "party": PartySummary }`: join in one step |
| `POST /api/join/preview` | **no** | `{ "code": "…" }` | `{ "party": { "name", "dmName", "ruleset" }, "expiresAt": … }`, for the join page and the waiting screen. Rate-limited per client address |

**New accounts (D39).** `POST /api/auth/register` takes an optional `"joinCode"`. A valid code is kept on the new
account, even while it waits for approval, and shows in `GET /api/invites` once the account can sign in (until the
invite expires). An invalid code changes nothing, and sign-up never fails over it: check it first with
`/api/join/preview`. A DM's invite never counts as approval. Web link: `https://<server>/join#<code>` (fragment, so the
code never reaches a server log).

## The DM's read-only view (0.2.1)

`POST /api/parties/{party}/members/{owner}/{character}/view/snapshot` (body ignored) → `{ snapshot, card, pseq }`

- The DM of the party only; everyone else, the owner and other members included, gets the usual 404.
- `owner` and `character` must name a member row; they are taken from that row, and nothing else in the request is
  used.
- `snapshot` is upstream's `CharacterSnapshot` as the owner has it now (their open session, or the saved file) without
  `displayPath`, `dirty`, `vaultNote`, `group` and `favorite`. Nothing is opened as a session and nothing is written.
- At most 2 views under way per DM (429 `too_many`), and at most 4 offline players' workspaces opened per DM in
  10 minutes (429).
- Not offered yet: `view/sheet` (the printable HTML sheet needs upstream's sheet code to run without a session).

## Live updates on `GET /api/events` (0.2.1)

Party events come on the account's existing event stream (no second connection), as
`data: {"event": "<name>", "payload": {…}}` with an `id:` line. They are invalidations only: refetch your own view.

| Event | Payload | Meaning |
|---|---|---|
| `party://party` | `{ party, pseq, kind, id }` | A party's view changed. `kind`: `party` (name or ruleset), `roster`, `card` (`id` = the character), and for the DM only `invites`, `offers` |
| `party://mine` | `{ kind }` | Your own list changed: `parties` (you joined, left, were removed, a party was deleted) or `invites` |
| `party://resync` | `{}` | Events may have been missed: refetch everything shown |
| `party://content-changed` | `{ account, kind, command }` | (0.2.3) Your own content changed and the change is complete: refetch the content lists. `kind`: `packs`, `indexes` or `fivetools`; `command`: what changed it (`remove_packs` for the bulk removal, `add_share_links`, a job's command when it ends). Only your own streams get it |

- **Two audiences per party,** each with its own `pseq`: the DM's, and the players'. The players' only moves when what
  players see changes (a member joins or leaves, a rename, a name, class or level; a health band only in a combat), never for an invite,
  an offer or damage that keeps the same band. `GET /api/parties/{id}` and `…/roster` answer with your audience's
  `pseq`; ignore an event whose `pseq` isn't newer.
- **Who hears** is read from the database when the event is sent: a removed member or a deleted party's players hear
  nothing more of it (they get one `party://mine`).
- **Ids** count per account. Reopen with `GET /api/events?after=<last id>`: a resync comes first when you missed
  anything. A stream that falls behind gets a resync instead of a silent drop.
- **Revocation:** signing out ends that session's streams; a new password ends the other sessions'; an admin's sign-out,
  password reset, disabling or deleting the account ends all of its streams. Streams also re-check their session every
  60 s. `X-Accel-Buffering: no` is sent so proxies don't hold events back.

## Kill switch

Server config only (no admin page switch, D40): `[features] parties = false` in `config.toml`, or
`BOREALIS__FEATURES__PARTIES=false`, then restart. Every route above then answers 404 `not_found`, as an unknown
route does, no party event is sent and no card is read, and the data stays. The UI should treat 404 from `GET /api/parties` as "parties are off on this server".
