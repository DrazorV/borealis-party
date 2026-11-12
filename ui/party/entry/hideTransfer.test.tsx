import { render } from "@testing-library/react";
import { MemoryRouter } from "react-router";
import { afterEach, describe, expect, it, vi } from "vitest";
import { MENUS } from "../../src/components/shell/sections";
import { setPhoneOsForTests, type PhoneOs } from "../../src/lib/phone";
import { MorePage } from "../../src/pages/MorePage";
import { ANDROID_LABELS, hideUnofferedRows } from "./hideTransfer";

vi.mock("../../src/state/platform", () => ({
  usePlatform: () => ({ mobile: true, aiAvailable: false, aiEnabled: false, storeBuild: true, ready: true }),
}));

const mode = vi.hoisted(() => ({ offline: false }));
vi.mock("../api/connectivity", () => ({ isOffline: () => mode.offline }));

afterEach(() => { document.body.innerHTML = ""; mode.offline = false; setPhoneOsForTests(null); });

const tick = () => new Promise((resolve) => setTimeout(resolve, 0)); // the observer runs after the change

it("offline, hides Duplicate in a dialog footer; online it stays", async () => {
  const foot = async () => {
    document.body.innerHTML = '<div class="dialog-foot"><button class="btn">Duplicate</button><button class="btn">Cancel</button></div>';
    await tick();
    return [...document.body.querySelectorAll("button")].map((b) => b.classList.contains("party-hidden"));
  };
  hideUnofferedRows();
  expect(await foot()).toEqual([false, false]);
  mode.offline = true;
  expect(await foot()).toEqual([true, false]);
});

async function morePage(os: PhoneOs | null) {
  setPhoneOsForTests(os);
  hideUnofferedRows();
  const { container } = render(<MemoryRouter><MorePage /></MemoryRouter>);
  await tick();
  const shown = (el: Element) => !el.closest(".party-hidden");
  const rows = Array.from(container.querySelectorAll(".v5-row")).filter(shown).map((r) => r.textContent?.trim());
  const devices = container.querySelector('[aria-label="Your other devices"]')!;
  return { container, rows, devices, shown };
}

describe("upstream's More page", () => {
  it("iOS: no device transfer row, and Share as a file comes back (E2)", async () => {
    const { rows, devices, shown, container } = await morePage("ios");
    expect(rows).not.toContain("Send to device");
    expect(rows).not.toContain("Receive from device");
    expect(rows).toContain("Share as a file");
    expect(shown(devices)).toBe(true);
    expect(shown(container.querySelector('[aria-label="More destinations"]')!)).toBe(true);
  });

  it("Android: no bundle row either, and no empty list where they were (E1b)", async () => {
    const { rows, devices, shown, container } = await morePage("android");
    for (const label of ["Send to device", "Receive from device", "Share as a file", "Export to bundle"]) expect(rows).not.toContain(label);
    expect(rows.length).toBeGreaterThan(0);
    expect(devices.classList.contains("party-hidden")).toBe(true);
    expect(shown(container.querySelector('[aria-label="More destinations"]')!)).toBe(true);
  });

  it("off the phone apps the row reads Export to bundle (E9)", async () => {
    const { rows } = await morePage(null);
    expect(rows).toContain("Export to bundle");
    expect(rows).not.toContain("Share as a file");
  });
});

describe("the desktop File menu", () => {
  const fileMenu = async () => {
    hideUnofferedRows();
    document.body.innerHTML = '<ul><li><button class="aur-menu-item">Send to device...</button></li><li><button class="aur-menu-item">Export to bundle...</button></li><li><button class="aur-menu-item">Open bundle...</button></li></ul>';
    await tick();
    return [...document.body.querySelectorAll("li")].map((li) => li.classList.contains("party-hidden"));
  };

  it.each<PhoneOs | null>(["ios", null])("%s: keeps Export to bundle... and Open bundle...", async (os) => {
    setPhoneOsForTests(os);
    expect(await fileMenu()).toEqual([true, false, false]);
  });

  it("Android: Export to bundle... is hidden, Open bundle... stays (E1b)", async () => {
    setPhoneOsForTests("android");
    expect(await fileMenu()).toEqual([true, true, false]);
  });

  it("Android: every bundle export label upstream has today is in the hidden list (fails if one comes back)", () => {
    const labels = MENUS.flatMap((m) => m.entries).map((e) => e.label).filter((l) => /bundle/i.test(l) && !/open/i.test(l));
    expect(labels.length).toBeGreaterThan(0);
    for (const label of [...labels, "Share as a file"]) expect(ANDROID_LABELS.has(label)).toBe(true);
  });
});
