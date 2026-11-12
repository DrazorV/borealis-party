import { COMMANDS, EVENTS, type Api, type ApiError, type ApiMethods, type CommandName, type DataDirs, type EventName } from "../../src/api/contract";
import type { ExportResult, SheetShare } from "../../src/api/contract";
import { phoneOs } from "../../src/lib/phone";
import { looksLikeBundle, openBundleFromImport, pickBundleFile } from "./bundles";
import { clearSession, loadSession, saveSession, type ServerSession } from "./session";

function fail(code: string, message: string): ApiError {
  return { code, message } as ApiError;
}

/**
 * Who hears about a request that could not reach the server, and about a token the server refused (connectivity.ts).
 * `expired` answers true when it took care of the refusal; otherwise the device is signed out.
 */
export const serverWatch: { network?: () => void; expired?: () => boolean } = {};

/** JSON request to the server. A rejected token signs the device out, unless the app keeps playing offline. */
export async function serverFetch(session: Pick<ServerSession, "baseUrl" | "token">, method: string, path: string, body?: unknown): Promise<unknown> {
  let response: Response;
  try {
    response = await fetch(`${session.baseUrl}${path}`, {
      method,
      headers: { "content-type": "application/json", ...(session.token ? { authorization: `Bearer ${session.token}` } : {}) },
      ...(body === undefined ? {} : { body: JSON.stringify(body) }),
    });
  } catch {
    serverWatch.network?.();
    throw fail("network", "The server can't be reached. Check your connection.");
  }
  const text = await response.text();
  let parsed: unknown = null;
  if (text) {
    try { parsed = JSON.parse(text); } catch { /* an empty or plain reply */ }
  }
  if (response.ok) return parsed;
  if (response.status === 401 && session.token && !serverWatch.expired?.()) {
    clearSession();
    window.location.reload();
  }
  const error = parsed as Partial<ApiError> | null;
  if (String(error?.code) === "password_change_required" && session.token) {
    // An admin asked for a new password: the app shows that screen until it is chosen.
    const current = loadSession();
    if (current && !current.user.mustChangePassword) {
      saveSession({ ...current, user: { ...current.user, mustChangePassword: true } });
      window.location.reload();
    }
  }
  throw fail(String(error?.code ?? "internal"), String(error?.message ?? `The server answered ${response.status}.`));
}

type Local = (args: Record<string, unknown>) => unknown;

/** What a device answers itself: the server has no OS "open with", update feed, semantic model or desktop folders. */
export const LOCAL: Partial<Record<CommandName, Local>> = {
  takeOpenedFiles: () => [],
  takeOpenedBundles: () => [],
  takePickedFiles: () => [],
  openLink: ({ url }) => { window.open(String(url), "_blank", "noopener"); },
  // The settings window reads these to show the folders, which are the server's own and not shown.
  getDataDirs: (): DataDirs => ({
    documentsDir: "", defaultDocumentsDir: "", isDefault: true, charactersDir: "", documentsDirUnavailable: null, secondaryContentDir: null,
    secondaryContentUnavailable: false, portraitLibraryDir: null, portraitLibraryUnavailable: false, vaultDir: null, vaultLabel: null, vaultUnavailable: false,
  }),
  appUpdateStatus: () => { throw unavailable(); },
  checkAppUpdate: () => { throw unavailable(); },
  semanticStatus: () => { throw unavailable(); },
  listPortraitLibrary: () => { throw unavailable(); },
};

export const unavailable = (): ApiError => fail("not_available", "That isn't available when connected to a server.");

export interface ServerEvent {
  event: string;
  payload: unknown;
}

/** Server-sent events over `fetch`, since EventSource can't send the Authorization header. Reconnects until closed. */
export function eventStream(session: ServerSession, onEvent: (e: ServerEvent) => void): () => void {
  const abort = new AbortController();
  void (async () => {
    while (!abort.signal.aborted) {
      try {
        const response = await fetch(`${session.baseUrl}/api/events`, { headers: { authorization: `Bearer ${session.token}` }, signal: abort.signal });
        if (!response.ok || !response.body) throw new Error("events refused");
        const reader = response.body.pipeThrough(new TextDecoderStream()).getReader();
        let buffer = "";
        for (;;) {
          const { done, value } = await reader.read();
          if (done) break;
          buffer += value;
          let end = buffer.indexOf("\n\n");
          while (end >= 0) {
            const data = buffer.slice(0, end).split("\n").filter((l) => l.startsWith("data:")).map((l) => l.slice(5).trim()).join("\n");
            buffer = buffer.slice(end + 2);
            if (data) onEvent(JSON.parse(data) as ServerEvent);
            end = buffer.indexOf("\n\n");
          }
        }
      } catch { /* dropped or refused: try again below */ }
      if (!abort.signal.aborted) await new Promise((r) => setTimeout(r, 3000));
    }
  })();
  return () => abort.abort();
}

/** The Borealis API over HTTP. Pages use this exactly as they use the on-device backend. */
export function createHttpApi(session: ServerSession): Api {
  const methods = {} as Record<string, unknown>;
  for (const name of Object.keys(COMMANDS) as CommandName[]) {
    methods[name] = async (args?: Record<string, unknown>) => {
      const local = LOCAL[name];
      if (local) return local(args ?? {});
      try {
        return await serverFetch(session, "POST", `/api/rpc/${COMMANDS[name]}`, args ?? {});
      } catch (e) {
        // "The server does not offer that" reads better as a statement about the feature.
        if ((e as ApiError).code === "not_found" && (e as ApiError).message === "The server does not offer that.") throw unavailable();
        throw e;
      }
    };
  }

  // Phones share the sheet PDF from the web view's own share sheet, with the bytes the server made. Android's web view
  // has no share sheet and upstream shares from the device's backend there, which has no character to share.
  methods.shareSheet = async (args?: Record<string, unknown>): Promise<SheetShare> => {
    if (phoneOs() === "android") throw unavailable();
    const file = await serverFetch(session, "POST", `/api/rpc/${COMMANDS.exportSheet}`, args ?? {}) as ExportResult;
    return { fileName: file.fileName, mime: file.mime, shared: false, dataBase64: file.dataBase64 ?? "" };
  };

  // File > Open bundle...: the browser's file chooser and an upload stand in for the native open panel (./bundles.ts).
  // Every other pick still goes to the server, which does not offer it.
  const serverPick = methods.pickPath as (args?: Record<string, unknown>) => Promise<unknown>;
  methods.pickPath = (args?: Record<string, unknown>) => (args?.purpose === "bundle-file" ? pickBundleFile(session) : serverPick(args));

  // Start page > Import characters with a .borealis file (or a zip under another name): the Open bundle window instead.
  const serverImport = methods.importCharacter as (args?: Record<string, unknown>) => Promise<unknown>;
  methods.importCharacter = (args?: Record<string, unknown>) => {
    const fileName = String(args?.fileName ?? "");
    const content = String(args?.contentBase64 ?? "");
    return looksLikeBundle(fileName, content) ? openBundleFromImport(session, fileName, content) : serverImport(args);
  };

  const listeners = new Map<string, Set<(payload: never) => void>>();
  let close: (() => void) | null = null;
  const dispatch = (e: ServerEvent) => listeners.get(e.event)?.forEach((h) => h(e.payload as never));

  return {
    ...(methods as unknown as ApiMethods),
    // The contract only knows these two; to the pages this is a real backend.
    kind: "tauri",
    subscribe(event: EventName, handler) {
      const name = EVENTS[event];
      const set = listeners.get(name) ?? new Set();
      listeners.set(name, set);
      set.add(handler as (payload: never) => void);
      close ??= eventStream(session, dispatch);
      return () => {
        set.delete(handler as (payload: never) => void);
        if ([...listeners.values()].every((s) => s.size === 0)) {
          close?.();
          close = null;
        }
      };
    },
  } as Api;
}
