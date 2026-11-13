import { act, render, screen } from "@testing-library/react";
import { useState } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Api } from "../../src/api/contract";
import { ApiProvider, useApi } from "../../src/state/ApiContext";
import { useAsync } from "../../src/state/useAsync";
import { ContentActivity } from "./ContentActivity";
import { ContentScope } from "./ContentScope";
import { contentSync, contentSyncDeps, resetContentSync, SETTLE_MS } from "./contentSync";

vi.mock("../api/liveEvents", () => ({ liveEvents: () => () => undefined }));

let fetches = 0;
let mounts = 0;
let packs = ["Alpha", "Beta"];
const api = {
  listPacks: async () => { fetches += 1; return packs; },
  subscribe: () => () => undefined,
} as unknown as Api;

/** Like upstream's sections: a list loaded with useAsync(..., [api]) and some state of its own. */
function Section() {
  const a = useApi();
  const list = useAsync(() => a.listPacks() as unknown as Promise<string[]>, [a]);
  const [typed] = useState(() => { mounts += 1; return "kept"; });
  return <ul aria-label="Packs" data-typed={typed}>{(list.data ?? []).map((p) => <li key={p}>{p}</li>)}</ul>;
}

beforeEach(() => {
  resetContentSync();
  contentSyncDeps.events = () => () => undefined;
  fetches = 0;
  mounts = 0;
  packs = ["Alpha", "Beta"];
});

describe("ContentScope", () => {
  it("fetches the list once per settled change, without mounting the section again", async () => {
    vi.useFakeTimers();
    try {
      render(<ApiProvider api={api}><ContentScope><Section /></ContentScope></ApiProvider>);
      await act(() => vi.advanceTimersByTimeAsync(0));
      expect(fetches).toBe(1);
      packs = ["Beta"];
      act(() => { contentSync.changed(); contentSync.changed(true); contentSync.changed(); });
      await act(() => vi.advanceTimersByTimeAsync(SETTLE_MS));
      expect(fetches).toBe(2);
      expect(screen.getAllByRole("listitem").map((li) => li.textContent)).toEqual(["Beta"]);
      expect(mounts).toBe(1);
    } finally { vi.useRealTimers(); }
  });

  it("with hold, leaves the list alone for an index update's own change", async () => {
    vi.useFakeTimers();
    try {
      render(<ApiProvider api={api}><ContentScope hold><Section /></ContentScope></ApiProvider>);
      await act(() => vi.advanceTimersByTimeAsync(0));
      const end = contentSync.updating();
      act(() => contentSync.changed());
      await act(() => vi.advanceTimersByTimeAsync(SETTLE_MS));
      expect(fetches).toBe(1);
      end();
      act(() => contentSync.changed(true));
      await act(() => vi.advanceTimersByTimeAsync(SETTLE_MS));
      expect(fetches).toBe(2);
    } finally { vi.useRealTimers(); }
  });

  it("follows the account's events while mounted", () => {
    let open = 0;
    contentSyncDeps.events = () => { open += 1; return () => { open -= 1; }; };
    const view = render(<ApiProvider api={api}><ContentScope><Section /></ContentScope><ContentScope><Section /></ContentScope></ApiProvider>);
    expect(open).toBe(1);
    view.unmount();
    expect(open).toBe(0);
  });
});

describe("ContentActivity", () => {
  it("shows Removing… for its own area, then the server's refusal with Dismiss", () => {
    render(<ApiProvider api={api}><ContentActivity areas={["packs"]} /></ApiProvider>);
    let op = 0;
    act(() => { op = contentSync.begin("packs", "Removing “Alpha”…", { packs: ["a"] }); contentSync.begin("indexes", "Elsewhere"); });
    expect(screen.getByRole("status").textContent).toBe("Removing “Alpha”…");
    act(() => contentSync.fail(op, "“Alpha” wasn't removed: no."));
    expect(screen.getByRole("alert").textContent).toMatch(/^“Alpha” wasn't removed: no\.Dismiss$/);
    act(() => screen.getByRole("button", { name: "Dismiss" }).click());
    expect(screen.queryByRole("alert")).toBeNull();
  });
});
