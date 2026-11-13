import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  blankForm, homebrewDeps, keepReports, missingFields, refusalOf, withSetter,
  type ElementForm, type PackDetail, type Report, type TypeSpec,
} from "../api/homebrew";
import { saveSession } from "../api/session";
import { memoryStorage } from "../test/memoryStorage";

const state = vi.hoisted(() => ({ confirm: "confirm" as string, toasts: [] as string[] }));
vi.mock("../../src/components/Confirm", () => ({ useConfirm: () => async () => state.confirm }));
vi.mock("../../src/components/Toast", () => ({ useToast: () => ({ push: (text: string) => state.toasts.push(text) }) }));

const { HomebrewPage, HomebrewPackPage } = await import("./HomebrewPage");
const { HomebrewEditor } = await import("./HomebrewEditor");

const MAGIC: TypeSpec = {
  type: "Magic Item", label: "Magic item", file: "magic-items.xml", idWord: "MAGIC_ITEM", requirements: false, sheet: true,
  fields: [
    { setter: "category", label: "Category", kind: "select", options: ["Wondrous Items"], required: false },
    { setter: "rarity", label: "Rarity", kind: "select", options: ["Common", "Uncommon"], required: false },
    { setter: "attunement", label: "Requires attunement", kind: "bool", required: false },
    { setter: "cost", label: "Cost", kind: "cost", required: false },
  ],
};
const SPELL: TypeSpec = {
  type: "Spell", label: "Spell", file: "spells.xml", idWord: "SPELL", requirements: false, sheet: false, supports: "Spell lists",
  fields: [{ setter: "level", label: "Level", kind: "select", options: ["0", "1"], required: true }],
};

interface Call { method: string; path: string; body?: unknown }
let calls: Call[];
let answer: (c: Call) => unknown;

const pack: PackDetail = {
  id: "hb-test-pack", name: "Test Pack", description: "", author: "tester", authorUrl: "", version: "1.0.1", elementCount: 1,
  updatedAt: 1, files: { "magic-items.xml": "1.0.0" },
  elements: [{ id: "ID_TEST_PACK_MAGIC_ITEM_LANTERN_OF_TESTING", type: "Magic Item", name: "Lantern of Testing", file: "magic-items.xml", revision: 1, updatedAt: 1 }],
};

function page(path: string) {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <Routes>
        <Route path="/homebrew" element={<HomebrewPage />} />
        <Route path="/homebrew/:pack" element={<HomebrewPackPage />} />
        <Route path="/homebrew/:pack/e/:element" element={<HomebrewEditor />} />
        <Route path="/homebrew/:pack/new/:type" element={<HomebrewEditor />} />
      </Routes>
    </MemoryRouter>,
  );
}

beforeEach(() => {
  vi.stubGlobal("localStorage", memoryStorage());
  saveSession({ baseUrl: "http://server", token: "t", user: { id: "u1", username: "tester", email: null, admin: false, mustChangePassword: false } });
  calls = [];
  state.toasts = [];
  state.confirm = "confirm";
  answer = () => Promise.reject({ code: "not_found", message: "no" });
  homebrewDeps.call = async (method, path, body) => {
    const call = { method, path, body };
    calls.push(call);
    if (path === "/api/homebrew/types") return { types: [SPELL, MAGIC] };
    return answer(call);
  };
});
afterEach(cleanup);

describe("form helpers", () => {
  it("sets, keeps attributes of, and removes setters", () => {
    let form: ElementForm = { type: "Item", name: "Kit", description: "" };
    form = withSetter(form, "cost", "5", { currency: "gp" });
    form = withSetter(form, "cost", "6");
    expect(form.setters).toEqual([{ name: "cost", value: "6", attrs: { currency: "gp" } }]);
    form = withSetter(form, "cost", "");
    expect(form.setters).toEqual([]);
  });
  it("fills a one-choice select and says what is missing", () => {
    const form = blankForm(MAGIC);
    expect(form.setters).toEqual([{ name: "category", value: "Wondrous Items" }]);
    expect(form.sheet).toEqual({});
    expect(missingFields(SPELL, blankForm(SPELL))).toEqual(["Name", "Level"]);
  });
  it("keeps a refused save's report", () => {
    const report: Report = { ok: false, errors: 1, warnings: 0, converted: 0, issues: [{ severity: "error", code: "xml_parse", message: "Line 2: bad", elementLine: 2 }] };
    const kept = keepReports({
      path: "/api/homebrew/packs/hb-x/elements", text: JSON.stringify({ code: "invalid_homebrew", message: "m", report, xml: "<x/>" }),
      error: { code: "invalid_homebrew", message: "m" },
    });
    expect(refusalOf(kept)).toEqual({ message: "m", report, xml: "<x/>" });
    expect(keepReports({ path: "/api/other", text: "{}", error: { code: "x", message: "m" } })).toBeUndefined();
    expect(refusalOf({ code: "other" })).toBeNull();
  });
});

describe("pages", () => {
  it("offers the first pack and makes it", async () => {
    answer = ({ method, path }) => {
      if (method === "GET" && path === "/api/homebrew/packs") return { packs: [] };
      if (method === "POST" && path === "/api/homebrew/packs") return { ...pack, elements: [], elementCount: 0 };
      if (path === "/api/homebrew/packs/hb-test-pack") return pack;
      return Promise.reject({ code: "not_found", message: "no" });
    };
    page("/homebrew");
    fireEvent.click(await screen.findByRole("button", { name: "Create your homebrew pack" }));
    const dialog = screen.getByRole("dialog");
    fireEvent.change(within(dialog).getByLabelText("Pack name"), { target: { value: "Test Pack" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Create" }));
    expect(await screen.findByRole("heading", { name: "Test Pack" })).toBeTruthy();
    expect(calls.find((c) => c.method === "POST")?.body).toEqual({ name: "Test Pack", description: "" });
  });

  it("builds the index for the author's own repository, and needs its address", async () => {
    answer = ({ method, path }) => {
      if (path === "/api/homebrew/packs/hb-test-pack" && method === "GET") return pack;
      if (path.startsWith("/api/homebrew/packs/hb-test-pack/index?base=")) {
        return { base: "https://raw.githubusercontent.com/me/brew/main/", files: [{ path: "test-pack.index", text: "<index>the index</index>" }] };
      }
      return Promise.reject({ code: "not_found", message: "no" });
    };
    page("/homebrew/hb-test-pack");
    expect(await screen.findByRole("link", { name: "Edit Lantern of Testing" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: /Publish/ })).toBeNull();
    const show = screen.getByRole("button", { name: "Show the index" }) as HTMLButtonElement;
    expect(show.disabled).toBe(true);
    expect((screen.getByRole("button", { name: "Download zip" }) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.change(screen.getByLabelText("Hosting address"), { target: { value: "https://raw.githubusercontent.com/me/brew/main" } });
    fireEvent.click(show);
    expect(await screen.findByText("<index>the index</index>")).toBeTruthy();
    expect(calls.some((c) => c.path === `/api/homebrew/packs/hb-test-pack/index?base=${encodeURIComponent("https://raw.githubusercontent.com/me/brew/main")}`)).toBe(true);
  });

  it("shows the server's refusal for a pack that is not the account's own homebrew", async () => {
    answer = ({ path }) => {
      if (path === "/api/homebrew/packs/hb-test-pack") return pack;
      if (path.includes("/index?")) return Promise.reject({ code: "not_redistributable", message: "Only your own homebrew, made in the homebrew editor, can be built into an index or exported." });
      return Promise.reject({ code: "not_found", message: "no" });
    };
    page("/homebrew/hb-test-pack");
    fireEvent.change(await screen.findByLabelText("Hosting address"), { target: { value: "https://raw.githubusercontent.com/me/brew/main" } });
    fireEvent.click(screen.getByRole("button", { name: "Show the index" }));
    expect(await screen.findByText(/Only your own homebrew/)).toBeTruthy();
  });

  it("makes an element from the form and shows the engine's errors when it refuses", async () => {
    const report: Report = {
      ok: false, errors: 1, warnings: 0, converted: 0,
      issues: [{ severity: "error", code: "unresolved_reference", message: "`ID_NOWHERE` is not an id in this pack or the installed content.", element: "ID_X", elementLine: 9 }],
    };
    answer = ({ method, path }) => {
      if (method === "POST" && path === "/api/homebrew/packs/hb-test-pack/elements") {
        return Promise.reject({ code: "invalid_homebrew", message: "The element has 1 error(s)", details: { report } });
      }
      return Promise.reject({ code: "not_found", message: "no" });
    };
    page("/homebrew/hb-test-pack/new/Magic%20Item");
    fireEvent.change(await screen.findByLabelText("Name"), { target: { value: "Lantern of Testing" } });
    fireEvent.change(screen.getByLabelText("Rarity"), { target: { value: "Uncommon" } });
    fireEvent.click(screen.getByLabelText("Requires attunement"));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(await screen.findByText(/ID_NOWHERE/)).toBeTruthy();
    expect(screen.getByText(/line 9 of the element/)).toBeTruthy();
    const sent = calls.find((c) => c.method === "POST")?.body as { form: ElementForm };
    expect(sent.form.name).toBe("Lantern of Testing");
    expect(sent.form.setters).toEqual([
      { name: "category", value: "Wondrous Items" },
      { name: "rarity", value: "Uncommon" },
      { name: "attunement", value: "true" },
    ]);
  });

  it("switches to XML and back through the server, and saves XML", async () => {
    const xml = "<element name=\"Lantern of Testing\" type=\"Magic Item\" source=\"Test Pack\" id=\"ID_L\"><description><p>x</p></description></element>";
    answer = ({ method, path, body }) => {
      if (path === "/api/homebrew/packs/hb-test-pack/elements/ID_L" && method === "GET") {
        return { id: "ID_L", type: "Magic Item", name: "Lantern of Testing", file: "magic-items.xml", revision: 1, createdAt: 1, updatedAt: 1, xml, form: { type: "Magic Item", id: "ID_L", name: "Lantern of Testing", description: "x" } };
      }
      if (path === "/api/homebrew/convert") {
        return (body as { form?: unknown }).form ? { xml } : { form: { type: "Magic Item", id: "ID_L", name: "Changed", description: "x" } };
      }
      if (method === "PATCH") {
        return { id: "ID_L", type: "Magic Item", name: "Changed", file: "magic-items.xml", revision: 2, createdAt: 1, updatedAt: 2, xml, form: null,
          report: { ok: true, errors: 0, warnings: 0, converted: 1, issues: [] }, pack: { ...pack, version: "1.0.2" } };
      }
      return Promise.reject({ code: "not_found", message: "no" });
    };
    page("/homebrew/hb-test-pack/e/ID_L");
    expect(await screen.findByDisplayValue("Lantern of Testing")).toBeTruthy();
    fireEvent.click(screen.getByRole("tab", { name: "XML" }));
    const area = await screen.findByLabelText("Element XML");
    expect((area as HTMLTextAreaElement).value).toBe(xml);
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(state.toasts).toEqual(["Saved: version 1.0.2."]));
    expect(calls.find((c) => c.method === "PATCH")?.body).toEqual({ xml });
    expect(screen.getByText("The engine accepts it.")).toBeTruthy();
    fireEvent.click(screen.getByRole("tab", { name: "Form" }));
    expect(await screen.findByDisplayValue("Changed")).toBeTruthy();
  });
});
