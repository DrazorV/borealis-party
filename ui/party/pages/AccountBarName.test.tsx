import { act, render } from "@testing-library/react";
import { MemoryRouter } from "react-router";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { saveSession } from "../api/session";
import { memoryStorage } from "../test/memoryStorage";
import { AccountBar } from "./AccountBar";

// The Live chip (0.3.0) reads the open character, which needs the app's CharacterProvider; this test is about the name.
vi.mock("../combat/player/LiveChip", () => ({ LiveChip: () => null }));

beforeEach(() => {
  vi.stubGlobal("localStorage", memoryStorage());
  vi.stubGlobal("sessionStorage", memoryStorage());
  saveSession({
    baseUrl: "http://party.test", token: "t",
    user: { id: "u1", username: "drazor", email: null, admin: true, mustChangePassword: false },
  });
  document.body.innerHTML = `<div class="aur-window"><div class="aur-content"></div></div>`;
});

afterEach(() => {
  vi.unstubAllGlobals();
  document.body.innerHTML = "";
});

it("the bar under the tabs names no account; the Account button's title still says who is signed in", async () => {
  await act(async () => {
    render(<MemoryRouter initialEntries={["/characters"]}><AccountBar /></MemoryRouter>);
    // The host's MutationObserver answers the bar's own insertion.
    await new Promise((r) => setTimeout(r, 20));
  });
  const bar = document.querySelector(".party-bar");
  expect(bar).not.toBeNull();
  expect(bar!.textContent).not.toContain("drazor");
  expect(bar!.querySelector(".party-bar-name")).toBeNull();
  const account = [...bar!.querySelectorAll("button")].find((b) => b.textContent === "Account");
  expect(account?.title).toBe("Signed in as drazor");
});
