import { beforeEach, describe, expect, it } from "vitest";
import { castToShow, expectCast, resetCastFocus, showCast } from "./castFocus";
import type { PendingCast } from "./spellTypes";

const cast = (id: string, caster: string) => ({ id, caster }) as unknown as PendingCast;

describe("castFocus: which resolution sheet this device opens", () => {
  beforeEach(() => resetCastFocus());

  it("opens the cast this device asked for, not another device's", () => {
    expect(castToShow([cast("other", "mage")])).toBeNull();
    expectCast("mage");
    expect(castToShow([cast("other", "mage"), cast("mine", "mage")])).toBe("mine");
  });

  it("follows the cast to the server's id: the copy shown at once is replaced by the server's answer", () => {
    expectCast("mage");
    expect(castToShow([cast("local-1", "mage")])).toBe("local-1");
    expect(castToShow([cast("srv-9", "mage")])).toBe("srv-9");
    // A render in between without either (an older view) doesn't lose it either.
    resetCastFocus();
    expectCast("mage");
    expect(castToShow([cast("local-2", "mage")])).toBe("local-2");
    expect(castToShow([])).toBeNull();
    expect(castToShow([cast("srv-10", "mage")])).toBe("srv-10");
  });

  it("an applied cast doesn't open the caster's next one from elsewhere after the window", () => {
    const t = Date.now();
    expectCast("mage");
    expect(castToShow([cast("a", "mage")], t)).toBe("a");
    expect(castToShow([], t + 1)).toBeNull();
    expect(castToShow([cast("b", "mage")], t + 61_000)).toBeNull();
  });

  it("closing the sheet by hand keeps it closed", () => {
    expectCast("mage");
    expect(castToShow([cast("a", "mage")])).toBe("a");
    showCast(null);
    expect(castToShow([cast("a", "mage")])).toBeNull();
    expect(castToShow([])).toBeNull();
    expect(castToShow([cast("b", "mage")])).toBeNull();
  });
});
