import { render } from "@testing-library/react";
import { MemoryRouter } from "react-router";
import { afterEach, expect, it, vi } from "vitest";
import { MorePage } from "../../src/pages/MorePage";
import { hideUnofferedRows } from "./hideTransfer";

vi.mock("../../src/state/platform", () => ({
  usePlatform: () => ({ mobile: true, aiAvailable: false, aiEnabled: false, storeBuild: true, ready: true }),
}));

const mode = vi.hoisted(() => ({ offline: false }));
vi.mock("../api/connectivity", () => ({ isOffline: () => mode.offline }));

afterEach(() => { document.body.innerHTML = ""; mode.offline = false; });

it("offline, hides Duplicate in a dialog footer; online it stays", async () => {
  const foot = async () => {
    document.body.innerHTML = '<div class="dialog-foot"><button class="btn">Duplicate</button><button class="btn">Cancel</button></div>';
    await new Promise((resolve) => setTimeout(resolve, 0)); // the observer runs after the change
    return [...document.body.querySelectorAll("button")].map((b) => b.classList.contains("party-hidden"));
  };
  hideUnofferedRows();
  expect(await foot()).toEqual([false, false]);
  mode.offline = true;
  expect(await foot()).toEqual([true, false]);
});

it("leaves no device transfer row on upstream's More page, and no empty list where they were", async () => {
  hideUnofferedRows();
  const { container } = render(<MemoryRouter><MorePage /></MemoryRouter>);
  await new Promise((resolve) => setTimeout(resolve, 0)); // the observer runs after the render

  const shown = (el: Element) => !el.closest(".party-hidden");
  const rows = Array.from(container.querySelectorAll(".v5-row")).filter(shown).map((r) => r.textContent?.trim());
  expect(rows).not.toContain("Send to device");
  expect(rows).not.toContain("Receive from device");
  expect(rows).not.toContain("Share as a file");
  expect(rows.length).toBeGreaterThan(0);

  const devices = container.querySelector('[aria-label="Your other devices"]');
  expect(devices).not.toBeNull();
  expect(devices?.classList.contains("party-hidden")).toBe(true);
  expect(shown(container.querySelector('[aria-label="More destinations"]')!)).toBe(true);
});
