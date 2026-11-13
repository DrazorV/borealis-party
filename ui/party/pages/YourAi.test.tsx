import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { assistantDeps, resetAssistant, type SaveForm } from "../api/overrides/assistant";
import { saveSession } from "../api/session";
import { fakeAi, SAVED, status, type FakeAi } from "../assistant/fakeAi";
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
    expect(await screen.findByText(/BOREALIS__AI__SECRET_KEY/)).toBeTruthy();
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

  it("isn't there when the server doesn't offer the assistant", async () => {
    ai.status = null;
    const { container } = render(<YourAiCard />);
    await waitFor(() => expect(ai.calls).toHaveLength(1));
    await waitFor(() => expect(container.textContent).toBe(""));
  });
});
