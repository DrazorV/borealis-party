/**
 * The party's version, which the party apps show (the status bar, Settings → Application, the splash): the first
 * `version = "…"` line of party/Cargo.toml, as party/desktop/build-desktop.ps1 and the release workflow read it.
 * ./vite.config.ts puts it in `__APP_VERSION__` in place of upstream's (ui/package.json). What the server and the
 * bundles report as `app_version` stays upstream's, which plain Borealis compares with.
 */
export function cargoVersion(toml: string): string {
  const version = /^version = "([^"]+)"/m.exec(toml)?.[1];
  if (!version) throw new Error("party/Cargo.toml has no version line");
  return version;
}

/**
 * The release the build shows: party/VERSION, checked against party/Cargo.toml as party/release-version.sh checks it.
 * `X.Y.Z` is a full release (the apps too); `X.Y.Z.W` a server and web release, whose X.Y.Z is the Cargo version the
 * apps keep. The apps are only built from full releases, so in an app the two are the same.
 */
export function releaseVersion(file: string, toml: string): string {
  const apps = cargoVersion(toml);
  const release = (file.split(/\r?\n/)[0] ?? "").trim();
  if (release === apps) return release;
  if (!release.startsWith(`${apps}.`)) {
    throw new Error(`party/VERSION is ${release}, but party/Cargo.toml is ${apps}: VERSION must be ${apps} or ${apps}.N`);
  }
  if (!/^[1-9][0-9]*$/.test(release.slice(apps.length + 1))) {
    throw new Error(`party/VERSION is ${release}: the 4th number must be 1 or more, without leading zeros`);
  }
  return release;
}
