import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { MemoryRouter, Route, Routes, useNavigate } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Api } from "../../src/api/contract";
import { AURORA_CHARACTER, AURORA_PACK, BUNDLE_ZIP, fileOf, FIVETOOLS_HOMEBREW, FIVETOOLS_ZIP } from "../import/testFiles";

/** The commands the screen sends, by name, with their arguments. */
const calls = vi.hoisted(() => [] as { name: string; args: unknown }[]);
const state = vi.hoisted(() => ({
  packs: [] as { id: string; name: string }[],
  urls: [] as { url: string }[],
  indexes: [] as { url: string; name: string; lastChecked: string | null; problem: string | null }[],
  refuse: null as string | null,
  confirm: "confirm" as string,
}));

function fakeApi(): Api {
  type Args = { fileName: string; url: string };
  const record = (name: string, answer: (args: Args) => unknown) => async (args?: unknown) => {
    calls.push({ name, args });
    return answer((args ?? {}) as Args);
  };
  const scan = { sources: [{ abbr: "SRDX", name: "SRD sample", ruleset: "dnd2014", category: "homebrew", counts: { spell: 1 }, unsupported: {}, needs: [] }] };
  return {
    subscribe: () => () => undefined,
    importCharacter: record("importCharacter", (a) => ({ summary: { name: a.fileName.replace(/\..*/, "") }, warnings: [], missingContent: [], unmappedSlots: [] })),
    listPacks: record("listPacks", () => state.packs),
    previewImportPack: record("previewImportPack", () => ({ previewId: "p1" })),
    cancelImportPreview: record("cancelImportPreview", () => undefined),
    stage5etoolsFiles: record("stage5etoolsFiles", () => ({ token: "brew", name: "brew", path: "" })),
    stage5etoolsZip: record("stage5etoolsZip", () => ({ token: "zip", name: "zip", path: "" })),
    stage5etoolsUrl: record("stage5etoolsUrl", () => { if (state.refuse) throw { code: "invalid_argument", message: state.refuse }; return { token: "url", name: "url", path: "" }; }),
    scan5etoolsFolder: record("scan5etoolsFolder", () => scan),
    import5etools: record("import5etools", () => ({ converted: 1, replaced: 0, partial: [], skipped: [], packs: [{ id: "x", name: "SRD sample" }] })),
    list5etoolsUrls: record("list5etoolsUrls", () => state.urls),
    update5etoolsUrl: record("update5etoolsUrl", () => ({ upToDate: true, report: null })),
    listIndexes: record("listIndexes", () => state.indexes),
    addIndex: record("addIndex", (a) => {
      if (state.refuse) throw { code: "invalid_argument", message: state.refuse };
      state.indexes.push({ url: a.url, name: "SRD sample index", lastChecked: "2026-11-12T10:00:00Z", problem: null });
      return state.indexes;
    }),
    updateIndexes: record("updateIndexes", () => ({ updated: 1, failed: [], reloaded: true, changed: [], newHosts: [], reports: [] })),
  } as unknown as Api;
}

let api: Api;
vi.mock("../../src/state/ApiContext", () => ({ useApi: () => api }));
vi.mock("../../src/components/Confirm", () => ({ useConfirm: () => async () => state.confirm }));
vi.mock("../../src/components/ImportNotice", () => ({ useImportNotice: () => ({ ensure: async () => true, read: () => undefined }) }));
vi.mock("../api/session", () => ({ loadSession: () => ({ baseUrl: "http://server", token: "t", user: { username: "ana" } }) }));
vi.mock("../api/bundles", () => ({
  openBundleFromImport: async (_s: unknown, fileName: string) => {
    calls.push({ name: "openBundleFromImport", args: { fileName } });
    return { summary: { name: "Ana" }, warnings: ["Also in the bundle: Bo."] };
  },
}));
// Upstream's preview dialog, reduced to its two ways out (the real store underneath).
vi.mock("../../src/pages/content/ImportPreviewDialog", async () => {
  const store = await import("../../src/state/importPreview");
  return {
    ImportPreviewHost: () => {
      const navigate = useNavigate();
      const { preview } = store.useImportPreviewState();
      if (!preview) return null;
      return (
        <div role="dialog" aria-label="Import preview">
          <button onClick={() => { store.setImportBusy(true); state.packs.push({ id: "new", name: "SRD sample" }); setTimeout(() => { store.showInstallReport({ issues: [] } as never); navigate("/content"); }, 0); }}>Install</button>
          <button onClick={() => store.dismissImportPreview(api)}>Cancel</button>
        </div>
      );
    },
  };
});

const { ImportHub: Hub } = await import("./ImportHub");
/** The screen in a router, as in the app; upstream's preview goes to /content after installing. */
function ImportHub() {
  return (
    <MemoryRouter initialEntries={["/import-files"]}>
      <Routes>
        <Route path="/import-files" element={<Hub />} />
        <Route path="/content" element={<p>Additional Content</p>} />
      </Routes>
    </MemoryRouter>
  );
}

const settle = () => act(() => new Promise((r) => setTimeout(r, 20)));
const names = () => calls.map((c) => c.name);
const row = (title: string | RegExp) => screen.getByText(title, { selector: ".party-import-name" }).closest("li")!;

beforeEach(() => {
  calls.length = 0;
  Object.assign(state, { packs: [], urls: [], indexes: [], refuse: null, confirm: "confirm" });
  api = fakeApi();
});

async function choose(files: File[]) {
  render(<ImportHub />);
  fireEvent.change(screen.getByLabelText("Choose files to import"), { target: { files } });
  await settle();
  await settle();
}

describe("Choose files", () => {
  it("sends each file to its importer and lists every result", async () => {
    await choose([
      fileOf("Ana.dnd5e", AURORA_CHARACTER), fileOf("srd.xml", AURORA_PACK), fileOf("content.borealis", BUNDLE_ZIP()),
      fileOf("brew.json", FIVETOOLS_HOMEBREW), fileOf("photo.png", "png"),
    ]);
    expect(calls.find((c) => c.name === "importCharacter")?.args).toMatchObject({ fileName: "Ana.dnd5e" });
    expect(calls.find((c) => c.name === "previewImportPack")?.args).toMatchObject({ request: { files: [{ fileName: "srd.xml" }] } });
    expect(within(row("photo.png")).getByRole("alert").textContent).toMatch(/Nothing here reads this file/);
    expect(within(row("Ana.dnd5e")).getByRole("status").textContent).toBe("Imported Ana.");
    // The bundle waits for the pack preview to close: one dialog at a time.
    expect(names()).not.toContain("openBundleFromImport");
    expect(within(row("srd.xml")).getByRole("status").textContent).toMatch(/import preview/);

    fireEvent.click(screen.getByRole("button", { name: "Install" }));
    await vi.waitFor(() => expect(within(row("brew.json")).queryByRole("button", { name: "Import 1 book" })).toBeTruthy());
    expect(within(row("srd.xml")).getByRole("status").textContent).toBe("Installed SRD sample.");
    // Still here: the preview's trip to Additional Content is kept from leaving the Import screen.
    expect(screen.queryByText("Additional Content")).toBeNull();
    expect(within(row("content.borealis")).getByRole("status").textContent).toBe("Imported Ana.");
    expect(within(row("content.borealis")).getByText("Also in the bundle: Bo.")).toBeTruthy();
    expect(calls.find((c) => c.name === "stage5etoolsFiles")?.args).toMatchObject({ files: [{ fileName: "brew.json" }] });
    expect(calls.find((c) => c.name === "scan5etoolsFolder")?.args).toEqual({ folder: "brew" });

    fireEvent.click(within(row("brew.json")).getByRole("button", { name: "Import 1 book" }));
    await settle();
    expect(calls.find((c) => c.name === "import5etools")?.args).toEqual({ request: { folder: "brew", sources: ["SRDX"], includeFluff: false } });
    expect(within(row("brew.json")).getByRole("status").textContent).toBe("Imported 1 element into 1 content pack.");
  });

  it("goes by content: a bundle named .zip, a 5etools zip", async () => {
    await choose([fileOf("Download.zip", BUNDLE_ZIP()), fileOf("data.zip", FIVETOOLS_ZIP())]);
    expect(names()).toContain("openBundleFromImport");
    expect(calls.find((c) => c.name === "stage5etoolsZip")?.args).toMatchObject({ file: { fileName: "data.zip" } });
    expect(names()).not.toContain("previewImportPack");
  });

  it("says when the preview was closed without installing", async () => {
    await choose([fileOf("srd.xml", AURORA_PACK)]);
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    await settle();
    expect(within(row("srd.xml")).getByRole("status").textContent).toMatch(/closed; nothing was installed/);
  });

  it("keeps the server's words when an import fails", async () => {
    api.importCharacter = async () => { throw { code: "too_large", message: "Too large to send through party.example.com: 120 MB." }; };
    await choose([fileOf("Ana.dnd5e", AURORA_CHARACTER)]);
    expect(within(row("Ana.dnd5e")).getByRole("alert").textContent).toBe("Too large to send through party.example.com: 120 MB.");
  });
});

describe("Paste a link", () => {
  async function paste(url: string) {
    render(<ImportHub />);
    fireEvent.change(screen.getByLabelText("Link to import"), { target: { value: url } });
    fireEvent.click(screen.getByRole("button", { name: "Import" }));
    await settle();
  }

  it("adds an index and downloads it", async () => {
    await paste("https://example.com/srd.index");
    expect(calls.find((c) => c.name === "addIndex")?.args).toEqual({ url: "https://example.com/srd.index" });
    expect(names()).toContain("updateIndexes");
    expect(row("https://example.com/srd.index").getAttribute("data-tone")).toBe("success");
  });

  it("shows the server's refusal as it is", async () => {
    state.refuse = "That address is on a private network, so the server won't download from it.";
    await paste("https://192.168.1.10/srd.index");
    expect(within(row("https://192.168.1.10/srd.index")).getByRole("alert").textContent).toBe(state.refuse);
  });

  it("asks first, and adds nothing when the answer is no", async () => {
    state.confirm = "cancel";
    await paste("https://example.com/srd.index");
    expect(names()).not.toContain("addIndex");
    expect(within(row("https://example.com/srd.index")).getByRole("status").textContent).toBe("Not added.");
  });

  it("downloads a 5etools repository and offers its books", async () => {
    await paste("https://github.com/someone/data-repo");
    expect(calls.find((c) => c.name === "stage5etoolsUrl")?.args).toEqual({ url: "https://github.com/someone/data-repo" });
    expect(within(row("https://github.com/someone/data-repo")).getByRole("button", { name: "Import 1 book" })).toBeTruthy();
  });

  it("lets the user say what an unclear address is", async () => {
    await paste("https://example.com/content");
    expect(names()).not.toContain("addIndex");
    fireEvent.click(within(row("https://example.com/content")).getByRole("button", { name: "Add as an index" }));
    await settle();
    expect(calls.find((c) => c.name === "addIndex")?.args).toEqual({ url: "https://example.com/content" });
  });
});

describe("Get the latest 5etools data", () => {
  it("downloads the preset the first time", async () => {
    render(<ImportHub />);
    fireEvent.click(screen.getByRole("button", { name: "Get the latest 5etools data" }));
    await settle();
    expect(calls.find((c) => c.name === "stage5etoolsUrl")?.args).toEqual({ url: "https://github.com/5etools-mirror-3/5etools-src" });
  });

  it("updates the same books once it was imported", async () => {
    state.urls = [{ url: "https://github.com/5etools-mirror-3/5etools-src" }];
    render(<ImportHub />);
    fireEvent.click(screen.getByRole("button", { name: "Get the latest 5etools data" }));
    await settle();
    expect(names()).toContain("update5etoolsUrl");
    expect(names()).not.toContain("stage5etoolsUrl");
    expect(within(row("Latest 5etools data")).getByRole("status").textContent).toBe("Everything is up to date.");
  });
});
