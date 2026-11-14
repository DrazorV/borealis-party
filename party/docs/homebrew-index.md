# Homebrew and custom indexes (0.3.1, part D)

An account makes its own content in the app: spells, feats, items, magic items, weapons, armor, races, subraces,
backgrounds, class features, archetypes and their features, and monsters. Elements are written as standard Aurora XML and
checked by the same engine that loads the account's characters. A pack can then be **built into a standard `.index`
plus its element files for the author's own repository** (for example a GitHub repository). Others subscribe to that
repository's `.index` in Borealis Connect, plain Borealis or Aurora.

**The server never hosts or publishes an index** (the user's decision D47: copyright risk). Index creation is for people
whose own local agent, or their own hands, put the files in their own git and push them. The server answers the files as
data (`index_build`) or as a zip (the UI's export). Both need the repository's address, and neither has a default.

Code: `party/crates/borealis-server/src/homebrew.rs` (store, forms, XML, engine checks, index building, routes, tools).
UI: `ui/party/pages/HomebrewPage.tsx`, `HomebrewEditor.tsx`, `ui/party/api/homebrew.ts`, `homebrew.css`. Tests:
`homebrew_tests.rs`, `ui/party/pages/Homebrew.test.tsx`, `party/ui-tests/tests/homebrew.spec.ts`. There is no database
change: homebrew lives in the account's folder.

## Content rules

- **Format.** The format is Aurora's. Elements may use Borealis's extensions, such as the `Monster` type, which Aurora
  ignores. The meaning of Aurora syntax never changes.
- **Ids never change**, because characters store ids.
  - A new element's id is made from the pack and its name, as `ID_<PACK>_<TYPE WORD>_<NAME>` (for example
    `ID_TEST_PACK_MAGIC_ITEM_LANTERN_OF_TESTING`), unless the author gives one.
  - These are refused: an edit that changes the id (`id_changed`), an id the pack already has (`duplicate_id`), and an
    id that other installed content defines (`id_taken`).
  - A renamed pack keeps its id and file names.
- **Versions.** Every saved change bumps the element's revision, its file's `<update version>` and the pack's version
  (the last number). The author may set the pack's version, for example `2.0.0`.
- **Files.** There is one file per kind:
  - `spells.xml`, `feats.xml`, `items.xml`, `magic-items.xml`, `weapons.xml`, `armor.xml`;
  - `races.xml` (races and subraces), `backgrounds.xml`, `class-features.xml`;
  - `archetypes.xml` (archetypes and their features), `monsters.xml`.

  Each file has an `<info>` block naming its own address under the repository folder. Aurora drops a file whose
  `<info>` has no `<update>` with a `<file url>`. The copy installed in the library is hosted nowhere, so it names
  `https://homebrew.invalid/`, which Borealis never fetches (it fetches only an index's own address).
- **Validation.**
  - The pack, with the change in it, is converted and loaded against the account's installed content in a scratch
    preview (upstream's `preview_import_pack`), and the preview is then dropped.
  - Each finding comes back with its severity, the engine's code, the element, the file and line, and the line inside
    the element's own XML.
  - A reference to an id that neither the pack nor the installed content has is an error (`unresolved_reference`).
  - Nothing with an error is saved.
  - Rules the engine enforced in testing: a monster's `str`..`cha`, `ac`, `hp` and each `speed*` are whole numbers
    (speeds in feet).
- **Storage.**
  - `<account data>/party-homebrew/<pack id>.json` is the source: the element XML as written.
  - The installed library pack has the same id (`hb-<slug>`) and is rebuilt from that file on every save.
  - Its `pack-meta.json` says `"kind": "homebrew"`.

## Licensing (D14)

Only the account's own homebrew, made in the editor, can be built or exported. Asking for any other pack is refused with
`403 not_redistributable` and a reason: it is bundled, it was converted from 5etools data, it was downloaded from an
index (share that index's address instead), or it was imported from a file. Examples in code and tests are made up or
SRD only.

## Building an index

- **Layout.** The base is the https folder the files will live in. Files go there as `<base><stem>.index` and
  `<base><stem>/<file>.xml`, where the stem is the pack id without `hb-`. This is the layout of the user's own homebrew
  repository: an index at the root, its XML in a folder named after it.
- **The base address.** It is required. A non-https address, or one with `?` or `#`, is refused. So is this server's
  own public host, because the server doesn't host indexes.
- **Structure.** The index's `<info>` has the pack's name, description, author (and the author's page), and
  `<update version>` with its own address. `<files>` lists each element file with its address.
- **Aurora check.** The opt-in corpus check confirms that this structure is a subset of the Aurora Legacy indexes'.

## Routes (any signed-in account, its own packs only)

| Method and path | Body | Answer |
|---|---|---|
| `GET /api/homebrew/types` | | `{ types: [TypeSpec] }`: each type's label, file and form fields (Aurora setters) |
| `GET /api/homebrew/packs` | | `{ packs: [summary] }` |
| `POST /api/homebrew/packs` | `{ name, description?, author?, authorUrl? }` | the pack |
| `GET/PATCH/DELETE /api/homebrew/packs/{pack}` | PATCH: `{ name?, description?, author?, authorUrl?, version? }` | the pack with element rows (DELETE also removes it from the library) |
| `POST /api/homebrew/packs/{pack}/elements` | `{ form }` or `{ xml }` | the element (`xml`, `form`), `report`, `pack` |
| `GET/PATCH/DELETE /api/homebrew/packs/{pack}/elements/{id}` | PATCH as POST | as POST |
| `POST /api/homebrew/validate` | `{ packId?, elementId?, form? \| xml? }` | `{ report, xml }`; with neither form nor XML, checks the whole pack |
| `POST /api/homebrew/convert` | `{ packId?, form }` or `{ xml }` | `{ xml }` or `{ form }` (or `{ report }` for XML that doesn't read) |
| `GET /api/homebrew/packs/{pack}/index?base=` | | `{ base, files: [{ path, text }] }` |
| `GET /api/homebrew/packs/{pack}/export?base=` | | `application/zip`: `<stem>.index` and `<stem>/*.xml` |

A refused save answers **422** with `{ code: "invalid_homebrew", message, report, xml }`. The `report` is
`{ ok, errors, warnings, converted, issues: [{ severity, code, message, element?, file?, line?, elementLine? }] }`.
The UI keeps that report through `serverHooks.fail`.

The form (`ElementForm`) has these fields:
- `type`, `id?`, `name`, and `source?` (the pack's name when empty);
- `description`: paragraphs separated by blank lines, or XHTML when it starts with `<`;
- `supports?`, `prerequisite?`, `requirements?`;
- `setters: [{ name, value, attrs? }]` (a numeric `weight` is written as `lb="n"`);
- `sheet?: { display?, usage?, action?, description? }`;
- `rules: [{ kind: grant | stat | select, attrs }]`;
- `extraXml?`: every other child, kept as XML.

The form and the XML say the same thing. XML the form has no field for goes to `extraXml` and comes back unchanged.

## Tools (party-tools, over MCP)

`homebrew::tools()` lists each tool's `name`, `title`, `scope` (`ToolScope`, serialized as `homebrew:read` or
`homebrew:write`), `description` and JSON `input_schema`. `homebrew::call_tool(state, user, name, args)` runs one tool
for the token's owner. Part B's registry checks scope, rate and idempotency first, and strips `requestId`. At merge,
`tools_backend::external_specs()` maps each spec to `party_tools::ToolSpec::external(...)`. Results are data: text the
user wrote appears only in named fields (`xml`, `description`, `files[].text`). There is no `index:publish` scope.

| Tool | Scope | Does |
|---|---|---|
| `homebrew_types` | read | the types and their fields |
| `homebrew_list` | read | the account's packs |
| `homebrew_get` | read | a pack, or one element's XML and form |
| `homebrew_validate` | read | checks without saving |
| `homebrew_create` | write | adds an element (checked by the engine; with no pack, makes "My Homebrew") |
| `homebrew_update` | write | changes an element (its id stays) |
| `homebrew_save` | write | `homebrew_update` with an `elementId`, else `homebrew_create` |
| `homebrew_delete` | write | deletes an element |
| `index_build` | read | `{ packId, version, base, indexUrl, files: [{ path, text }], zip: { fileName, contentBase64 } }` for `baseUrl` (required) |

The local agent writes `files` into the repository and pushes. A chat app gets `zip` to offer as a download.

## Tests

- `homebrew_tests.rs`:
  - every type round-trips through the real engine (form in; form and compendium out);
  - bad XML, unknown types and unresolved references are reported with their element and line;
  - ids stay and versions go up;
  - an index built for a repository parses with Borealis's index parser, lists every built file at its address, and
    its files import as a pack for another account;
  - the zip holds the same files;
  - building with no address, with an http address or with the server's own host is refused, and there is no
    publishing route;
  - packs that are not the account's own homebrew are refused;
  - the tools match the routes.
- `the_index_has_the_aurora_legacy_shape` is opt-in (`BOREALIS_TEST_AURORA_DIR`, a folder holding Aurora Legacy
  `.index` files). Every index there must parse, and the generated index must use only element and attribute paths
  they use. It prints counts only. On the user's machine: 136 indexes, 1,894 files.
