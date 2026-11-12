import { override } from "./registry";

/**
 * The version the app shows (Settings → Application): the party version, as the status bar shows it (`__APP_VERSION__`,
 * which ../../vite.config.ts sets from party/Cargo.toml). The server's `app_version` stays upstream's, for the bundles
 * plain Borealis reads. Upstream asks `appVersion` for its own update check too, which the party apps don't use
 * (`checkAppUpdate` and `appUpdateStatus` are not available there, ../http.ts); the desktop updater compares the
 * installed app's own version (../desktopUpdate.ts).
 */
export const shownVersion = (): string => __APP_VERSION__;

override("version", (methods) => {
  methods.appVersion = async () => shownVersion();
});
