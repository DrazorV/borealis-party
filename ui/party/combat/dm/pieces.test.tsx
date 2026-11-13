import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { ConditionChips } from "./ConditionToggles";
import { HpBar } from "./HpBar";
import { RollInput } from "./RollInput";
import { healthBand } from "./common";

describe("shared combat pieces", () => {
  it("RollInput: a typed table roll, or a roll in the app (D21, D43)", () => {
    const got: unknown[] = [];
    render(<RollInput kind="d20" label="Initiative" modifier={2} onSubmit={(i) => got.push(i)} />);
    fireEvent.change(screen.getByLabelText("Initiative: typed result"), { target: { value: "17" } });
    fireEvent.click(screen.getByRole("button", { name: "Enter" }));
    fireEvent.click(screen.getByRole("button", { name: "Roll in app" }));
    expect(got).toEqual([{ kind: "typed", total: 17 }, { kind: "roll", modifier: 2, mode: "normal" }]);
  });

  it("RollInput for dice: totals in range, or notation the server rolls", () => {
    const got: unknown[] = [];
    render(<RollInput kind="dice" label="Recharge" notation="1d6" min={1} max={6} onSubmit={(i) => got.push(i)} />);
    const box = screen.getByLabelText("Recharge: typed result");
    fireEvent.change(box, { target: { value: "9" } });
    fireEvent.click(screen.getByRole("button", { name: "Enter" }));
    expect(got).toEqual([]);
    expect(box.getAttribute("aria-invalid")).toBe("true");
    fireEvent.change(box, { target: { value: "5" } });
    fireEvent.click(screen.getByRole("button", { name: "Enter" }));
    fireEvent.click(screen.getByRole("button", { name: "Roll in app" }));
    expect(got).toEqual([{ kind: "typed", total: 5 }, { kind: "roll", notation: "1d6" }]);
  });

  it("the HP band follows the party card's rule (D6)", () => {
    expect(healthBand({ current: 10, max: 20 })).toBe("bloodied");
    expect(healthBand({ current: 11, max: 20 })).toBe("unhurt");
    expect(healthBand({ current: 0, max: 20 })).toBe("down");
    render(<HpBar band="bloodied" bandOnly label="Goblin" />);
    expect(screen.getByRole("meter", { name: "Goblin" }).getAttribute("aria-valuetext")).toBe("Bloodied");
  });

  it("condition chips toggle, and an immune condition can't be set", () => {
    const toggle = vi.fn();
    render(<ConditionChips active={["prone"]} immune={["frightened"]} onToggle={toggle} />);
    fireEvent.click(screen.getByRole("button", { name: "Prone" }));
    expect(toggle).toHaveBeenCalledWith("prone", false);
    expect((screen.getByRole("button", { name: "Frightened" }) as HTMLButtonElement).disabled).toBe(true);
  });

});
