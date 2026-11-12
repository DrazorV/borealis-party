import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { offerUpdate } from "../api/desktopUpdate";
import { DesktopUpdate, FIRST_CHECK_MS, RECHECK_MS } from "./DesktopUpdate";

const tauri = vi.hoisted(() => ({
  inApp: true,
  phone: null as string | null,
  calls: [] as { cmd: string; args?: Record<string, unknown> }[],
  answer: {} as Record<string, (args?: Record<string, unknown>) => unknown>,
}));

vi.mock("../../src/lib/tauri", () => ({ isTauri: () => tauri.inApp }));
vi.mock("../../src/lib/phone", () => ({ phoneOs: () => tauri.phone }));
vi.mock("@tauri-apps/api/core", () => ({
  Channel: class { onmessage: (m: unknown) => void = () => {}; },
  invoke: vi.fn(async (cmd: string, args?: Record<string, unknown>) => {
    tauri.calls.push({ cmd, args });
    const answer = tauri.answer[cmd];
    if (!answer) throw new Error(`no ${cmd}`);
    return answer(args);
  }),
}));

const offer = { rid: 7, currentVersion: "0.1.1", version: "0.1.2" };

beforeEach(() => {
  vi.useFakeTimers();
  tauri.inApp = true;
  tauri.phone = null;
  tauri.calls = [];
  tauri.answer = { "plugin:updater|check": () => offer };
});
afterEach(() => vi.useRealTimers());

async function pass(ms: number) {
  await act(async () => { await vi.advanceTimersByTimeAsync(ms); });
}

it("offers a newer version a little after start, and Later hides it", async () => {
  render(<DesktopUpdate />);
  await pass(FIRST_CHECK_MS - 1);
  expect(tauri.calls).toEqual([]);
  await pass(1);
  expect(screen.getByRole("status").textContent).toContain("Borealis Party 0.1.2 is available (this is 0.1.1)");
  fireEvent.click(screen.getByRole("button", { name: "Later" }));
  expect(screen.queryByRole("status")).toBeNull();
});

it("says nothing when the feed has nothing newer, or this copy has no updater, and asks again later", async () => {
  tauri.answer = { "plugin:updater|check": () => null };
  render(<DesktopUpdate />);
  await pass(FIRST_CHECK_MS);
  expect(screen.queryByRole("status")).toBeNull();
  tauri.answer = {}; // a deb or an unpackaged build: the plugin isn't there
  await pass(RECHECK_MS);
  expect(tauri.calls.map((c) => c.cmd)).toEqual(["plugin:updater|check", "plugin:updater|check"]);
  expect(screen.queryByRole("status")).toBeNull();
  tauri.answer = { "plugin:updater|check": () => offer };
  await pass(RECHECK_MS);
  expect(screen.getByRole("status").textContent).toContain("0.1.2 is available");
});

it("asks nothing in a browser tab or on a phone", async () => {
  tauri.inApp = false;
  const { unmount } = render(<DesktopUpdate />);
  await pass(FIRST_CHECK_MS);
  unmount();
  tauri.inApp = true;
  tauri.phone = "android";
  render(<DesktopUpdate />);
  await pass(FIRST_CHECK_MS);
  expect(tauri.calls).toEqual([]);
});

it("Install and restart: downloads that update with progress, then restarts", async () => {
  let channel: { onmessage: (m: unknown) => void } | undefined;
  let finish: () => void = () => {};
  tauri.answer["plugin:updater|download_and_install"] = (args) => {
    channel = args?.onEvent as typeof channel;
    return new Promise<void>((resolve) => { finish = resolve; });
  };
  tauri.answer.restart_app = () => undefined;
  render(<DesktopUpdate />);
  await pass(FIRST_CHECK_MS);
  fireEvent.click(screen.getByRole("button", { name: "Install and restart" }));
  expect(tauri.calls[1]).toMatchObject({ cmd: "plugin:updater|download_and_install", args: { rid: 7 } });
  await act(async () => {
    channel!.onmessage({ event: "Started", data: { contentLength: 200 } });
    channel!.onmessage({ event: "Progress", data: { chunkLength: 50 } });
  });
  expect(screen.getByRole("status").textContent).toContain("Downloading Borealis Party 0.1.2 (25%)");
  await act(async () => { finish(); await Promise.resolve(); });
  expect(tauri.calls.map((c) => c.cmd)).toContain("restart_app");
});

it("a failed install says why and can be tried again", async () => {
  tauri.answer["plugin:updater|download_and_install"] = () => { throw { code: "x", message: "the signature does not match" }; };
  render(<DesktopUpdate />);
  await pass(FIRST_CHECK_MS);
  fireEvent.click(screen.getByRole("button", { name: "Install and restart" }));
  await pass(0);
  expect(screen.getByRole("status").textContent).toContain("The update could not be installed: the signature does not match");
  expect(screen.getByRole("button", { name: "Try again" })).toBeTruthy();
});

it("Help → Check for updates…: shows what it found even after Later, and installs it when asked", async () => {
  tauri.answer["plugin:updater|download_and_install"] = () => new Promise<void>(() => {});
  render(<DesktopUpdate />);
  await pass(FIRST_CHECK_MS);
  fireEvent.click(screen.getByRole("button", { name: "Later" }));
  expect(screen.queryByRole("status")).toBeNull();
  const newer = { rid: 9, currentVersion: "0.1.1", version: "0.1.3" };
  act(() => { expect(offerUpdate({ update: newer, install: false })).toBe(true); });
  expect(screen.getByRole("status").textContent).toContain("Borealis Party 0.1.3 is available");
  act(() => { offerUpdate({ update: newer, install: true }); });
  expect(tauri.calls.at(-1)).toMatchObject({ cmd: "plugin:updater|download_and_install", args: { rid: 9 } });
  expect(screen.getByRole("status").textContent).toContain("Downloading Borealis Party 0.1.3");
});

it("with no update bar shown, an offer reaches nobody", () => {
  expect(offerUpdate({ update: offer, install: true })).toBe(false);
});

it("an offer while a download is under way leaves it alone", async () => {
  tauri.answer["plugin:updater|download_and_install"] = () => new Promise<void>(() => {});
  render(<DesktopUpdate />);
  await pass(FIRST_CHECK_MS);
  fireEvent.click(screen.getByRole("button", { name: "Install and restart" }));
  act(() => { offerUpdate({ update: { ...offer, rid: 8 }, install: false }); });
  act(() => { offerUpdate({ update: { ...offer, rid: 8 }, install: true }); });
  expect(tauri.calls.filter((c) => c.cmd === "plugin:updater|download_and_install")).toHaveLength(1);
  expect(screen.getByRole("status").textContent).toContain("Downloading Borealis Party 0.1.2");
});
