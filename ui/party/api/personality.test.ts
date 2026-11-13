import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { CharacterSnapshot, SelectionSlot } from "../../src/api/contract";
import { createHttpApi } from "./http";
import { createOfflineApi } from "./offline";
import { saveSession, type ServerSession } from "./session";
import { memoryStorage } from "../test/memoryStorage";
import { followSettings, isPersonalitySlot, personalitySkipped, rewrite, SKIP_KEY, withoutPersonality } from "./personality";

/** The SRD Acolyte's choices (content/srd-5.1/backgrounds.json): two languages, then the four personality lists. */
const BG = "ID_BACKGROUND_ACOLYTE";

function slot(over: Partial<SelectionSlot> & Pick<SelectionSlot, "id" | "title">): SelectionSlot {
  return {
    tab: "background", groupId: null, groupLabel: null, classSlotId: null, elementType: "list", ordinal: 1, of: 1,
    required: true, locked: false, chosen: null, requiredLevel: null, allowReplace: false, hint: null, gainedOnLevel: 1,
    replaced: [], emptyReason: null, problem: null, prerequisitesUnmet: false, ...over,
  };
}

const language = (n: number) => slot({ id: `${BG}:2:${n}`, title: "Language (Acolyte)", elementType: "language", ordinal: n, of: 2 });
const trait = (n: number) => slot({ id: `${BG}:4:${n}`, title: "Personality Trait", ordinal: n, of: 2 });
const ideal = slot({ id: `${BG}:5:1`, title: "Ideal" });
const bond = slot({ id: `${BG}:6:1`, title: "Bond" });
const flaw = slot({ id: `${BG}:7:1`, title: "Flaw" });
const cantrip = slot({ id: "ID_CLASS_WIZARD:3:1", title: "Cantrip", tab: "spells", elementType: "spell" });

/** A snapshot as the host makes it: every unfilled required slot counted, Next selection the host's first. */
function snapshot(slots: SelectionSlot[], nextSlotId: string | null): CharacterSnapshot {
  const byTab = { race: 0, class: 0, background: 0, abilities: 0, feats: 0, languages: 0, proficiencies: 0, companion: 0, spells: 0 };
  for (const s of slots) if (s.required && !s.chosen) byTab[s.tab] += 1;
  const remaining = Object.values(byTab).reduce((a, b) => a + b, 0);
  return { id: "c1", slots, progress: { total: slots.length, remaining, nextSlotId, byTab } } as unknown as CharacterSnapshot;
}

const picked = (s: SelectionSlot, name: string): SelectionSlot => ({ ...s, chosen: { id: `${s.id}#1`, name, type: "list" } as SelectionSlot["chosen"] });

afterEach(() => followSettings({}));

describe("which choices are personality choices", () => {
  it("a background's Personality Trait, Ideal, Bond and Flaw lists, by any prefix or plural", () => {
    for (const s of [trait(1), trait(2), ideal, bond, flaw]) expect(isPersonalitySlot(s), s.title).toBe(true);
    expect(isPersonalitySlot(slot({ id: `${BG}:9:1`, title: "Guild Personality Trait" }))).toBe(true);
    expect(isPersonalitySlot(slot({ id: `${BG}:9:1`, title: "Ideals" }))).toBe(true);
    // An Aurora "Background Characteristics" element granted by the background owns its lists on the Class section.
    expect(isPersonalitySlot(slot({ id: "ID_WOTC_BACKGROUND_CHARACTERISTICS_X:0:1", title: "Bond", tab: "class" }))).toBe(true);
  });

  it("not the background's other choices, nor a class's own Bond", () => {
    expect(isPersonalitySlot(language(1))).toBe(false);
    expect(isPersonalitySlot(slot({ id: `${BG}:8:1`, title: "Alignment" }))).toBe(false);
    expect(isPersonalitySlot(slot({ id: "ID_CLASS_X:1:1", title: "Spirit Bond", tab: "class", elementType: "class-feature" }))).toBe(false);
    expect(isPersonalitySlot(slot({ id: "ID_CLASS_X:1:1", title: "Blood Bond", tab: "class" }))).toBe(false);
  });
});

describe("the selection flow without them", () => {
  it("Next selection goes past them, and the counts leave them out", () => {
    const host = snapshot([language(1), language(2), trait(1), trait(2), ideal, bond, flaw, cantrip], `${BG}:2:1`);
    expect(host.progress.remaining).toBe(8);
    const s = withoutPersonality(host);
    expect(s.progress.nextSlotId).toBe(`${BG}:2:1`);
    expect(s.progress.byTab.background).toBe(2);
    expect(s.progress.byTab.spells).toBe(1);
    expect(s.progress.remaining).toBe(3);
    // Still listed, still pickable: only no longer required.
    expect(s.slots.map((x) => x.id)).toEqual(host.slots.map((x) => x.id));
    expect(s.slots.filter((x) => !x.required).map((x) => x.title)).toEqual(["Personality Trait", "Personality Trait", "Ideal", "Bond", "Flaw"]);
  });

  it("once the languages are chosen, Next selection goes to the next section, not to a trait", () => {
    const host = snapshot([picked(language(1), "Common"), picked(language(2), "Elvish"), trait(1), trait(2), ideal, bond, flaw, cantrip], `${BG}:4:1`);
    const s = withoutPersonality(host);
    expect(s.progress.nextSlotId).toBe(cantrip.id);
    expect(s.progress.byTab.background).toBe(0);
    expect(s.progress.remaining).toBe(1);
  });

  it("with nothing else open the build reads as complete", () => {
    const s = withoutPersonality(snapshot([picked(language(1), "Common"), picked(language(2), "Elvish"), trait(1), trait(2), ideal, bond, flaw], `${BG}:4:1`));
    expect(s.progress).toMatchObject({ nextSlotId: null, remaining: 0 });
  });

  it("picks already made are kept as they are", () => {
    const host = snapshot([picked(language(1), "Common"), language(2), picked(trait(1), "I quote sacred texts."), trait(2), picked(ideal, "Tradition.")], `${BG}:2:2`);
    const s = withoutPersonality(host);
    expect(s.slots.find((x) => x.id === trait(1).id)?.chosen?.name).toBe("I quote sacred texts.");
    expect(s.slots.find((x) => x.id === ideal.id)?.chosen?.name).toBe("Tradition.");
    expect(s.progress).toMatchObject({ nextSlotId: `${BG}:2:2`, remaining: 1 });
  });

  it("a snapshot without them is the same snapshot", () => {
    const host = snapshot([language(1), cantrip], language(1).id);
    expect(withoutPersonality(host)).toBe(host);
  });
});

describe("the setting", () => {
  const host = () => snapshot([trait(1), language(1)], trait(1).id);

  it("is on by default, and off leaves the host's flow as it is", () => {
    followSettings({});
    expect(personalitySkipped()).toBe(true);
    expect((rewrite(host()) as CharacterSnapshot).progress.nextSlotId).toBe(language(1).id);
    followSettings({ [SKIP_KEY]: false });
    expect(personalitySkipped()).toBe(false);
    expect(rewrite(host())).toEqual(host());
    followSettings({ [SKIP_KEY]: true });
    expect((rewrite(host()) as CharacterSnapshot).progress.nextSlotId).toBe(language(1).id);
  });

  it("rewrites a snapshot inside an answer too (switch version, convert to 2024)", () => {
    const out = rewrite({ snapshot: host(), dropped: [] }) as { snapshot: CharacterSnapshot; dropped: string[] };
    expect(out.snapshot.progress.nextSlotId).toBe(language(1).id);
    expect(out.dropped).toEqual([]);
    expect(rewrite("text")).toBe("text");
  });
});

describe("over the API", () => {
  const session: ServerSession = { baseUrl: "http://server", token: "t", user: { id: "u", username: "alice", email: null, admin: false, mustChangePassword: false } };
  let skipOnServer: boolean;
  let calls: string[];

  beforeEach(() => {
    vi.stubGlobal("localStorage", memoryStorage());
    saveSession(session);
    skipOnServer = true;
    calls = [];
    vi.stubGlobal("fetch", vi.fn(async (url: string, init: RequestInit) => {
      const command = url.split("/").pop()!;
      calls.push(command);
      const body = JSON.parse(String(init.body ?? "{}")) as { patch?: Record<string, unknown> };
      if (command === "save_settings" && typeof body.patch?.[SKIP_KEY] === "boolean") skipOnServer = body.patch[SKIP_KEY] as boolean;
      const answer = command.endsWith("settings") ? { accentColor: "blue", [SKIP_KEY]: skipOnServer } : snapshot([trait(1), language(1)], trait(1).id);
      return new Response(JSON.stringify(answer), { status: 200 });
    }));
  });
  afterEach(() => vi.unstubAllGlobals());

  it("the account's setting, read and saved through the server, decides each snapshot", async () => {
    const api = createHttpApi(session);
    await api.getSettings();
    expect((await api.getSnapshot({ characterId: "c1" })).progress.nextSlotId).toBe(language(1).id);
    // A pick of a trait goes to the server as before.
    await api.applySelection({ characterId: "c1", slotId: trait(1).id, elementId: "1" });
    expect(calls).toContain("apply_selection");

    await api.saveSettings({ patch: { [SKIP_KEY]: false } as never });
    expect(personalitySkipped()).toBe(false);
    expect((await api.getSnapshot({ characterId: "c1" })).progress.nextSlotId).toBe(trait(1).id);
    await api.restoreDefaultSettings();
    expect(personalitySkipped()).toBe(false); // the stub server keeps it; a real Restore defaults answers true
  });

  it("offline, the app's own backend keeps the account's last choice", async () => {
    followSettings({ [SKIP_KEY]: false });
    const local = {
      getSettings: async () => ({ accentColor: "blue" }),
      getSnapshot: async () => snapshot([trait(1), language(1)], trait(1).id),
    } as never;
    const api = createOfflineApi(local, new Map());
    await api.getSettings();
    expect(personalitySkipped()).toBe(false);
    expect((await api.getSnapshot({ characterId: "c1" })).progress.nextSlotId).toBe(trait(1).id);
    followSettings({});
    expect((await api.getSnapshot({ characterId: "c1" })).progress.nextSlotId).toBe(language(1).id);
  });
});
