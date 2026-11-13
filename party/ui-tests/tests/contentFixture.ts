/**
 * A large made-up library for content-compact.spec.ts: about 120 sources, as a 5etools import of a whole data folder
 * would bring, all with invented names (no real book is named; the two SRD bestiaries are the app's own).
 */

const OPENERS = ["Atlas of", "Beasts of", "Chronicle of", "Masks of", "Ruins of", "Secrets of", "Shadows over", "Songs of", "Tides of", "Wardens of"];
const PLACES = ["Ashvale", "Brindlewood", "Dunmarrow", "Emberfall", "Frostwhistle", "Glimmerdeep", "Hollowpine", "Lanternmoor", "Mossgate", "Quillhaven", "Saltreach", "Thornmere"];

/** 120 made-up titles, "Atlas of Ashvale" to "Wardens of Thornmere". */
export const TITLES = OPENERS.flatMap((o) => PLACES.map((p) => `${o} ${p}`));

export const SOURCE_URL = "https://example.com/sample-5etools-data/";

const NO_EXTRAS = { environment: 0, page: 0, mythic: 0, covenCr: 0, altSpeeds: 0, variants: 0, reactionIntro: 0, fluff: 0, tokens: 0 };

function coveragePack(i: number, title: string) {
  const monsters = 1 + ((i * 37) % 300);
  const partial = i % 4 === 0 ? 1 + (i % 9) : 0;
  const skipped = i % 15 === 0 ? 1 : 0;
  const status = partial > 0 || skipped > 0 ? "partial" : "complete";
  return {
    packId: `made-up-${i}`, name: `5etools-format: ${title}`, kind: "5etools", enabled: true, packRev: "r1", status,
    monsters, companions: 0, inData: monsters + skipped, skipped, partial,
    skippedReasons: skipped ? [{ value: "the creature type is a choice", count: skipped }] : [],
    partialNotes: partial ? [{ value: "alternate speeds", count: partial }] : [],
    sources: [], byCr: [{ value: "1", count: monsters }], byType: [{ value: "beast", count: monsters }], rulesets: [{ value: "dnd2014", count: monsters }],
    extras: { ...NO_EXTRAS, environment: monsters }, captured: true,
  };
}

function srd(id: string, name: string, monsters: number) {
  return {
    packId: id, name, kind: "bundled", enabled: true, packRev: "r1", status: "complete", monsters, companions: 0, inData: monsters,
    skipped: 0, partial: 0, skippedReasons: [], partialNotes: [], sources: [], byCr: [{ value: "1", count: monsters }],
    byType: [{ value: "beast", count: monsters }], rulesets: [], extras: NO_EXTRAS, captured: true,
  };
}

export function coverage() {
  const packs = [srd("srd-5.1-monsters", "SRD 5.1 Monsters", 317), srd("srd-5.2.1-monsters", "SRD 5.2.1 Monsters", 330), ...TITLES.map((t, i) => coveragePack(i, t))];
  return {
    totals: { packs: packs.length, monsters: packs.reduce((n, p) => n + p.monsters, 0), companions: 0 },
    packs,
  };
}

export function packs() {
  const pack = (id: string, name: string, kind: string) => ({
    id, name, kind, version: "1", rulesets: ["dnd2014"], elementCount: 40, license: "", attribution: "", enabled: true,
    redistributable: kind === "local-import" ? false : undefined, sourceIds: [],
  });
  return [pack("core", "SRD Core", "bundled"), ...TITLES.map((t, i) => pack(`made-up-${i}`, `5etools-format: ${t}`, "local-import"))];
}

export function sources() {
  return [{ url: SOURCE_URL, revision: "abc123", sources: [], packs: TITLES.map((_, i) => `made-up-${i}`), lastChecked: "2026-11-01T09:00:00Z", lastImported: "2026-11-01T09:00:00Z", problem: null }];
}
