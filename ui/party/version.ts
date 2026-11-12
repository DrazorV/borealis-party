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
