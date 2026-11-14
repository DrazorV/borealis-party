import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { concentrationText, type CardStats } from "../api/overrides/parties";
import { bandOf, Conditions, StatLine } from "./PartiesPage";

/** The roster card's numbers when the server leaves the DM's HP out (0.3.3, party/docs/role-views.md). */
const base: CardStats = { armorClass: 16, initiative: 2, speed: 30, passivePerception: 12, spellDcs: [{ class: "Cleric", dc: 13 }], exhaustion: 0, concentration: null, inspiration: false };

describe("the roster card's numbers", () => {
  it("with hit points: the HP chip first, and a band", () => {
    render(<StatLine stats={{ ...base, hp: 9, hpMax: 27, tempHp: 3 }} />);
    expect(screen.getByLabelText("Hit points 9 of 27, 3 temporary")).toBeTruthy();
    expect(bandOf({ ...base, hp: 9, hpMax: 27 })).toBe("bloodied");
  });

  it("without hit points (the DM's card): no HP chip and no band, the rest as before", () => {
    render(<StatLine stats={base} />);
    expect(screen.queryByLabelText(/^Hit points/)).toBeNull();
    expect(screen.getByLabelText("Armor class 16")).toBeTruthy();
    expect(screen.getByLabelText("Spell save DC 13")).toBeTruthy();
    expect(bandOf(base)).toBeNull();
    expect(document.body.textContent).not.toMatch(/undefined|NaN/);
  });

  it("concentration as the spell or only yes/no", () => {
    expect(concentrationText("Bless")).toBe("Concentrating: Bless");
    expect(concentrationText(true)).toBe("Concentrating");
    expect(concentrationText(false)).toBeNull();
    render(<Conditions stats={{ ...base, concentration: true }} />);
    expect(screen.getByText("Concentrating")).toBeTruthy();
  });
});
