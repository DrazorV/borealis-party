# Borealis Party server

Borealis Party is Borealis with a multi-user server: Dungeon Master and player accounts, groups, and a party view for the
DM. The server runs the Borealis rules engine and stores everything. Browsers, the Android app and the desktop app are
clients.

## Install on Linux

Build the release on a Linux machine of the same architecture (needs Rust and Node 20+):

```sh
sh build/server/package.sh
```

Copy the tarball to the server, then:

```sh
tar xzf borealis-party-*.tar.gz
cd borealis-party-*/
sudo ./install.sh
```

The service is `borealis-party`. It listens on port 8787 and keeps its data in `/var/lib/borealis-party`. Settings are in
`/etc/borealis-party/server.env`. Back up `/var/lib/borealis-party` to back up everything.

Running `install.sh` from a newer release upgrades in place and leaves the data alone.

## Connect

- **Browser:** open `http://<server>:8787`.
- **Android app:** on first launch, enter `<server>:8787`.

Then sign in or create an account.

On the open internet, put the server behind a reverse proxy that terminates HTTPS, and use `https://your.domain` as the
address. Plain HTTP is only sensible on a network you trust, because passwords and session tokens would cross it
unencrypted.

Set `BOREALIS_OPEN_REGISTRATION=false` if strangers can reach the server, and create accounts with:

```sh
sudo -u borealis-party /opt/borealis-party/borealis-server add-user <name> <dm|player>
```

The password is read from standard input. The command needs the same `BOREALIS_*` settings as the service, or the default
paths.

## How it works

| Rule | Where it is enforced |
|---|---|
| Players own characters; they create, edit and delete them | `crates/borealis-server/src/rpc.rs` |
| One character per group, a character in at most one group | `members` table in `db.rs` |
| A DM sees a member's character and cannot change it | `groups.rs`, `view` |
| Only DMs add content, from their Library | `rpc.rs`, `check_access` |
| Only commands listed in `src-tauri/src/headless.rs` are reachable | `headless.rs` |

Content follows the character's folder. A character outside a group sees the bundled SRD. A character in a group sees the
bundled SRD plus the DM's library, minus the packs the DM turned off for that group. See the layout comment at the top of
`crates/borealis-server/src/workspaces.rs`.

## Keeping up with Borealis

The server compiles the Borealis command code unchanged against `crates/tauri-headless`, a small stand-in for the `tauri`
crate. To take a new upstream release:

```sh
git fetch upstream
git merge upstream/main
```

If upstream adds a command, add it to the right list in `headless.rs` to expose it. New commands stay off the network
until someone does.

## Build the Android app

Needs JDK 17, the Android SDK with `ndk;27.2.12479018`, `platforms;android-37.0`, `build-tools;36.0.0` and `cmake;3.22.1`, and
the Rust target `aarch64-linux-android`. Set `JAVA_HOME`, `ANDROID_HOME`, `NDK_HOME`, `ANDROID_KEYSTORE_PATH` and
`ANDROID_KEYSTORE_PASSWORD`, then:

```sh
npm run tauri -- android build --apk --target aarch64
```

On Windows without Developer Mode that command compiles the library and then stops at creating a symlink. Copy
`target/aarch64-linux-android/release/libborealis_lib.so` to `src-tauri/gen/android/app/src/main/jniLibs/arm64-v8a/`, then
run `gradlew assembleArm64Release -x rustBuildArm64Release` in `src-tauri/gen/android`. A long `CARGO_HOME` path also fails
on Windows, on a deep path in a dependency; a short one such as `C:\c` works.

Keep the signing key: Android only installs an update signed with the same key.
