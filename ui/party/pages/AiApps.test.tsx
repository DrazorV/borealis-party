import { act, fireEvent, render, screen } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { saveSession } from "../api/session";
import { memoryStorage } from "../test/memoryStorage";
import { AiAppsCard, setupSnippets } from "./AiApps";
import { AiAppsConsent, keepConsentThroughSignIn, safeRedirect } from "./AiAppsConsent";

const BASE = "https://party.example.com";
const user = { id: "u1", username: "ana", email: null, admin: false, mustChangePassword: false };
const SCOPES = [
  { id: "bestiary:read", label: "Search and read your bestiary", default: true },
  { id: "encounters:read", label: "Read encounters", default: true },
  { id: "homebrew:write", label: "Create and edit your homebrew content", default: false },
];
const ID = "abcdefghijklmnopqrstuvwxyz012345";

let requests: { method: string; path: string; body: unknown }[];
let answers: Record<string, { status: number; body: unknown }>;

const settle = () => act(() => new Promise((r) => setTimeout(r, 20)));

beforeEach(() => {
  vi.stubGlobal("localStorage", memoryStorage());
  vi.stubGlobal("sessionStorage", memoryStorage());
  requests = [];
  answers = {};
  vi.stubGlobal("fetch", vi.fn(async (url: string, init?: RequestInit) => {
    const path = url.replace(BASE, "");
    const method = init?.method ?? "GET";
    requests.push({ method, path, body: init?.body ? JSON.parse(String(init.body)) : undefined });
    const a = answers[`${method} ${path}`] ?? { status: 404, body: { code: "not_found", message: "nope" } };
    return new Response(JSON.stringify(a.body), { status: a.status, headers: { "content-type": "application/json" } });
  }));
});

afterEach(() => { vi.unstubAllGlobals(); });

const signIn = () => saveSession({ baseUrl: BASE, token: "session-token", user });

it("makes setup lines with the address and the token", () => {
  const [claude, json] = setupSnippets(`${BASE}/api/mcp`, "bpt_x");
  expect(claude!.text).toBe(`claude mcp add --transport http borealis-party ${BASE}/api/mcp --header "Authorization: Bearer bpt_x"`);
  expect(JSON.parse(json!.text)).toEqual({ mcpServers: { "borealis-party": { url: `${BASE}/api/mcp`, headers: { Authorization: "Bearer bpt_x" } } } });
});

it("never sends the browser to a scheme that would run in the page", () => {
  expect(safeRedirect("https://claude.ai/api/mcp/auth_callback?code=1")).toBe(true);
  expect(safeRedirect("http://127.0.0.1:3118/callback?code=1")).toBe(true);
  expect(safeRedirect("cursor://anysphere.cursor-mcp/oauth/callback?code=1")).toBe(true);
  for (const bad of ["javascript:alert(1)", "JAVASCRIPT:alert(1)", "data:text/html,x", "vbscript:x", "not a url"]) {
    expect(safeRedirect(bad)).toBe(false);
  }
});

it("keeps a consent link through signing in", () => {
  const loc = { hash: `#/oauth/consent/${ID}` };
  keepConsentThroughSignIn(sessionStorage, loc);
  // Signing in sends the browser to #/ and reloads.
  signIn();
  loc.hash = "#/";
  keepConsentThroughSignIn(sessionStorage, loc);
  expect(loc.hash).toBe(`#/oauth/consent/${ID}`);
  // Only once.
  loc.hash = "#/";
  keepConsentThroughSignIn(sessionStorage, loc);
  expect(loc.hash).toBe("#/");
});

function consentPage(go: (url: string) => void) {
  return render(
    <MemoryRouter initialEntries={[`/oauth/consent/${ID}`]}>
      <Routes><Route path="/oauth/consent/:id" element={<AiAppsConsent go={go} />} /></Routes>
    </MemoryRouter>,
  );
}

it("shows the app, the account and the scopes, and sends back only what was allowed", async () => {
  signIn();
  answers[`GET /api/oauth/requests/${ID}`] = {
    status: 200,
    body: { client: { name: "Claude", uri: null, redirectUri: "https://claude.ai/api/mcp/auth_callback", redirectHost: "https://claude.ai" },
      scopes: SCOPES.slice(0, 2), account: { username: "ana" }, expiresAt: 1 },
  };
  answers[`POST /api/oauth/requests/${ID}/approve`] = { status: 200, body: { redirect: "https://claude.ai/api/mcp/auth_callback?code=bpc_1&state=s" } };
  const go = vi.fn();
  consentPage(go);
  await settle();
  expect(screen.getByText("Claude")).toBeTruthy();
  expect(screen.getByText("ana")).toBeTruthy();
  expect(screen.getByText("https://claude.ai")).toBeTruthy();
  fireEvent.click(screen.getByLabelText(/Search and read your bestiary/));
  fireEvent.click(screen.getByText("Allow"));
  await settle();
  expect(requests.at(-1)).toEqual({ method: "POST", path: `/api/oauth/requests/${ID}/approve`, body: { scopes: ["encounters:read"] } });
  expect(go).toHaveBeenCalledWith("https://claude.ai/api/mcp/auth_callback?code=bpc_1&state=s");
});

it("refuses a redirect that would run in the page", async () => {
  signIn();
  answers[`GET /api/oauth/requests/${ID}`] = {
    status: 200,
    body: { client: { name: "Evil", uri: null, redirectUri: "x", redirectHost: "x" }, scopes: SCOPES.slice(0, 1), account: { username: "ana" }, expiresAt: 1 },
  };
  answers[`POST /api/oauth/requests/${ID}/deny`] = { status: 200, body: { redirect: "javascript:alert(1)" } };
  const go = vi.fn();
  consentPage(go);
  await settle();
  fireEvent.click(screen.getByText("Deny"));
  await settle();
  expect(go).not.toHaveBeenCalled();
  expect(screen.getByRole("alert").textContent).toContain("can't be opened");
});

it("lists tokens and apps, shows a new token once, and asks before revoking", async () => {
  signIn();
  answers["GET /api/me/ai"] = {
    status: 200,
    body: { mcpUrl: `${BASE}/api/mcp`, oauth: true, scopes: SCOPES, defaultDays: 90, maxDays: 365, maxTokens: 25, callsPerMinute: 60 },
  };
  const old = { id: "t1", name: "Laptop", hint: "bpt_abcd…", scopes: ["bestiary:read"], createdAt: 1, expiresAt: 2, lastUsedAt: null };
  answers["GET /api/me/ai/tokens"] = { status: 200, body: { tokens: [old] } };
  answers["GET /api/me/ai/apps"] = {
    status: 200,
    body: { apps: [{ id: "g1", name: "ChatGPT", uri: null, redirectHost: "https://chatgpt.com", scopes: ["encounters:read"], connectedAt: 1, lastUsedAt: 3 }] },
  };
  answers["POST /api/me/ai/tokens"] = { status: 200, body: { ...old, id: "t2", name: "Claude Code", token: "bpt_SECRET" } };
  answers["DELETE /api/me/ai/tokens/t1"] = { status: 200, body: { revoked: true } };
  render(<AiAppsCard />);
  await settle();
  expect(screen.getByText("Laptop")).toBeTruthy();
  expect(screen.getByText("ChatGPT", { selector: "strong" })).toBeTruthy();
  expect(screen.queryByText(/bpt_SECRET/)).toBeNull();

  fireEvent.click(screen.getByText("New token"));
  // The defaults are ticked, and only those.
  expect((screen.getByLabelText(/Create and edit your homebrew/) as HTMLInputElement).checked).toBe(false);
  expect((screen.getByLabelText(/Read encounters/) as HTMLInputElement).checked).toBe(true);
  fireEvent.change(screen.getByLabelText("Token name"), { target: { value: "Claude Code" } });
  fireEvent.click(screen.getByText("Create token"));
  await settle();
  expect(requests.find((r) => r.method === "POST")?.body).toEqual({ name: "Claude Code", scopes: ["bestiary:read", "encounters:read"], expiresInDays: 90 });
  expect((screen.getByLabelText("API token") as HTMLInputElement).value).toBe("bpt_SECRET");
  fireEvent.click(screen.getByText("Done"));
  expect(screen.queryByDisplayValue("bpt_SECRET")).toBeNull();

  fireEvent.click(screen.getAllByText("Revoke")[0]!);
  expect(requests.some((r) => r.method === "DELETE")).toBe(false);
  const buttons = screen.getAllByText("Revoke");
  fireEvent.click(buttons.at(-1)!);
  await settle();
  expect(requests.some((r) => r.method === "DELETE" && r.path === "/api/me/ai/tokens/t1")).toBe(true);
});

it("hides itself when the server has AI apps switched off", async () => {
  signIn();
  const { container } = render(<AiAppsCard />);
  await settle();
  expect(container.textContent).toBe("");
});

it("guides each app in tabs, copies the address and the instructions, and keeps keys in their own section", async () => {
  signIn();
  const writes: string[] = [];
  vi.stubGlobal("navigator", { ...navigator, clipboard: { writeText: async (t: string) => { writes.push(t); } } });
  const INSTRUCTIONS = "You are a Dungeon Master's assistant for Borealis Party.\nNever invent a stat block.";
  answers["GET /api/me/ai"] = {
    status: 200,
    body: { mcpUrl: `${BASE}/api/mcp`, oauth: true, scopes: SCOPES, defaultDays: 90, maxDays: 365, maxTokens: 25, callsPerMinute: 60, instructions: INSTRUCTIONS },
  };
  answers["GET /api/me/ai/tokens"] = { status: 200, body: { tokens: [] } };
  answers["GET /api/me/ai/apps"] = { status: 200, body: { apps: [] } };
  render(<AiAppsCard />);
  await settle();

  // ChatGPT first: numbered steps and example prompts.
  const tabs = screen.getAllByRole("tab").map((t) => t.textContent);
  expect(tabs).toEqual(["ChatGPT", "Claude", "Developer tools"]);
  const panel = () => screen.getByRole("tabpanel");
  expect(panel().textContent).toContain("Add custom MCP server");
  expect(panel().textContent).toContain("OAuth");
  expect(panel().querySelectorAll("ol li").length).toBeGreaterThanOrEqual(4);
  expect(panel().textContent).toContain("Build a forest ambush for my level 5 party from my bestiary.");
  expect(screen.getByText(/Menu names change/)).toBeTruthy();

  fireEvent.click(screen.getByRole("tab", { name: "Claude" }));
  expect(panel().textContent).toContain("Add custom connector");
  expect(panel().textContent).toContain("Connectors");

  fireEvent.click(screen.getByRole("tab", { name: "Developer tools" }));
  expect(panel().textContent).toContain(`claude mcp add --transport http borealis-party ${BASE}/api/mcp`);
  expect(panel().textContent).toContain("codex mcp login borealis-party");

  // Keys live in a collapsible section.
  const keys = screen.getByText("Keys for developer tools", { selector: "summary" }).closest("details")!;
  expect(keys.textContent).toContain("New token");
  expect(keys.open).toBe(false);

  // The address and the instructions copy.
  fireEvent.click(screen.getByRole("button", { name: "Copy" }));
  const box = screen.getByLabelText("Instructions for the assistant") as HTMLTextAreaElement;
  expect(box.readOnly).toBe(true);
  expect(box.value).toBe(INSTRUCTIONS);
  expect(screen.getByText(/Connected apps get these automatically; for ChatGPT, also paste them into a Project's instructions/)).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Copy instructions" }));
  await settle();
  expect(writes).toEqual([`${BASE}/api/mcp`, INSTRUCTIONS]);
});
