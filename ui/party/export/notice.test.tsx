import { act, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, it } from "vitest";
import { showNotice } from "./notice";

it("an app dialog (no browser alert) with the message and OK; closing it resolves and removes it", async () => {
  let done = false;
  let shown!: Promise<void>;
  act(() => { shown = showNotice("Not on this device yet", "Wait for the account bar to say Synced."); });
  void shown.then(() => { done = true; });
  const dlg = await screen.findByRole("dialog", { name: "Not on this device yet" });
  expect(dlg.textContent).toContain("Wait for the account bar to say Synced.");
  expect(done).toBe(false);
  await userEvent.setup().click(within(dlg).getByRole("button", { name: "OK" }));
  await shown;
  expect(done).toBe(true);
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(document.querySelector("[data-party-notice]")).toBeNull();
});
