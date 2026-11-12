import type { CommandName } from "../../../src/api/contract";
import { phoneOs } from "../../../src/lib/phone";
import { isTauri } from "../../../src/lib/tauri";
import { sheetOnDevice, type SheetCommand } from "../../export/androidSheet";
import { unavailable } from "../http";
import { override, type Methods } from "./registry";

/** The Android app (not Chrome on Android, which reports the same phoneOs()). */
const inAndroidApp = (): boolean => phoneOs() === "android" && isTauri();

/** Not offered in the Android app; elsewhere `name` works as the earlier overrides made it. */
function notOnAndroid(methods: Methods, name: CommandName): void {
  const elsewhere = methods[name];
  methods[name] = async (args?: Record<string, unknown>) => {
    if (inAndroidApp()) throw unavailable();
    return await elsewhere(args);
  };
}

/** In the Android app, from the device's copy of the character (../../export/androidSheet.ts); elsewhere as before. */
function sheetOnAndroid(methods: Methods, name: SheetCommand, userId: string): void {
  const elsewhere = methods[name];
  methods[name] = async (args?: Record<string, unknown>) => {
    if (!inAndroidApp()) return await elsewhere(args);
    return await sheetOnDevice(name, args, userId, (a) => methods.getSnapshot(a));
  };
}

// Android's web view has no share sheet and no download. The sheet PDF goes through the device's own save picker and
// share sheet instead (E1a). Bundles can't: the app is built without device-link, so its backend has no bundle command;
// Android will send bundles through the server (share codes), and entry/hideTransfer.ts hides their entries there.
// This wraps ./sheet.ts and ./bundles.ts, so ../http.ts imports it after them.
override("android", (methods, session) => {
  sheetOnAndroid(methods, "exportSheet", session.user.id);
  sheetOnAndroid(methods, "shareSheet", session.user.id);
  notOnAndroid(methods, "bundleShare");
  notOnAndroid(methods, "bundleExport");
});
