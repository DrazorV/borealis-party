import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { assistantDeps, resetAssistant, type SaveForm } from "../api/overrides/assistant";
import { linkDeps, resetLinks } from "../api/overrides/links";
import { saveSession } from "../api/session";
import { ADMIN_HELP, fakeAi, KEYS_OFF, SAVED, status, type FakeAi } from "../assistant/fakeAi";
import { memoryStorage } from "../test/memoryStorage";

const state = vi.hoisted(() => ({ confirm: "confirm" as string, toasts: [] as { text: string; tone?: string }[] }));
vi.mock("../../src/components/Confirm", () => ({ useConfirm: () => async () => state.confirm }));
vi.mock("../../src/components/Toast", () => ({ useToast: () => ({ push: (text: string, tone?: string) => state.toasts.push({ text, tone }) }) }));

const { YourAiCard } = await import("./YourAiCard");

let ai: FakeAi;
const KEY = "sk-test-SECRET-abcdwxyz";
const puts = () => ai.calls.filter((c) => c.method === "PUT").map((c) => c.body as SaveForm);

beforeEach(() => {
  vi.stubGlobal("localStorage", memoryStorage());
  saveSession({ baseUrl: "http://server", token: "t", user: { id: "u1", username: "u1", email: null, admin: false, mustChangePassword: false } });
  ai = fakeAi(null);
  assistantDeps.fetch = ai.fetch;
  state.toasts = [];
  state.confirm = "confirm";
});

afterEach(() => {
  cleanup();
  resetAssistant();
  resetLinks();
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe("Your AI", () => {
  it("saves only after the person agrees, and never shows the key again", async () => {
    render(<YourAiCard />);
    const save = await screen.findByRole("button", { name: "Save" });
    expect((save as HTMLButtonElement).disabled).toBe(true);
    fireEvent.change(screen.getByLabelText("API key"), { target: { value: KEY } });
    fireEvent.click(screen.getByLabelText("I agree my requests go to the provider"));
    expect(screen.getByText(/goes to api\.openai\.com, under its own terms/)).toBeTruthy();
    fireEvent.click(save);
    await waitFor(() => expect(puts()).toHaveLength(1));
    expect(puts()[0]).toMatchObject({ provider: "openai", mode: "server", baseUrl: "https://api.openai.com/v1", model: "gpt-4.1-mini", key: KEY, consent: true, autoApply: false });
    expect(puts()[0]!.profile).toBeUndefined();
    const field = (await screen.findByPlaceholderText(/•••• wxyz saved/)) as HTMLInputElement;
    expect(field.value).toBe("");
    expect(document.body.innerHTML).not.toContain(KEY);
    // Saving again without typing a key keeps the saved one.
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(puts()).toHaveLength(2));
    expect(puts()[1]!.key).toBeUndefined();
    expect(screen.getByText(/1\D?234 of 200\D?000 tokens/)).toBeTruthy();
  });

  it("explains why keys can't be saved here, and offers a model on this device", async () => {
    ai.status = status(null, false);
    render(<YourAiCard />);
    expect(await screen.findByText(/isn't switched on for this server yet; ask its admin/)).toBeTruthy();
    expect(document.body.textContent).not.toContain("BOREALIS__AI__SECRET_KEY");
    expect((screen.getByLabelText("API key") as HTMLInputElement).disabled).toBe(true);
    fireEvent.change(screen.getByRole("combobox", { name: "Start from" }), { target: { value: "Ollama on this device" } });
    expect(screen.queryByLabelText("API key")).toBeNull();
    expect((screen.getByLabelText("Address") as HTMLInputElement).value).toBe("http://localhost:11434/v1");
    expect(screen.getByText(/the server never does/)).toBeTruthy();
    fireEvent.click(screen.getByLabelText("I agree my requests go to the provider"));
    expect(screen.getByText(/goes to the model on your device/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(puts()).toHaveLength(1));
    expect(puts()[0]).toMatchObject({ mode: "device", baseUrl: "http://localhost:11434/v1", model: "llama3.1:8b" });
    expect(puts()[0]!.key).toBeUndefined();
  });

  it("tests a device model from the app, and a server one through the server", async () => {
    ai = fakeAi({ ...SAVED, mode: "device", baseUrl: "http://localhost:1234/v1", hasKey: false, keyLast4: null });
    assistantDeps.fetch = ai.fetch;
    const local = vi.fn(async () => new Response(JSON.stringify({ choices: [{ message: { content: "OK" } }] }), { status: 200 }));
    assistantDeps.local = local;
    render(<YourAiCard />);
    fireEvent.click(await screen.findByRole("button", { name: "Test connection" }));
    await waitFor(() => expect(state.toasts.at(-1)?.text).toBe('Connected: the model answered "OK".'));
    expect(local).toHaveBeenCalledWith("http://localhost:1234/v1/chat/completions", expect.anything());
    expect(ai.calls.some((c) => c.path === "/api/ai/test")).toBe(false);
    cleanup();
    ai = fakeAi(SAVED);
    assistantDeps.fetch = ai.fetch;
    render(<YourAiCard />);
    fireEvent.click(await screen.findByRole("button", { name: "Test connection" }));
    await waitFor(() => expect(ai.calls.some((c) => c.path === "/api/ai/test")).toBe(true));
  });

  it("removes the key or everything after an app confirmation", async () => {
    ai = fakeAi(SAVED);
    assistantDeps.fetch = ai.fetch;
    render(<YourAiCard />);
    state.confirm = "cancel";
    fireEvent.click(await screen.findByRole("button", { name: "Remove key" }));
    await waitFor(() => expect(ai.calls.filter((c) => c.method === "DELETE")).toHaveLength(0));
    state.confirm = "confirm";
    fireEvent.click(screen.getByRole("button", { name: "Remove key" }));
    await waitFor(() => expect(ai.calls.some((c) => c.method === "DELETE" && c.path === "/api/ai/key")).toBe(true));
    fireEvent.click(await screen.findByRole("button", { name: "Remove" }));
    await waitFor(() => expect(ai.calls.some((c) => c.method === "DELETE" && c.path === "/api/ai")).toBe(true));
  });

  it("offers no live-combat option, and a model size for simulations", async () => {
    render(<YourAiCard />);
    await screen.findByRole("button", { name: "Save" });
    expect(screen.queryByRole("switch", { name: /combat/i })).toBeNull();
    expect(screen.getByText(/never touches a live combat/)).toBeTruthy();
    fireEvent.change(screen.getByRole("combobox", { name: "Model size" }), { target: { value: "frontier" } });
    fireEvent.change(screen.getByLabelText("API key"), { target: { value: KEY } });
    fireEvent.click(screen.getByLabelText("I agree my requests go to the provider"));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(puts()).toHaveLength(1));
    expect(puts()[0]!.profile).toBe("frontier");
  });

  it("tells only an admin how to switch keys on; the browser sign-in is disabled for everyone without it", async () => {
    ai.status = status(null, false);
    render(<YourAiCard />);
    expect(await screen.findByText(KEYS_OFF)).toBeTruthy();
    expect(document.body.textContent).not.toContain("BOREALIS__AI__SECRET_KEY");
    const connect = screen.getByRole("button", { name: "Connect OpenRouter" }) as HTMLButtonElement;
    expect(connect.disabled).toBe(true);
    cleanup();
    ai.status = status(null, false, true);
    render(<YourAiCard />);
    expect(await screen.findByText(new RegExp(ADMIN_HELP.slice(0, 40)))).toBeTruthy();
    expect(document.body.textContent).toContain("BOREALIS__AI__SECRET_KEY");
    expect(document.body.textContent).toContain(KEYS_OFF);
    expect((screen.getByRole("button", { name: "Connect OpenRouter" }) as HTMLButtonElement).disabled).toBe(true);
    expect(ai.calls.some((c) => c.path.includes("/oauth/"))).toBe(false);
  });

  it("connects OpenRouter in the browser after the person agrees, then sees the key the server saved", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const opened: string[] = [];
    linkDeps.openTab = (url) => { opened.push(url); };
    render(<YourAiCard />);
    state.confirm = "cancel";
    fireEvent.click(await screen.findByRole("button", { name: "Connect OpenRouter" }));
    await waitFor(() => expect(opened).toHaveLength(0));
    expect(ai.calls.some((c) => c.path.includes("/oauth/"))).toBe(false);
    state.confirm = "confirm";
    fireEvent.click(screen.getByRole("button", { name: "Connect OpenRouter" }));
    await waitFor(() => expect(opened).toHaveLength(1));
    expect(opened[0]).toMatch(/^https:\/\/openrouter\.ai\/auth\?/);
    const start = ai.calls.find((c) => c.path === "/api/ai/oauth/openrouter/start");
    expect(start?.method).toBe("POST");
    expect(start?.body).toMatchObject({ consent: true, autoApply: false });
    expect(await screen.findByText(/Finish signing in to OpenRouter in your browser/)).toBeTruthy();
    // The server saved the key (the browser came back to it): the card notices on its next check.
    ai.status = status({ ...SAVED, baseUrl: "https://openrouter.ai/api/v1", model: "openrouter/auto", keyLast4: "abcd", consentedAt: 99 });
    await vi.advanceTimersByTimeAsync(3100);
    await waitFor(() => expect(state.toasts.at(-1)?.text).toBe("OpenRouter is connected."));
    expect(await screen.findByPlaceholderText(/•••• abcd saved/)).toBeTruthy();
    expect(screen.queryByText(/Finish signing in/)).toBeNull();
  });

  it("points Claude users to AI apps instead", async () => {
    render(<YourAiCard />);
    const link = await screen.findByRole("link", { name: /Account → AI apps/ });
    expect(link.textContent).toContain("Using Claude? Connect claude.ai or Claude Desktop to Borealis Party instead");
    const target = document.createElement("section");
    target.className = "ai-apps";
    target.scrollIntoView = vi.fn();
    document.body.appendChild(target);
    fireEvent.click(link);
    expect(target.scrollIntoView).toHaveBeenCalled();
    target.remove();
  });

  it("isn't there when the server doesn't offer the assistant", async () => {
    ai.status = null;
    const { container } = render(<YourAiCard />);
    await waitFor(() => expect(ai.calls).toHaveLength(1));
    await waitFor(() => expect(container.textContent).toBe(""));
  });
});
