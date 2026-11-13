import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { assistantDeps, resetAssistant, type Turn } from "../api/overrides/assistant";
import { saveSession } from "../api/session";
import { memoryStorage } from "../test/memoryStorage";
import { AssistantPanel, SIMULATE } from "./AssistantPanel";
import { fakeAi, reply, SAVED, type FakeAi } from "./fakeAi";

let ai: FakeAi;

beforeEach(() => {
  vi.stubGlobal("localStorage", memoryStorage());
  saveSession({ baseUrl: "http://server", token: "t", user: { id: "u1", username: "u1", email: null, admin: false, mustChangePassword: false } });
  ai = fakeAi();
  assistantDeps.fetch = ai.fetch;
});

afterEach(() => {
  cleanup();
  document.querySelectorAll("[data-party-ask]").forEach((n) => n.remove());
  resetAssistant();
  vi.unstubAllGlobals();
});

function open(onChanged = vi.fn(), onClose = vi.fn(), context: { partyId?: string; encounterId?: string; label?: string } = { partyId: "p1", label: "Tuesday Party" }, purpose: "encounter" | "index" = "encounter") {
  render(
    <MemoryRouter initialEntries={["/x"]}>
      <Routes>
        <Route path="/x" element={<AssistantPanel purpose={purpose} context={context} onClose={onClose} onChanged={onChanged} />} />
        <Route path="/account" element={<p>Account page</p>} />
      </Routes>
    </MemoryRouter>,
  );
  return { onChanged, onClose };
}

async function say(text: string) {
  const box = await screen.findByRole("textbox", { name: "Message to the assistant" });
  fireEvent.change(box, { target: { value: text } });
  fireEvent.click(screen.getByRole("button", { name: "Send" }));
}

describe("the assistant panel", () => {
  it("builds an encounter: every tool call shows, and the write asks in an app dialog first", async () => {
    ai.replies.push(
      reply("", [["bestiary_search", { environments: ["forest"], crMax: "2" }]]),
      reply("", [["encounter_create", { partyId: "p1", name: "Forest ambush" }]]),
      reply("I made a draft: wolves in the trees and a scout above."),
    );
    const { onChanged } = open();
    expect(await screen.findByText(/Uses api\.example\.com \(gpt-test\)/)).toBeTruthy();
    await say("make a forest ambush for my level 5 party");
    const allow = await screen.findByRole("dialog", { name: "Allow: Create an encounter draft?" });
    expect(within(allow).getByText(/name: Forest ambush/)).toBeTruthy();
    // The read already ran and shows as done.
    const search = screen.getByLabelText("Tool: Search the bestiary");
    expect(within(search).getByText("Done")).toBeTruthy();
    fireEvent.click(within(allow).getByRole("button", { name: "Allow" }));
    expect(await screen.findByText("I made a draft: wolves in the trees and a scout above.")).toBeTruthy();
    expect(within(screen.getByLabelText("Tool: Create an encounter draft")).getByText("Done")).toBeTruthy();
    expect(onChanged).toHaveBeenCalledTimes(1);
    // The first message carried the page's ids for the tools; the person's own words show as typed.
    const first = ai.calls.find((c) => c.path === "/api/ai/chat")!.body as { purpose: string; turns: Turn[] };
    expect(first.purpose).toBe("encounter");
    expect(first.turns[0]).toMatchObject({ role: "user", text: expect.stringContaining("partyId p1") });
    expect(screen.getByText("make a forest ambush for my level 5 party")).toBeTruthy();
    expect(document.body.textContent).not.toMatch(/\bXP\b|difficulty/i);
  });

  it("does nothing the person doesn't allow", async () => {
    ai.replies.push(reply("", [["encounter_create", { partyId: "p1", name: "x" }]]), reply("Fine, nothing saved."));
    const { onChanged } = open();
    await say("make one");
    const allow = await screen.findByRole("dialog", { name: "Allow: Create an encounter draft?" });
    fireEvent.click(within(allow).getByRole("button", { name: "Don't allow" }));
    expect(await screen.findByText("Fine, nothing saved.")).toBeTruthy();
    expect(within(screen.getByLabelText("Tool: Create an encounter draft")).getByText("You declined")).toBeTruthy();
    expect(ai.calls.some((c) => c.path === "/api/ai/tools/encounter_create")).toBe(false);
    expect(onChanged).not.toHaveBeenCalled();
  });

  it("shows the model's text as plain text", async () => {
    ai.replies.push(reply("<img src=x onerror=alert(1)> <b>bold</b>"));
    open();
    await say("hi");
    expect(await screen.findByText("<img src=x onerror=alert(1)> <b>bold</b>")).toBeTruthy();
    expect(document.querySelector(".ai-log img, .ai-log b")).toBeNull();
  });

  it("switches purpose and asks the server for that purpose's tools", async () => {
    open();
    await screen.findByRole("textbox", { name: "Message to the assistant" });
    fireEvent.click(screen.getByRole("button", { name: "Homebrew" }));
    await waitFor(() => expect(ai.calls.some((c) => c.path === "/api/ai/tools?purpose=homebrew")).toBe(true));
    expect(screen.getByRole("button", { name: "Homebrew" }).getAttribute("aria-pressed")).toBe("true");
  });

  it("says what went wrong with the provider", async () => {
    open();
    await say("hi");
    expect(await screen.findByText("No more canned replies.")).toBeTruthy();
  });

  it("simulates the open encounter: the play-by-play and what needs a ruling, with no difficulty words", async () => {
    ai.results.sim_auto = { ok: true, result: { step: 9, lines: ["Wolf 1 bites Ana: hit, 7 piercing", "Ana misses Wolf 1"], card: null, ended: true } };
    ai.results.sim_report = { ok: true, result: {
      outcome: "party standing: Ana, Bo", rounds: 4,
      timeline: ["R2 Ana drops to 0 (Wolf 1 Bite)"],
      pcs: [{ name: "Ana", lowestHp: 0, roundsAtZero: 1 }],
      creatures: [{ name: "Wolf 1", wentDown: 3 }],
      adjudications: ["Wolf 2: Pack Tactics"], steering: { agent: 0, basic: 12, fallback: 0 },
    } };
    ai.replies.push(reply("", [["sim_start", { encounter_id: "e1" }], ["sim_auto", { sim_id: "s1", step: 1, until: "end" }], ["sim_report", { sim_id: "s1" }]]), reply("The party won after Ana went down once."));
    open(vi.fn(), vi.fn(), { partyId: "p1", encounterId: "e1", label: "Forest ambush" });
    fireEvent.click(await screen.findByRole("button", { name: "Simulate encounter" }));
    expect(await screen.findByText("The party won after Ana went down once.")).toBeTruthy();
    const sims = screen.getAllByLabelText("Simulation");
    const [play, report] = sims;
    expect(within(play!).getByLabelText("Play-by-play").textContent).toContain("Wolf 1 bites Ana: hit, 7 piercing");
    expect(report!.textContent).toContain("Outcome: party standing: Ana, Bo (rounds: 4)");
    expect(within(report!).getByLabelText("Key moments").textContent).toContain("R2 Ana drops to 0");
    expect(within(report!).getByLabelText("Characters").textContent).toContain("Ana: lowestHp 0, roundsAtZero 1");
    expect(within(report!).getByLabelText("Needs your ruling").textContent).toContain("Pack Tactics");
    const first = ai.calls.find((c) => c.path === "/api/ai/chat")!.body as { turns: Turn[] };
    expect(first.turns[0]).toMatchObject({ text: expect.stringContaining(SIMULATE) });
    expect(screen.queryByRole("dialog", { name: /Allow/ })).toBeNull();
    expect(document.body.textContent).not.toMatch(/\bXP\b|difficulty|deadly|balanced|rating/i);
  });

  it("offers a built index as a zip download", async () => {
    ai.results.index_build = { ok: true, result: { name: "My/Index", base: "https://example.com/brew", files: [{ path: "my-index.index", text: "<index/>" }, { path: "spells.xml", text: "<elements/>" }] } };
    ai.replies.push(reply("", [["index_build", { name: "Mine" }]]), reply("Ready."));
    const created = vi.fn(() => "blob:x");
    vi.stubGlobal("URL", Object.assign(URL, { createObjectURL: created, revokeObjectURL: vi.fn() }));
    const click = vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(() => {});
    open(vi.fn(), vi.fn(), {}, "index");
    await say("build my index");
    fireEvent.click(await screen.findByRole("button", { name: "Download My_Index.zip" }));
    expect(created).toHaveBeenCalled();
    expect(click).toHaveBeenCalled();
    click.mockRestore();
  });

  it("opens in a page's own tool column when it gives a frame", async () => {
    render(
      <MemoryRouter>
        <AssistantPanel purpose="encounter" onClose={vi.fn()} frame={({ body, footer }) => <aside aria-label="Tool column">{body}{footer}</aside>} />
      </MemoryRouter>,
    );
    const column = await screen.findByRole("complementary", { name: "Tool column" });
    expect(await within(column).findByRole("textbox", { name: "Message to the assistant" })).toBeTruthy();
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("leads to Your AI when nothing is set up", async () => {
    ai = fakeAi(null);
    assistantDeps.fetch = ai.fetch;
    const { onClose } = open();
    fireEvent.click(await screen.findByRole("button", { name: "Set up Your AI" }));
    expect(onClose).toHaveBeenCalled();
    expect(await screen.findByText("Account page")).toBeTruthy();
  });

  it("says so when the server doesn't offer the assistant", async () => {
    ai.status = null;
    open();
    expect(await screen.findByText("This server doesn't offer the assistant.")).toBeTruthy();
    expect(screen.queryByRole("textbox")).toBeNull();
  });

  it("names a device model and leaves auto-applied drafts unasked", async () => {
    ai = fakeAi({ ...SAVED, mode: "device", baseUrl: "http://localhost:11434/v1", model: "llama3.1:8b", hasKey: false, keyLast4: null });
    ai.autoApply = true;
    assistantDeps.fetch = ai.fetch;
    const answers = [
      { choices: [{ message: { content: null, tool_calls: [{ id: "t1", function: { name: "encounter_create", arguments: "{\"partyId\":\"p1\",\"name\":\"Bog\"}" } }] } }] },
      { choices: [{ message: { content: "Saved a draft." } }] },
    ];
    assistantDeps.local = async () => new Response(JSON.stringify(answers.shift()), { status: 200 });
    open();
    expect(await screen.findByText(/Uses the model on your device \(llama3\.1:8b\)/)).toBeTruthy();
    expect(screen.getByText(/except drafts/)).toBeTruthy();
    await say("a bog encounter");
    expect(await screen.findByText("Saved a draft.")).toBeTruthy();
    expect(screen.queryByRole("dialog", { name: /Allow/ })).toBeNull();
    expect(ai.calls.some((c) => c.path === "/api/ai/tools/encounter_create")).toBe(true);
  });
});
