<div align="center">

<img src="/ui/party/public/favicon.svg" alt="Borealis Party" width="96">

# Borealis Party

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

Borealis Party is an add-on to **[Borealis](https://github.com/TeamBorealis/borealis)**, the free and open-source
character builder by **[TeamBorealis](https://github.com/TeamBorealis)**. The character builder, the rules and content
engine, the character sheets and the importers are all their work. This fork adds only the multi-user part on top: the
server, accounts and the party features.

If you like Borealis Party, you like Borealis. Please give the
**[original repository a star](https://github.com/TeamBorealis/borealis)**, try the
**[desktop and phone apps](https://borealisbuilder.com)**, and come and say hi on the
**[Borealis Discord](https://discord.gg/r4K4D3AJpZ)**.

The two projects work together. Borealis Party stays a separate port, by agreement with the Borealis team, and the two
coordinate releases: every Borealis Party release is built on a Borealis release, and fixes that belong in Borealis go
to them as pull requests.

## What it adds

Borealis keeps everything on one device. Borealis Party puts the same app on a server you run, so a whole group can use
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
  5etools-format data you supply, all in one place.
- **Share codes.** Send characters and content packs to another player with a one-time code, Android included.
- **Parties.** Switch on "Dungeon Master tools" on your Account page to run parties: invite players by username or with
  a join code and add characters. The **live party roster** updates as characters are played: the Dungeon Master sees
  hit points, AC, concentration and who is online, and can open any member's sheet read-only.

**In progress** (not released yet): a bestiary, an encounter builder and a combat tracker.

## Run your own server

With Docker (or a Portainer stack), using SQLite:

```yaml
services:
  borealis-party:
    image: "ghcr.io/fortress-soft/borealis-connect:latest"
    ports:
      - "8787:8787/tcp"
    restart: "unless-stopped"
    volumes:
      - "/docker/borealis-party/config:/config"
```

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

Borealis Party ships only the System Reference Document content (SRD 5.1 and SRD 5.2.1) that Borealis includes. Any
other content is what each player imports into their own account, for their own use.

### Licence

Like Borealis, Borealis Party is dual-licensed under **[MIT](/LICENSE-MIT)** OR **[Apache-2.0](/LICENSE-APACHE)**, at
your option. Borealis's copyright and notices are kept unchanged ([NOTICE](/NOTICE)).

## Support

Borealis Party is a hobby project. If it brings your table together, you can buy me a coffee:

<a href="https://buymeacoffee.com/drazorv"><img src="https://cdn.buymeacoffee.com/buttons/v2/default-yellow.png" alt="Buy Me a Coffee" height="45"></a>

And please support **[Borealis](https://github.com/TeamBorealis/borealis)** too: without it, none of this would exist.
