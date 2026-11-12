import { COMMANDS, type Api, type ApiError, type ApiMethods, type CharacterSummary, type CommandName } from "../../src/api/contract";
import { LOCAL, unavailable } from "./http";

/** What an action that needs the server says offline. */
export const OFFLINE_MESSAGE = "Needs the server. Connect and try again.";

export const offlineError = (): ApiError => ({ code: "offline", message: OFFLINE_MESSAGE }) as unknown as ApiError;

/**
 * Refused offline: anything that makes or removes a character, every content change, settings that point at the
 * device's folders, and files handed in from outside. The app's copy only takes edits of characters it already has,
 * so a sync never has to send a new or deleted character.
 */
const DENIED = new Set<CommandName>([
  "createCharacter", "saveCharacterAs", "deleteCharacter", "duplicateCharacter", "importCharacter",
  "previewConvertTo2024", "applyConvertTo2024",
  "importPack", "previewImportPack", "installImportPreview", "cancelImportPreview", "setPackEnabled", "removePack", "setSourceEdition",
  "addIndex", "removeIndex", "updateIndexes", "clearDownloadedContent",
  "scan5etoolsFolder", "stage5etoolsZip", "stage5etoolsFiles", "stage5etoolsUrl", "remove5etoolsUrl", "update5etoolsUrl", "import5etools",
  "pickPath", "setAiOverride", "setDocumentsDir", "setPortraitLibraryDir", "setVaultDir", "setSecondaryContentDir", "unlinkSecondaryContentDir",
  "restoreDefaultSettings", "installAppUpdate",
  "bundleExport", "bundleShare", "bundleOpen", "bundleAccept", "bundleDiscard",
]);

/** What the server does not offer either (device transfer, AI, the content import tool, notes, semantic search): the same answer as online. */
const UNOFFERED_PREFIXES = ["link", "ai", "import", "semantic", "vault"];

function unoffered(name: CommandName): boolean {
  return UNOFFERED_PREFIXES.some((p) => name.startsWith(p) && /[A-Z0-9]/.test(name.charAt(p.length)));
}

/** Settings a device keeps for itself (sync.ts copies the rest from the server). */
const DEVICE_SETTINGS = ["documentsDir", "secondaryContentDir", "portraitLibraryDir", "vaultDir"];

/** Every string equal to a key of `map`, replaced by its value, in any nesting of arrays and plain objects. */
export function swapIds<T>(value: T, map: ReadonlyMap<string, string>): T {
  if (map.size === 0) return value;
  const walk = (v: unknown): unknown => {
    if (typeof v === "string") return map.get(v) ?? v;
    if (Array.isArray(v)) return v.map(walk);
    if (v && typeof v === "object" && Object.getPrototypeOf(v) === Object.prototype) {
      return Object.fromEntries(Object.entries(v).map(([k, x]) => [k, walk(x)]));
    }
    return v;
  };
  return walk(value) as T;
}

export interface OfflineApi extends Api {
  /** Server ids of the characters open on this page (loaded and not closed). */
  openCharacters(): string[];
  /** Save and close what is open, before the page goes back to the server. */
  closeAll(): Promise<void>;
}

/**
 * The app's own backend for a page with no server. Characters keep their server ids S here: the copies' own ids L
 * (see sync/mirror.ts) are swapped in on the way in and out, so the screens, the address bar and sessionStorage only
 * ever see S. Copies that are in no entry of the map (a spare of a stopped sync) are not listed.
 */
export function createOfflineApi(local: Api, serverToLocal: ReadonlyMap<string, string>): OfflineApi {
  const localToServer = new Map([...serverToLocal].map(([s, l]) => [l, s]));
  const open = new Set<string>();
  const methods = {} as Record<string, unknown>;
  for (const name of Object.keys(COMMANDS) as CommandName[]) {
    methods[name] = async (args?: Record<string, unknown>) => {
      if (DENIED.has(name)) throw offlineError();
      if (unoffered(name)) throw unavailable();
      const answer = LOCAL[name];
      if (answer) return answer(args ?? {});
      const call = local[name] as (a?: Record<string, unknown>) => Promise<unknown>;
      const result = swapIds(await call(swapIds(args, serverToLocal)), localToServer);
      const id = typeof args?.characterId === "string" ? args.characterId : null;
      if (id && (name === "loadCharacter" || name === "getSnapshot")) open.add(id);
      if (id && name === "closeCharacter") open.delete(id);
      return result;
    };
  }
  // Upstream discards an open character only before deleting it or for a bundle, both refused here: an edit that
  // was not on file yet is saved instead of lost to a refused delete.
  const close = methods.closeCharacter as (a: { characterId: string; discard: boolean }) => Promise<void>;
  methods.closeCharacter = (args: { characterId: string; discard: boolean }) => close({ ...args, discard: false });
  const list = methods.listCharacters as () => Promise<CharacterSummary[]>;
  methods.listCharacters = async () => (await list()).filter((c) => localToServer.has(c.id) || serverToLocal.has(c.id));
  const save = methods.saveSettings as (a: { patch: Record<string, unknown> }) => Promise<unknown>;
  methods.saveSettings = (args: { patch: Record<string, unknown> }) => {
    const patch = { ...(args?.patch ?? {}) };
    for (const k of DEVICE_SETTINGS) delete patch[k];
    return save({ ...args, patch });
  };

  return {
    ...(methods as unknown as ApiMethods),
    kind: "tauri",
    subscribe(event, handler) {
      return local.subscribe(event, (payload) => handler(swapIds(payload, localToServer)));
    },
    openCharacters: () => [...open],
    async closeAll() {
      for (const id of [...open]) {
        await local.closeCharacter({ characterId: serverToLocal.get(id) ?? id, discard: false });
        open.delete(id);
      }
    },
  } as OfflineApi;
}
