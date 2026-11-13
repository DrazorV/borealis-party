import type { ContentIndex, ContentPack } from "../../../src/api/contract";
import { errorMessage } from "../../../src/state/useAsync";
import { type Area, contentSync } from "../../content/contentSync";
import { override, type Method, type Methods } from "./registry";

/**
 * Additional Content without the wait (../../content/contentSync.ts). The page's commands, for every caller:
 * - The lists (`listPacks`, `listIndexes`, `list5etoolsUrls`) leave out what is being removed and show pending pack
 *   switches. Calls made while one is on its way share its answer, so a change fetched by several sections is one
 *   request each.
 * - Removing a pack or switching it on or off answers at once: the server's reload of the content goes on in the
 *   background, with an activity row. A refusal brings the pack back and keeps the message in that row.
 *   (Removing an index file or a 5etools address: ./sourceRemoval.ts, after its question.)
 * - Additions and downloads (an index file, an update, an import, a bundle) get an activity row with progress, and
 *   the lists are fetched again when they end.
 * Applied last (../http.ts), so these wrap the jobs and the source-removal question.
 */

const hostOf = (url: string) => { try { return new URL(url).host || url; } catch { return url; } };

/** The last lists the server sent, for names in activity rows and for the answers given at once. */
export const lastLists: { packs: ContentPack[]; indexes: ContentIndex[]; fiveTools: { url: string }[] } = { packs: [], indexes: [], fiveTools: [] };

/**
 * Share one answer among calls made at the same content version. Right after a removal or a switch (see
 * `contentSync.takeCached`) the next call is answered from the last list, with no request.
 */
function shared<T>(kind: keyof typeof lastLists, plain: Method, show: (list: T[]) => T[]): Method {
  let flight: { version: number; promise: Promise<T[]> } | null = null;
  let loaded = false;
  return () => {
    if (loaded && contentSync.takeCached(kind)) return Promise.resolve(show(lastLists[kind] as T[]));
    const version = contentSync.get().version;
    if (!flight || flight.version !== version) {
      const promise = Promise.resolve(plain()).then((list) => {
        const all = (Array.isArray(list) ? list : []) as T[];
        (lastLists as Record<string, unknown>)[kind] = all;
        loaded = true;
        return all;
      });
      const mine = { version, promise };
      flight = mine;
      // Settled answers are not reused: the next call asks again (an upstream reload after its own action).
      void promise.then(() => { if (flight === mine) flight = null; }, () => { if (flight === mine) flight = null; });
    }
    return flight.promise.then(show);
  };
}

/** Run `work` with an activity row; the lists are fetched again when it ends. Its error is the caller's to show. */
async function tracked<T>(area: Area, label: string, work: () => Promise<T>, progress = true): Promise<T> {
  contentSync.adding();
  const op = contentSync.begin(area, label, {}, progress);
  try {
    const out = await work();
    contentSync.done(op);
    return out;
  } catch (e) {
    contentSync.dismiss(op);
    contentSync.changed(true);
    throw e;
  }
}

export function applyContentRefresh(methods: Methods): void {
  methods.listPacks = shared<ContentPack>("packs", methods.listPacks, (l) => contentSync.packs(l));
  methods.listIndexes = shared<ContentIndex>("indexes", methods.listIndexes, (l) => contentSync.indexes(l));
  methods.list5etoolsUrls = shared<{ url: string }>("fiveTools", methods.list5etoolsUrls, (l) => contentSync.fiveTools(l));

  const removePack = methods.removePack;
  methods.removePack = (args?: Record<string, unknown>) => {
    const id = String(args?.packId ?? "");
    const name = lastLists.packs.find((p) => p.id === id)?.name ?? id;
    const op = contentSync.begin("packs", `Removing “${name}”…`, { packs: [id] });
    void Promise.resolve(removePack(args)).then(
      () => contentSync.done(op, `Removed “${name}”.`),
      (e) => contentSync.fail(op, `“${name}” wasn't removed: ${errorMessage(e)}`),
    );
    return Promise.resolve(contentSync.packs(lastLists.packs));
  };

  const setPackEnabled = methods.setPackEnabled;
  methods.setPackEnabled = (args?: Record<string, unknown>) => {
    const id = String(args?.packId ?? "");
    const enabled = args?.enabled === true;
    const name = lastLists.packs.find((p) => p.id === id)?.name ?? id;
    const op = contentSync.begin("packs", `${enabled ? "Turning on" : "Turning off"} “${name}”…`);
    contentSync.switching(id, enabled, op);
    void Promise.resolve(setPackEnabled(args)).then(
      () => contentSync.done(op),
      (e) => contentSync.fail(op, `“${name}” wasn't turned ${enabled ? "on" : "off"}: ${errorMessage(e)}`),
    );
    return Promise.resolve(contentSync.packs(lastLists.packs));
  };

  const addIndex = methods.addIndex;
  methods.addIndex = (args?: Record<string, unknown>) =>
    tracked("indexes", `Adding the index file from ${hostOf(String(args?.url ?? ""))}…`, async () => {
      const out = await addIndex(args);
      // Shown by every Index files list, upstream's included, even while an update runs.
      contentSync.changed(true);
      return out;
    }, false);

  const updateIndexes = methods.updateIndexes;
  methods.updateIndexes = (args?: Record<string, unknown>) => {
    const end = contentSync.updating();
    return tracked("indexes", "Downloading index files…", () => Promise.resolve(updateIndexes(args))).finally(end);
  };

  const adds: [keyof Methods, Area, string][] = [
    ["importPack", "packs", "Adding content…"],
    ["installImportPreview", "packs", "Adding content…"],
    ["import5etools", "packs", "Adding 5etools content…"],
    ["update5etoolsUrl", "fivetools", "Updating 5etools content…"],
    ["bundleAccept", "packs", "Adding the bundle's content…"],
    ["clearDownloadedContent", "packs", "Removing downloaded content…"],
  ];
  for (const [name, area, label] of adds) {
    const plain = methods[name];
    if (!plain) continue;
    methods[name] = (args?: Record<string, unknown>) => tracked(area, label, () => Promise.resolve(plain(args)));
  }
}

override("contentRefresh", applyContentRefresh);
