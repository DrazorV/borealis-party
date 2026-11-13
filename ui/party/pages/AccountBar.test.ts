import { expect, it } from "vitest";
import { compactAccount, syncText } from "./AccountBar";

it("puts the account rows on More on a phone and anywhere in the Android app, wide windows included", () => {
  expect(compactAccount(true, { tauri: false, os: "android" })).toBe(true); // a phone browser
  expect(compactAccount(true, { tauri: true, os: "android" })).toBe(true); // the app on a phone
  expect(compactAccount(false, { tauri: true, os: "android" })).toBe(true); // the app on a tablet or a wide window
  expect(compactAccount(false, { tauri: true, os: null })).toBe(false); // the desktop app
  expect(compactAccount(false, { tauri: false, os: "android" })).toBe(false); // Chrome on an Android tablet
  expect(compactAccount(false, { tauri: false, os: null })).toBe(false); // a desktop browser
});

it("spells the sync state out in full for More", () => {
  expect(syncText({ state: "synced", waiting: 0 } as never)).toBe("Synced");
  expect(syncText({ state: "synced", waiting: 3 } as never)).toBe("Synced; 3 changes made here wait for the server");
  expect(syncText({ state: "syncing", first: true, done: 2, total: 9 } as never)).toBe("Downloading your characters, 2 of 9");
  expect(syncText({ state: "problem", message: "The server can't be reached." } as never)).toBe("Sync problem: The server can't be reached.");
  expect(syncText({ state: "off" } as never)).toBe("");
});
