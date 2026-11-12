import { fireEvent, render } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { addNotices, emptyMirror, saveMirror } from "../sync/mirror";
import { memoryStorage } from "../test/memoryStorage";
import { SyncNotes } from "./AccountPage";

beforeEach(() => { vi.stubGlobal("localStorage", memoryStorage()); });
afterEach(() => { vi.unstubAllGlobals(); document.body.innerHTML = ""; });

it("shows nothing when the last sync went well", () => {
  saveMirror(emptyMirror("u1"));
  const { container } = render(<SyncNotes userId="u1" />);
  expect(container.textContent).toBe("");
});

it("shows the last sync's problems and the edits the server did not take, until dismissed", () => {
  saveMirror({ ...emptyMirror("u1"), problems: { c_a: "Your changes to Ana could not be sent: no." } });
  addNotices("u1", ["Ana was deleted on the server after it was changed on this device."]);
  const { container, getByText } = render(<SyncNotes userId="u1" />);
  expect(container.textContent).toContain("Your changes to Ana could not be sent: no.");
  expect(container.textContent).toContain("Ana was deleted on the server");
  fireEvent.click(getByText("Dismiss"));
  expect(container.textContent).not.toContain("Ana was deleted on the server");
  // A problem stays until a sync goes well.
  expect(container.textContent).toContain("could not be sent");
});
