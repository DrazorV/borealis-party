import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { FiveToolsImportReport } from "../../src/api/contract";
import { FiveToolsResult } from "./FiveToolsResult";

const notes = (n: number, reason: string) => Array.from({ length: n }, (_, i) => ({ element: `Creature ${i}|SRDX`, kind: "monster", reason }));
const report = (over: Partial<FiveToolsImportReport> = {}): FiveToolsImportReport =>
  ({ converted: 16746, replaced: 0, partial: [], skipped: [], packs: [{ id: "a", name: "SRD Sample" }], ...over }) as never;

describe("FiveToolsResult", () => {
  it("shows a finished import with notes as a success, details folded", () => {
    const { container } = render(<FiveToolsResult report={report({
      partial: [...notes(411, "creature section: variant sidebars not imported"), ...notes(21, "creature section: mythic actions not imported")],
    })} />);
    expect(container.querySelector(".party-report")!.getAttribute("data-tone")).toBe("success");
    expect(screen.getByRole("status").textContent).toBe("Imported 16746 elements into 1 content pack.");
    expect(screen.getByText(/432 details couldn't be carried over/)).toBeTruthy();
    expect(screen.getByText(/Local import — not redistributable.*source address, not as files/)).toBeTruthy();
    expect(screen.queryByText("Variant sidebars")).toBeNull();

    const show = screen.getByRole("button", { name: "Show details" });
    expect(show.getAttribute("aria-expanded")).toBe("false");
    fireEvent.click(show);
    expect(screen.getByRole("button", { name: "Hide details" }).getAttribute("aria-expanded")).toBe("true");
    const group = screen.getByRole("group", { name: "Carried over in part" });
    const toggles = within(group).getAllByRole("button", { expanded: false });
    expect(toggles.map((t) => t.textContent)).toEqual(["411 Variant sidebars", "21 Mythic actions"]);
    // Sentence case, never the importer's raw upper-case labels.
    expect(group.textContent).not.toMatch(/VARIANT|NOT IMPORTED/);
    fireEvent.click(toggles[0]!);
    expect(within(group).getByText(/stats are complete/)).toBeTruthy();
    expect(within(group).getByText("Creature 0 (SRDX)")).toBeTruthy();
    expect(within(group).getByText("and 361 more")).toBeTruthy();
  });

  it("has no details button when there is nothing to tell", () => {
    render(<FiveToolsResult report={report()} />);
    expect(screen.queryByRole("button", { name: "Show details" })).toBeNull();
  });

  it("keeps a real warning for an import that brought nothing in", () => {
    const { container } = render(<FiveToolsResult report={report({ converted: 0, packs: [] })} />);
    expect(container.querySelector(".party-report")!.getAttribute("data-tone")).toBe("warning");
    expect(screen.getByRole("alert").textContent).toMatch(/^Nothing was imported/);
    expect(screen.queryByText(/not redistributable/)).toBeNull();
  });
});
