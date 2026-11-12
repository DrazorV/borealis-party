import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeAll, describe, expect, it } from "vitest";
import { MockApi } from "../../src/api/mock";
import { setPhoneOsForTests, type PhoneOs } from "../../src/lib/phone";
import { renderApp } from "../../src/test/utils";
import { hideUnofferedRows } from "../entry/hideTransfer";
import { CODE_BUTTON, DOWNLOAD, EXPORT_TITLE, LICENCE_LEAD, PARTY_HELP, PHONE_TITLE, setPackListerForTests, UPSTREAM_HELP } from "./exportDialog";

// Every party client gets upstream's phone path of the dialog: the server reports `mobile` for all of them.
beforeAll(() => hideUnofferedRows());

afterEach(() => {
  setPhoneOsForTests(null);
  setPackListerForTests(null);
});

const PACKS = [
  { id: "srd", name: "SRD", redistributable: undefined },
  { id: "my-books", name: "My books", redistributable: false },
  { id: "homebrew", name: "Homebrew", redistributable: true },
];

async function openExport(os: PhoneOs | null, packs = PACKS) {
  setPhoneOsForTests(os);
  setPackListerForTests(async () => packs);
  const user = userEvent.setup();
  renderApp("/characters", { api: new MockApi({ aiChunkDelayMs: 2, rng: () => 0.5, mobile: true }) });
  await user.click(await screen.findByRole("menuitem", { name: "File" }));
  await user.click(await screen.findByRole("menuitem", { name: "Export to bundle..." }));
  const dlg = await screen.findByRole("dialog", { name: os === null ? EXPORT_TITLE : PHONE_TITLE });
  return { user, dlg };
}

const helpLines = (dlg: HTMLElement) => [...dlg.querySelectorAll(".aur-set-help")].map((p) => p.textContent?.trim());

describe("E9: off the phone apps", () => {
  it('the dialog says "Export to bundle" and its button "Download"; the help says bundles are for other players too', async () => {
    const { dlg } = await openExport(null);
    expect(within(dlg).getByRole("heading").textContent).toBe(EXPORT_TITLE);
    expect(within(dlg).getByRole("button", { name: DOWNLOAD })).toBeTruthy();
    expect(within(dlg).queryByRole("button", { name: "Share" })).toBeNull();
    expect(helpLines(dlg)).toContain(PARTY_HELP);
    expect(helpLines(dlg)).not.toContain(UPSTREAM_HELP);
    expect(PARTY_HELP).toContain("other players");
  });
});

describe("E4: share with a code", () => {
  it.each<PhoneOs | null>([null, "android", "ios"])("%s: the dialog offers a code instead of a file, which closes it and opens the share page", async (os) => {
    const { user, dlg } = await openExport(os);
    const button = await within(dlg).findByRole("button", { name: CODE_BUTTON });
    expect(dlg.querySelectorAll("[data-party-share-code]")).toHaveLength(1);
    await user.click(button);
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(window.location.hash).toBe("#/share");
    window.location.hash = "";
  });
});

describe("iOS", () => {
  it("keeps upstream's share wording, with the party help line", async () => {
    const { dlg } = await openExport("ios");
    expect(within(dlg).getByRole("button", { name: "Share" })).toBeTruthy();
    expect(within(dlg).queryByRole("button", { name: DOWNLOAD })).toBeNull();
    expect(helpLines(dlg)).toContain(PARTY_HELP);
  });
});

describe("E6: packs marked not redistributable", () => {
  it("get a warning chip while packs go into the file, and none with No content packs", async () => {
    const { user, dlg } = await openExport(null);
    const note = await within(dlg).findByRole("note");
    expect(note.textContent).toContain(LICENCE_LEAD);
    expect([...note.querySelectorAll(".chip.warning")].map((c) => c.textContent)).toEqual(["My books"]);
    // Right under the packs' help line.
    expect(note.previousElementSibling?.textContent?.trim()).toBe(PARTY_HELP);

    await user.selectOptions(within(dlg).getByLabelText("CONTENT PACKS"), "none");
    await waitFor(() => expect(within(dlg).queryByRole("note")).toBeNull());
    await user.selectOptions(within(dlg).getByLabelText("CONTENT PACKS"), "all");
    expect(await within(dlg).findByRole("note")).toBeTruthy();
  });

  it("no chip when every pack may travel", async () => {
    const { dlg } = await openExport(null, PACKS.filter((p) => p.redistributable !== false));
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(within(dlg).queryByRole("note")).toBeNull();
  });
});
