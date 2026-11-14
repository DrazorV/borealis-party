import { readFileSync } from "node:fs";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { NoticesBanner, readableId, readableNotice } from "./NoticesBanner";

const deps = vi.hoisted(() => ({ snapshot: { id: "c1", missingContent: [] as { id: string; name: string }[] } }));
vi.mock("../../src/state/CharacterContext", () => ({ useCharacter: () => ({ snapshot: deps.snapshot }) }));

// Made-up content: a homebrew book "HBX" whose spell "Frost Knot" a race grants through Aurora's proxy item.
const PROXY = "ID_HBX_INTERNAL_ITEM__SPELL_PROXY_SPELL_FROST_KNOT";
const MISSING = `inventory item ${PROXY} is not installed`;

afterEach(() => {
  sessionStorage.clear();
  deps.snapshot = { id: "c1", missingContent: [] };
  document.body.innerHTML = "";
});

describe("readable notices", () => {
  it("names a proxy item by what it grants", () => {
    expect(readableId(PROXY)).toBe("Additional Spell, Frost Knot");
    expect(readableId("ID_HBX_INTERNAL_ITEM_BLADE_MAGE_SPELL_PROXY_SPELL_FROST_KNOT")).toBe("Additional Blade Mage Spell, Frost Knot");
    expect(readableId("ID_HBX_INTERNAL_ITEM_FEAT_PROXY_IRON_WILL")).toBe("Additional Feat, Iron Will");
    expect(readableId("ID_HBX_MAGIC_ITEM_LANTERN_OF_DUSK")).toBe("Lantern Of Dusk");
  });

  it("says what is missing in words and keeps the raw id", () => {
    expect(readableNotice(MISSING)).toEqual({
      text: "This character needs an item that isn't in your installed content: Additional Spell, Frost Knot.",
      id: PROXY,
    });
    expect(readableNotice("chosen element ID_HBX_RACE_SNOWKIN is not installed")?.text).toContain("needs a choice");
    expect(readableNotice("A user content pack could not be loaded: x.json")).toBeNull();
  });

  it("prefers the name the import recorded", () => {
    expect(readableNotice(MISSING, [{ id: PROXY, name: "Additional Spell, Frost's Knot", type: "", context: "", source: "" }])?.text)
      .toContain("Frost's Knot");
  });

  it("shows the sentence with the id in a Details toggle", () => {
    render(<NoticesBanner notices={[MISSING]} />);
    const banner = screen.getByRole("region", { name: "Content notices" });
    expect(banner.textContent).toContain("needs an item that isn't in your installed content: Additional Spell, Frost Knot");
    expect(banner.querySelector("details > summary")?.textContent).toBe("Details");
    expect(banner.querySelector("details > code")?.textContent).toBe(PROXY);
  });
});

describe("layout", () => {
  it("carries the class that lines it up with the rows and spaces it from them", () => {
    render(<NoticesBanner notices={[MISSING]} />);
    expect(screen.getByRole("region", { name: "Content notices" }).classList).toContain("party-notices");
    // vitest stubs CSS imports, so read the file (the tests run from ui/)
    const css = readFileSync("party/builder/notices.css", "utf8");
    // the rows start past their focus bar: 4 px border + 10 px padding (6 px under 900 px), none on phones
    expect(css).toMatch(/\.alert\.bp-notices\.party-notices \{[^}]*margin: 0 0 var\(--sp-3\) 14px/);
    expect(css).toMatch(/max-width: 899px\) \{ \.alert\.bp-notices\.party-notices \{ margin-left: 10px/);
    expect(css).toMatch(/html\.phone \.alert\.bp-notices\.party-notices \{ margin-left: 0/);
  });
});

describe("dismiss", () => {
  it("hides the notices for this character and remembers it for the session", async () => {
    const { unmount } = render(<NoticesBanner notices={[MISSING, "other"]} />);
    await userEvent.setup().click(screen.getByRole("button", { name: "Dismiss notices" }));
    expect(screen.queryByRole("region", { name: "Content notices" })).toBeNull();
    unmount();
    render(<NoticesBanner notices={[MISSING, "other"]} />);
    expect(screen.queryByRole("region", { name: "Content notices" })).toBeNull();
  });

  it("shows a new or changed message again, and other characters' notices", async () => {
    const view = render(<NoticesBanner notices={[MISSING]} />);
    await userEvent.setup().click(screen.getByRole("button", { name: "Dismiss notices" }));
    view.rerender(<NoticesBanner notices={[MISSING, "a new problem"]} />);
    const banner = screen.getByRole("region", { name: "Content notices" });
    expect(banner.textContent).toContain("a new problem");
    expect(banner.textContent).not.toContain("Frost Knot");
    view.unmount();
    deps.snapshot = { id: "c2", missingContent: [] };
    render(<NoticesBanner notices={[MISSING]} />);
    expect(screen.getByRole("region", { name: "Content notices" }).textContent).toContain("Frost Knot");
  });

  it("still works when storage is blocked", async () => {
    const get = vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => { throw new Error("blocked"); });
    const set = vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("blocked"); });
    render(<NoticesBanner notices={[MISSING]} />);
    await userEvent.setup().click(screen.getByRole("button", { name: "Dismiss notices" }));
    expect(screen.queryByRole("region", { name: "Content notices" })).toBeNull();
    get.mockRestore();
    set.mockRestore();
  });
});
