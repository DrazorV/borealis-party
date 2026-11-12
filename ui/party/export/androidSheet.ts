import type { Api, ApiError, CharacterSnapshot } from "../../src/api/contract";
import { createTauriApi } from "../../src/api/tauri";
import { swapIds } from "../api/offline";
import { loadMirror, type Mirror } from "../sync/mirror";
import { requestSync } from "../sync/runner";
import { showNotice } from "./notice";

/**
 * E1a: the sheet PDF on Android. The web view there can neither download nor share bytes, so Save and Share go through
 * the device's own `export_sheet` / `share_sheet` (the system save picker and share sheet), on the device's copy of the
 * character. These two commands are in the app's backend whatever its features; the bundle commands are not.
 */

export type SheetCommand = "exportSheet" | "shareSheet";

type LocalSheetApi = Pick<Api, "loadCharacter" | "closeCharacter" | "exportSheet" | "shareSheet">;

export interface DeviceSheetDeps {
  /** One sync now, so the copy has what the server has. */
  sync: () => Promise<void>;
  mirror: (userId: string) => Mirror | null;
  local: () => LocalSheetApi;
  notice: (title: string, message: string) => Promise<void>;
}

// Looked up when called, so a test that mocks one of these modules need not provide them.
const DEFAULT_DEPS: DeviceSheetDeps = {
  sync: () => requestSync(),
  mirror: (userId) => loadMirror(userId),
  local: () => createTauriApi(),
  notice: (title, message) => showNotice(title, message),
};

export const UNSAVED_TITLE = "Save the character first";
export const NOT_ON_DEVICE_TITLE = "Not on this device yet";

const WHAT: Record<SheetCommand, string> = { exportSheet: "saved", shareSheet: "shared" };

/** After the dialog, the page's own error line says nothing happened (a toast). */
const notDone = (command: SheetCommand): ApiError =>
  ({ code: "cancelled", message: `The sheet was not ${WHAT[command]}.` }) as unknown as ApiError;

const isNotDone = (e: unknown) => (e as { code?: string } | null)?.code === "cancelled";

const notOpen =(e: unknown) => (e as Partial<ApiError>)?.code === "not_found";

/**
 * Runs `command` on the device for the server's character `args.characterId`. `serverSnapshot` reads the server's
 * open character: unsaved changes there are not in any file yet, so the device's copy can't have them.
 */
export async function sheetOnDevice(
  command: SheetCommand,
  args: Record<string, unknown> | undefined,
  userId: string,
  serverSnapshot: (args: { characterId: string }) => unknown,
  deps: DeviceSheetDeps = DEFAULT_DEPS,
): Promise<unknown> {
  const id = String(args?.characterId ?? "");
  let name = "This character";
  try {
    const snap = await serverSnapshot({ characterId: id }) as Partial<CharacterSnapshot> | undefined;
    if (snap?.name) name = snap.name;
    if (snap?.dirty) {
      await deps.notice(UNSAVED_TITLE, `${name} has changes that are not saved yet. Save the character, then try again, so the sheet shows them.`);
      throw notDone(command);
    }
  } catch (e) {
    if (isNotDone(e)) throw e;
    // Not open on the server (or no answer): the sync below still says whether the copy is here.
  }

  await deps.sync();
  const mirror = deps.mirror(userId);
  const copy = mirror?.characters[id];
  if (!mirror || !copy || mirror.problems[id]) {
    await deps.notice(NOT_ON_DEVICE_TITLE, `${name} is not on this device yet, so the sheet can't be made here. Wait for the account bar to say Synced, then try again.`);
    throw notDone(command);
  }

  const local = deps.local();
  const ids = new Map(Object.entries(mirror.characters).map(([s, m]) => [s, m.local]));
  const deviceArgs = swapIds({ ...(args ?? {}), characterId: id }, ids);
  const run = async () => {
    await local.loadCharacter({ characterId: copy.local });
    try {
      return await (local[command] as (a: Record<string, unknown>) => Promise<unknown>)(deviceArgs);
    } finally {
      await local.closeCharacter({ characterId: copy.local, discard: false }).catch(() => undefined);
    }
  };
  try {
    return await run();
  } catch (e) {
    // A sync that started meanwhile closes every copy before it reads them: once more.
    if (!notOpen(e)) throw e;
    return await run();
  }
}
