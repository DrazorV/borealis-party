import type { CommandName } from "../../../src/api/contract";
import { phoneOs } from "../../../src/lib/phone";
import { unavailable } from "../http";
import { override, type Methods } from "./registry";

/** Not offered on Android; elsewhere `name` works as the earlier overrides made it. */
function notOnAndroid(methods: Methods, name: CommandName): void {
  const elsewhere = methods[name];
  methods[name] = async (args?: Record<string, unknown>) => {
    if (phoneOs() === "android") throw unavailable();
    return await elsewhere(args);
  };
}

// Android's web view has no share sheet and no download, and upstream shares from the device's backend there, which
// has no character to share. This wraps ./sheet.ts and ./bundles.ts, so ../http.ts imports it after them.
override("android", (methods) => {
  notOnAndroid(methods, "shareSheet");
  notOnAndroid(methods, "bundleShare");
});
