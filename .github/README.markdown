<div align="center">

<img src="/ui/party/public/favicon.svg" alt="Borealis Connect" width="96">

# Borealis Connect

**Borealis for the whole table: a self-hosted, multi-user server for playing fifth edition together.**

<p>
<a href="https://github.com/fortress-soft/borealis-connect/releases/latest"><img alt="Latest release" src="https://img.shields.io/github/v/release/fortress-soft/borealis-connect?style=for-the-badge&label=release&color=2f875b&labelColor=070a0d"></a>
<a href="https://github.com/TeamBorealis/borealis"><img alt="Built on Borealis" src="https://img.shields.io/badge/built%20on-Borealis-6fd0b5?style=for-the-badge&labelColor=070a0d"></a>
<a href="#licence"><img alt="Licence MIT OR Apache-2.0" src="https://img.shields.io/badge/licence-MIT%20OR%20Apache--2.0-e3b64a?style=for-the-badge&labelColor=070a0d"></a>
</p>

<p><a href="https://github.com/fortress-soft/borealis-connect/releases/latest"><b>Download the apps</b></a> &nbsp;·&nbsp; <a href="#run-your-own-server">Run a server</a> &nbsp;·&nbsp; <a href="https://github.com/fortress-soft/borealis-connect/issues">Issues</a> &nbsp;·&nbsp; <a href="https://github.com/TeamBorealis/borealis">Borealis</a></p>

</div>

---

## Built on Borealis ❤️

Borealis Connect is an add-on to **[Borealis](https://github.com/TeamBorealis/borealis)**, the free and open-source
character builder by **[TeamBorealis](https://github.com/TeamBorealis)**. The character builder, the rules and content
engine, the character sheets and the importers are all their work. This fork adds only the multi-user part on top: the
server, accounts and the party features.

If you like Borealis Connect, you like Borealis. Please give the
**[original repository a star](https://github.com/TeamBorealis/borealis)**, try the
**[desktop and phone apps](https://borealisbuilder.com)**, and come and say hi on the
**[Borealis Discord](https://discord.gg/r4K4D3AJpZ)**.

The two projects work together. Borealis Connect stays a separate port, by agreement with the Borealis team, and the two
coordinate releases: every Borealis Connect release is built on a Borealis release, and fixes that belong in Borealis go
to them as pull requests.

## What it adds

Borealis keeps everything on one device. Borealis Connect puts the same app on a server you run, so a whole group can use
it from any device:

- **A self-hosted server.** One Docker image (`ghcr.io/fortress-soft/borealis-connect`), with SQLite or PostgreSQL. It runs
  the Borealis rules engine and stores every account's characters and content.
- **Accounts.** Each account has its own characters and its own content. New accounts wait for an admin's approval
  (by default), with optional email confirmation. The admin page approves, disables and resets accounts and keeps an
  activity log.
- **Every device.** Use it in the browser, or with the Android, iPhone (unsigned, for sideloading) and Windows and Linux
  apps. The Windows installer and the Linux AppImages update themselves.
- **Offline play.** The apps keep a copy of your characters, so you can play and edit them without the server. They
  sync when you are back online.
- **One import page.** Aurora `.dnd5e` characters, Aurora XML content and `.index` files, `.borealis` bundles, and
  5etools-format data you supply, all in one place. In the apps, **Open with** (or Share to) Borealis Connect takes a file
  such as a `.borealis` bundle straight to the import, after asking.
- **Share codes.** Send characters and content packs to another player with a one-time code, Android included.
- **Homebrew.** Write your own spells, feats, items, races, backgrounds, class features, archetypes and monsters in the
  app, checked by the same engine as your characters. Build a pack into a standard `.index` plus its element files for
  your own repository, so others can subscribe to it from Borealis Connect, Borealis or Aurora.
- **Parties.** Switch on "Dungeon Master tools" on your Account page to run parties: invite players by username or with
  a join code, add characters, and set the party's edition (2014 or 2024 rules). The **live party roster** updates as
  characters are played: health, AC, concentration and who is online.
- **Bestiary and encounters.** Every creature in your installed content, searchable. The Dungeon Master builds
  encounters from it, with per-encounter changes, pets and summons, and decides which creatures players see.
- **Combat tracker.** Initiative, turns, hit points, conditions, concentration and legendary actions, run by the server
  so everyone stays in step. The DM works in the **Combat** tab and the players in their own **Play** tab, and each
  side sees only what it should: players see a monster's name and whether it is bloodied, and in a fight the DM sees
  the characters' health bands, not their exact hit points.
- **Spells as mechanics.** SRD spells carry their saves, attacks, damage and effects, so casting one in combat applies
  it. Spells from your own 5etools data get a draft of their mechanics that you can check and correct.
- **Simulations.** Play an encounter out on a copy, never the real fight, as often as you like, and read what happened.
- **Your own AI, over MCP.** Connect your own ChatGPT, Claude or another MCP client to your account: it can search
  the bestiary, build and simulate encounters, read characters and write homebrew, with the permissions you approve,
  and it sees only what you may see. Borealis Connect never calls an AI service itself, and your own plan pays for
  the AI.
- **Safe upgrades.** Before each update's database changes the server makes an automatic backup, and `GET /api/health`
  tells an uptime checker whether the server and its database are well.

**In progress:** 0.3.4 brings quick wins for players, and the 0.4 releases continue with larger features. The
[releases](https://github.com/fortress-soft/borealis-connect/releases) list what each version brings.

## Run your own server

With Docker (or a Portainer stack), using SQLite:

```yaml
services:
  borealis-connect:
    image: "ghcr.io/fortress-soft/borealis-connect:latest"
    ports:
      - "8787:8787/tcp"
    restart: "unless-stopped"
    volumes:
      - "/docker/borealis-party/config:/config"
```

Servers set up before the rename to Borealis Connect keep their `borealis-party` service name; the host folder and the
`BOREALIS__*` variables keep their names on every install.

1. Start it and open `http://<server>:8787`. The first account you create becomes the admin.
2. Everything the server keeps is in `/config`. Back up that folder to back up the server.
3. Settings are in `config.toml` there, or in `BOREALIS__<SECTION>__<KEY>` environment variables. For example, set
   `BOREALIS__SERVER__PUBLIC_URL=https://party.example.com` for the email links.
4. On the open internet, put it behind a reverse proxy that terminates HTTPS.

The **[server guide](/party/README.md)** covers the rest: PostgreSQL, email, an install without Docker, upload limits
and **[Security and proxies](/party/README.md#security-and-proxies)** (read it before you put the server on the
internet). Ready-made files are in [`party/docker`](/party/docker).

## Install the apps

The apps are on the **[latest release](https://github.com/fortress-soft/borealis-connect/releases/latest)**. On first launch,
enter your server's address (such as `party.example.com`), then sign in.

| System | File |
|---|---|
| Android | `borealis-party-arm64.apk` |
| iPhone (sideload with AltStore, SideStore or Sideloadly) | `borealis-party-ios-unsigned.ipa` |
| Windows | `borealis-party-windows-x64-setup.exe` |
| Linux | `borealis-party-linux-x86_64.AppImage` or `borealis-party-linux-amd64.deb` |

The apps aren't code-signed, so Windows SmartScreen asks first: choose More info > Run anyway. You don't need the apps
at all: every browser works too.

## Content and licensing

Borealis Connect ships only the System Reference Document content (SRD 5.1 and SRD 5.2.1) that Borealis includes. Any
other content is what each player imports into their own account, for their own use.

### Licence

Like Borealis, Borealis Connect is dual-licensed under **[MIT](/LICENSE-MIT)** OR **[Apache-2.0](/LICENSE-APACHE)**, at
your option. Borealis's copyright and notices are kept unchanged ([NOTICE](/NOTICE)).

## Support

Borealis Connect is a hobby project. If it brings your table together, you can buy me a coffee:

<a href="https://buymeacoffee.com/drazorv"><img src="https://cdn.buymeacoffee.com/buttons/v2/default-yellow.png" alt="Buy Me a Coffee" height="45"></a>

And please support **[Borealis](https://github.com/TeamBorealis/borealis)** too: without it, none of this would exist.
