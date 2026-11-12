import { describe, expect, it, vi } from "vitest";
import { emptyMirror, type Mirror } from "../sync/mirror";
import { NOT_ON_DEVICE_TITLE, sheetOnDevice, UNSAVED_TITLE, type DeviceSheetDeps } from "./androidSheet";

/** Server id S1 has the device copy L1; S2 has none. */
function setup(opts: { dirty?: boolean; snapshotFails?: boolean; mirror?: Mirror | null; notOpenOnce?: boolean; exportFails?: Error } = {}) {
  const log: string[] = [];
  const mirror = opts.mirror !== undefined ? opts.mirror : { ...emptyMirror("u"), lastSyncMs: 1, characters: { S1: { local: "L1", serverSha: "a", localSha: "b", localMtime: "t" } } };
  let notOpen = opts.notOpenOnce ?? false;
  const deps: DeviceSheetDeps = {
    sync: vi.fn(async () => { log.push("sync"); }),
    mirror: vi.fn(() => mirror),
    notice: vi.fn(async (title: string) => { log.push(`notice ${title}`); }),
    local: () => ({
      loadCharacter: vi.fn(async (a: { characterId: string }) => { log.push(`load ${a.characterId}`); return {} as never; }),
      closeCharacter: vi.fn(async (a: { characterId: string; discard: boolean }) => { log.push(`close ${a.characterId} ${a.discard}`); }),
      exportSheet: vi.fn(async (a: { characterId: string }) => {
        log.push(`exportSheet ${JSON.stringify(a)}`);
        if (opts.exportFails) throw opts.exportFails;
        if (notOpen) { notOpen = false; throw { code: "not_found", message: "That character is not open." }; }
        return { fileName: "Hero.pdf", mime: "application/pdf", path: "content://x/Hero.pdf", dataBase64: null };
      }),
      shareSheet: vi.fn(async (a: { characterId: string }) => {
        log.push(`shareSheet ${a.characterId}`);
        return { fileName: "Hero.pdf", mime: "application/pdf", shared: true, dataBase64: null };
      }),
    }) as never,
  };
  const snapshot = vi.fn(async () => {
    if (opts.snapshotFails) throw { code: "not_found", message: "not open" };
    return { name: "Hero", dirty: opts.dirty ?? false };
  });
  return { log, deps, snapshot };
}

describe("the sheet PDF on Android, from the device's copy", () => {
  it("Save: syncs, then the device saves the copy's sheet with the same options, and closes the copy again", async () => {
    const { log, deps, snapshot } = setup();
    const out = await sheetOnDevice("exportSheet", { characterId: "S1", options: { format: "pdf", defaultPaper: "a4" } }, "u", snapshot, deps);
    expect(out).toMatchObject({ path: "content://x/Hero.pdf" });
    expect(deps.mirror).toHaveBeenCalledWith("u");
    expect(log).toEqual(["sync", "load L1", 'exportSheet {"characterId":"L1","options":{"format":"pdf","defaultPaper":"a4"}}', "close L1 false"]);
  });

  it("Share: the device's share sheet", async () => {
    const { log, deps, snapshot } = setup();
    await expect(sheetOnDevice("shareSheet", { characterId: "S1" }, "u", snapshot, deps)).resolves.toMatchObject({ shared: true });
    expect(log).toEqual(["sync", "load L1", "shareSheet L1", "close L1 false"]);
  });

  it("unsaved changes on the server: a dialog, and nothing is made", async () => {
    const { log, deps, snapshot } = setup({ dirty: true });
    await expect(sheetOnDevice("exportSheet", { characterId: "S1" }, "u", snapshot, deps)).rejects.toMatchObject({ message: "The sheet was not saved." });
    expect(log).toEqual([`notice ${UNSAVED_TITLE}`]);
  });

  it.each<[string, Mirror | null]>([
    ["no copy of the account yet", null],
    ["no copy of this character", { ...emptyMirror("u"), lastSyncMs: 1 }],
    ["a copy the last sync could not update", { ...emptyMirror("u"), lastSyncMs: 1, characters: { S1: { local: "L1", serverSha: "a", localSha: "b", localMtime: "t" } }, problems: { S1: "A character could not be copied." } }],
  ])("%s: a dialog after the sync, and nothing is made", async (_, mirror) => {
    const { log, deps, snapshot } = setup({ mirror });
    await expect(sheetOnDevice("shareSheet", { characterId: "S1" }, "u", snapshot, deps)).rejects.toMatchObject({ message: "The sheet was not shared." });
    expect(log).toEqual(["sync", `notice ${NOT_ON_DEVICE_TITLE}`]);
  });

  it("the server not answering the snapshot does not stop it", async () => {
    const { log, deps, snapshot } = setup({ snapshotFails: true });
    await sheetOnDevice("exportSheet", { characterId: "S1" }, "u", snapshot, deps);
    expect(log[0]).toBe("sync");
    expect(log).toContain("close L1 false");
  });

  it("a sync that closed the copy meanwhile: tried once more", async () => {
    const { log, deps, snapshot } = setup({ notOpenOnce: true });
    await expect(sheetOnDevice("exportSheet", { characterId: "S1" }, "u", snapshot, deps)).resolves.toMatchObject({ path: "content://x/Hero.pdf" });
    expect(log.filter((l) => l.startsWith("load"))).toEqual(["load L1", "load L1"]);
  });

  it("a device error comes through, and the copy is closed anyway", async () => {
    const { log, deps, snapshot } = setup({ exportFails: new Error("no PDF") });
    await expect(sheetOnDevice("exportSheet", { characterId: "S1" }, "u", snapshot, deps)).rejects.toThrow("no PDF");
    expect(log.at(-1)).toBe("close L1 false");
  });
});
