import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { memoryStorage } from "../test/memoryStorage";
import { emptyMirror, loadMirror, saveMirror } from "../sync/mirror";
import { loadSession, saveSession, type ServerSession } from "../api/session";

/** The app's backend: one copy per account, whose file may hold an edit made here. */
const device = vi.hoisted(() => ({
  files: new Map<string, string>(),
  deleted: [] as string[],
  sync: [] as string[],
  offline: false,
}));
vi.mock("../../src/lib/tauri", () => ({ isTauri: () => true }));
vi.mock("../../src/api/tauri", () => ({
  createTauriApi: () => ({
    listCharacters: async () => [...device.files.keys()].map((id) => ({ id, name: `Hero ${id}`, updatedAt: device.files.get(id) === "{}" ? "t1" : "t2" })),
    loadCharacter: async () => ({}),
    exportCharacter: async ({ characterId }: { characterId: string }) => ({ dataBase64: btoa(device.files.get(characterId) ?? "") }),
    closeCharacter: async () => {},
    deleteCharacter: async ({ characterId }: { characterId: string }) => { device.deleted.push(characterId); device.files.delete(characterId); },
    listPacks: async () => [],
    listIndexes: async () => [],
  }),
}));
vi.mock("../sync/runner", () => ({
  lastSync: async () => { device.sync.push("last"); },
  resumeSync: () => { device.sync.push("resume"); },
}));
vi.mock("../api/connectivity", () => ({ isOffline: () => device.offline, openCopies: () => new Set<string>() }));

const { useSignOut } = await import("./SignOut");
const { ConnectPage } = await import("./ConnectPage");

const user = (id: string) => ({ id, username: id, email: `${id}@example.com`, admin: false, mustChangePassword: false });
const session: ServerSession = { baseUrl: "http://server", token: "t", user: user("ana") };
let reload: ReturnType<typeof vi.fn>;
let requests: string[];

/** A copy of `userId`'s character on the device; `edited` gives it a change the server does not have. */
async function copy(userId: string, local: string, edited: boolean) {
  const sha = [...new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode("{}")))].map((b) => b.toString(16).padStart(2, "0")).join("");
  device.files.set(local, edited ? '{"hp":1}' : "{}");
  const m = emptyMirror(userId);
  m.lastSyncMs = 1;
  m.characters[`s_${local}`] = { local, serverSha: "x", localSha: sha, localMtime: "t1" };
  saveMirror(m);
}

function Bar() {
  const out = useSignOut();
  return <><button onClick={out.start}>Sign out</button>{out.dialog}</>;
}

const settle = () => act(() => new Promise((r) => setTimeout(r, 30)));

beforeEach(() => {
  vi.stubGlobal("localStorage", memoryStorage());
  vi.stubGlobal("sessionStorage", memoryStorage());
  reload = vi.fn();
  vi.stubGlobal("location", { ...window.location, reload, hash: "" });
  requests = [];
  vi.stubGlobal("fetch", vi.fn(async (url: string, init?: RequestInit) => {
    requests.push(`${init?.method ?? "GET"} ${url}`);
    if (url.endsWith("/api/server")) return new Response(JSON.stringify({ app: "borealis-party", openRegistration: false }), { status: 200 });
    if (url.endsWith("/api/auth/login")) return new Response(JSON.stringify({ token: "new", user: user("bo") }), { status: 200 });
    return new Response("", { status: 200 });
  }));
  device.files.clear();
  device.deleted = [];
  device.sync = [];
  device.offline = false;
});

afterEach(() => { vi.unstubAllGlobals(); document.body.innerHTML = ""; });

/** The "Sign out" button inside the dialog's foot (the page's own button has the same label). */
const dialogButton = (label: string) => [...document.querySelectorAll<HTMLButtonElement>(".dialog-foot button")].find((b) => b.textContent === label)!;

it("online with nothing unsent: the last sync, a question, then the copy goes and the session ends", async () => {
  saveSession(session);
  await copy("ana", "l1", false);
  render(<Bar />);
  fireEvent.click(screen.getByText("Sign out"));
  await settle();
  expect(device.sync).toEqual(["last"]);
  // Asked first: Cancel is the default, and nothing is gone yet.
  expect(screen.getByText(/remove this device's copy/)).toBeTruthy();
  const buttons = [...document.querySelectorAll(".dialog-foot button")].map((b) => b.textContent);
  expect(buttons).toEqual(["Cancel", "Sign out"]);
  expect(document.querySelector(".dialog-foot button")!.hasAttribute("data-autofocus")).toBe(true);
  expect(device.deleted).toEqual([]);
  fireEvent.click(dialogButton("Sign out"));
  await settle();
  expect(device.deleted).toEqual(["l1"]);
  expect(loadMirror("ana")).toBeNull();
  expect(requests).toEqual(["POST http://server/api/auth/logout"]);
  expect(loadSession()).toBeNull();
  expect(reload).toHaveBeenCalledOnce();
});

it("nothing unsent, and Cancel: everything stays and the sync goes on", async () => {
  saveSession(session);
  await copy("ana", "l1", false);
  render(<Bar />);
  fireEvent.click(screen.getByText("Sign out"));
  await settle();
  fireEvent.click(dialogButton("Cancel"));
  await settle();
  expect(document.querySelector(".dialog-foot")).toBeNull();
  expect(device.deleted).toEqual([]);
  expect(loadSession()).not.toBeNull();
  expect(device.sync).toEqual(["last", "resume"]);
  expect(reload).not.toHaveBeenCalled();
});

it("offline with an unsent edit: a dialog whose default is Cancel, which keeps everything", async () => {
  device.offline = true;
  saveSession(session);
  await copy("ana", "l1", true);
  render(<Bar />);
  fireEvent.click(screen.getByText("Sign out"));
  await settle();
  expect(device.sync).toEqual([]);
  expect(screen.getByText("Hero l1")).toBeTruthy();
  const buttons = [...document.querySelectorAll(".dialog-foot button")].map((b) => b.textContent);
  expect(buttons).toEqual(["Cancel", "Sign out and discard"]);
  expect(document.querySelector(".dialog-foot button")!.hasAttribute("data-autofocus")).toBe(true);
  fireEvent.click(screen.getByText("Cancel"));
  await settle();
  expect(device.deleted).toEqual([]);
  expect(loadSession()).not.toBeNull();
  expect(device.sync).toEqual(["resume"]);
  expect(reload).not.toHaveBeenCalled();

  fireEvent.click(screen.getByText("Sign out"));
  await settle();
  fireEvent.click(screen.getByText("Sign out and discard"));
  await settle();
  expect(device.deleted).toEqual(["l1"]);
  expect(loadSession()).toBeNull();
});

it("signing in as another account: its unsent edits are named first; Cancel ends the new session and keeps them", async () => {
  await copy("ana", "l1", true);
  render(<ConnectPage again={{ ...session, expired: true }} />);
  await settle();
  // The address and the account are filled in.
  expect((screen.getByLabelText("Username or email") as HTMLInputElement).value).toBe("ana@example.com");
  fireEvent.change(screen.getByLabelText("Username or email"), { target: { value: "bo" } });
  fireEvent.change(screen.getByLabelText("Password"), { target: { value: "pw" } });
  fireEvent.click(screen.getByText("Sign in"));
  await settle();
  expect(screen.getByText("Hero l1")).toBeTruthy();
  fireEvent.click(screen.getByText("Cancel"));
  await settle();
  expect(requests).toContain("POST http://server/api/auth/logout");
  expect(device.files.has("l1")).toBe(true);
  expect(loadSession()).toBeNull();

  fireEvent.change(screen.getByLabelText("Password"), { target: { value: "pw" } });
  fireEvent.click(screen.getByText("Sign in"));
  await settle();
  fireEvent.click(screen.getByText("Discard and sign in"));
  await settle();
  expect(device.deleted).toEqual(["l1"]);
  expect(loadMirror("ana")).toBeNull();
  expect(loadSession()).toMatchObject({ token: "new", user: { id: "bo" } });
  expect(reload).toHaveBeenCalledOnce();
});

it("the same account signing in again keeps its copy for the next sync", async () => {
  await copy("ana", "l1", true);
  vi.stubGlobal("fetch", vi.fn(async (url: string) => (url.endsWith("/api/server")
    ? new Response(JSON.stringify({ app: "borealis-party" }), { status: 200 })
    : new Response(JSON.stringify({ token: "new", user: user("ana") }), { status: 200 }))));
  render(<ConnectPage again={{ ...session, expired: true }} />);
  await settle();
  fireEvent.change(screen.getByLabelText("Password"), { target: { value: "pw" } });
  fireEvent.click(screen.getByText("Sign in"));
  await settle();
  expect(device.deleted).toEqual([]);
  expect(loadMirror("ana")).not.toBeNull();
  expect(loadSession()).toEqual({ baseUrl: "http://server", token: "new", user: user("ana") });
});

it("the account turned off: the sign-in says so and the copy is kept, marked for the banner", async () => {
  saveSession({ ...session, expired: true });
  await copy("ana", "l1", true);
  vi.stubGlobal("fetch", vi.fn(async (url: string) => (url.endsWith("/api/server")
    ? new Response(JSON.stringify({ app: "borealis-party" }), { status: 200 })
    : new Response(JSON.stringify({ code: "account_disabled", message: "This account has been disabled." }), { status: 403 }))));
  render(<ConnectPage again={{ ...session, expired: true }} />);
  await settle();
  fireEvent.change(screen.getByLabelText("Password"), { target: { value: "pw" } });
  fireEvent.click(screen.getByText("Sign in"));
  await settle();
  expect(screen.getByText("This account has been disabled.")).toBeTruthy();
  expect(loadSession()).toMatchObject({ expired: true, blocked: true });
  expect(device.files.has("l1")).toBe(true);
  fireEvent.click(screen.getByText("Keep playing offline"));
  expect(reload).toHaveBeenCalledOnce();
});
