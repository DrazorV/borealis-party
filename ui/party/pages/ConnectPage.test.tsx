import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { keepJoinCode, keptJoinCode, partiesDeps, resetParties } from "../api/overrides/parties";
import { memoryStorage } from "../test/memoryStorage";

const signUps = vi.hoisted(() => [] as unknown[][]);
const answer = vi.hoisted(() => ({ value: {} as Record<string, unknown> }));
vi.mock("../api/party", () => ({
  checkServer: async () => ({ app: "borealis-party", version: "1", openRegistration: true, requireApproval: true, email: false }),
  signIn: async () => { throw new Error("not used"); },
  signUp: async (...args: unknown[]) => { signUps.push(args); return answer.value; },
  endSession: async () => undefined,
}));

const { ConnectPage } = await import("./ConnectPage");

const previews: unknown[] = [];
beforeEach(() => {
  vi.stubGlobal("localStorage", memoryStorage());
  signUps.length = 0;
  previews.length = 0;
  partiesDeps.fetch = async (session, _method, path, body) => {
    previews.push({ session, path, body });
    if ((body as { code: string }).code === "BADCODE1") throw { code: "not_found", message: "Not found." };
    return { party: { name: "Tuesday Party", dmName: "dana", ruleset: "dnd2024" }, expiresAt: Date.UTC(2026, 10, 19) / 1000 };
  };
});
afterEach(() => {
  resetParties();
  vi.unstubAllGlobals();
});

async function signUp(code?: string) {
  await act(async () => { render(<ConnectPage />); });
  fireEvent.click(await screen.findByRole("button", { name: "Create an account" }));
  fireEvent.change(screen.getByLabelText("Username"), { target: { value: "newbie" } });
  fireEvent.change(screen.getByLabelText("Email"), { target: { value: "n@example.com" } });
  fireEvent.change(screen.getByLabelText("Password"), { target: { value: "long enough pw" } });
  if (code !== undefined) fireEvent.change(screen.getByLabelText("Join code (optional)"), { target: { value: code } });
  fireEvent.click(screen.getByRole("button", { name: "Create account" }));
}

it("a pending account's join code is kept on the server until an admin approves it (D39), and the wait says so", async () => {
  keepJoinCode("abcd-efgh");
  answer.value = { status: "pending", message: "Your account is waiting for approval." };
  await signUp();
  const notice = await screen.findByRole("status");
  expect(notice.textContent).toMatch(/waiting for approval\. Your join code for Tuesday Party is kept: once an admin approves you, sign in and pick the character that joins \(until /);
  // Asked without a session, before the sign-up.
  expect(previews).toEqual([{ session: { baseUrl: "", token: "" }, path: "/api/join/preview", body: { code: "ABCDEFGH" } }]);
  expect(signUps[0]).toEqual(["", "newbie", "n@example.com", "long enough pw", "ABCDEFGH"]);
  // The server has it now: the device forgets it.
  expect(keptJoinCode()).toBe("");
});

it("without a code nothing is sent, and the wait says only that", async () => {
  answer.value = { status: "pending", message: "Your account is waiting for approval." };
  await signUp();
  expect((await screen.findByRole("status")).textContent).toBe("Your account is waiting for approval.");
  expect(signUps[0]).toEqual(["", "newbie", "n@example.com", "long enough pw", undefined]);
  expect(previews).toEqual([]);
});

it("a code that isn't valid stops the sign-up and says so, keeping what was typed", async () => {
  await signUp("badcode1");
  expect((await screen.findByRole("alert")).textContent).toMatch(/join code isn't valid/);
  expect(signUps).toEqual([]);
  expect((screen.getByLabelText("Join code (optional)") as HTMLInputElement).value).toBe("badcode1");
});

it("waiting for the email link names that instead", async () => {
  answer.value = { status: "pending", awaitingEmail: true, message: "Check your email." };
  await signUp("wxyz1234");
  expect((await screen.findByRole("status")).textContent).toMatch(/once your email address is confirmed/);
});

it("a typed code goes with the sign-up too", async () => {
  answer.value = { status: "pending", message: "Waiting." };
  await signUp("wxyz 1234");
  await screen.findByRole("status");
  expect(signUps[0]?.[4]).toBe("WXYZ1234");
});
