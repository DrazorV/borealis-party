import { describe, expect, it } from "vitest";
import { buildSteps, type Facts, isDismissed, nextStep, progress, serverKeepsDismissal, showChecklist } from "./checklist";

const none: Facts = { content: false, character: false, party: false, encounter: false, ai: false };

describe("first-run checklist", () => {
  it("a player gets content, character and Join a party, plus the optional AI step", () => {
    const steps = buildSteps(none, false);
    expect(steps.map((s) => s.id)).toEqual(["content", "character", "party", "ai"]);
    expect(steps.find((s) => s.id === "party")!.title).toBe("Join a party");
    expect(steps.find((s) => s.id === "ai")!.optional).toBe(true);
    expect(progress(steps)).toEqual({ done: 0, total: 3, complete: false });
  });

  it("a DM gets Create a party and Create an encounter", () => {
    const steps = buildSteps(none, true);
    expect(steps.map((s) => s.id)).toEqual(["content", "character", "party", "encounter", "ai"]);
    expect(steps.find((s) => s.id === "party")!.title).toBe("Create a party");
    expect(steps.find((s) => s.id === "encounter")!.to).toBe("/encounters");
  });

  it("each step links to its page", () => {
    const to = Object.fromEntries(buildSteps(none, true).map((s) => [s.id, s.to]));
    expect(to).toEqual({ content: "/content", character: "/characters", party: "/parties", encounter: "/encounters", ai: "/account" });
  });

  it("steps tick themselves from the facts; unknown reads as not done", () => {
    const steps = buildSteps({ content: true, character: null, party: true, encounter: false, ai: false }, true);
    expect(steps.filter((s) => s.done).map((s) => s.id)).toEqual(["content", "party"]);
    expect(progress(steps)).toEqual({ done: 2, total: 4, complete: false });
    expect(nextStep(steps)!.id).toBe("character");
  });

  it("a server without AI apps leaves the optional step out", () => {
    expect(buildSteps({ ...none, ai: null }, false).map((s) => s.id)).not.toContain("ai");
  });

  it("closes itself once every required step is done, the optional one or not", () => {
    const all = buildSteps({ content: true, character: true, party: true, encounter: false, ai: false }, false);
    expect(progress(all).complete).toBe(true);
    expect(showChecklist(all, false)).toBe(false);
    // The encounter step only counts for a DM.
    const dm = buildSteps({ content: true, character: true, party: true, encounter: false, ai: true }, true);
    expect(showChecklist(dm, false)).toBe(true);
    expect(nextStep(dm)!.id).toBe("encounter");
  });

  it("stays closed once dismissed", () => {
    const steps = buildSteps(none, false);
    expect(showChecklist(steps, false)).toBe(true);
    expect(showChecklist(steps, true)).toBe(false);
  });

  it("dismissal: the account setting or this device's answer", () => {
    expect(isDismissed({ onboardingDismissed: true }, false)).toBe(true);
    expect(isDismissed({ onboardingDismissed: false }, false)).toBe(false);
    expect(isDismissed({ onboardingDismissed: false }, true)).toBe(true);
    expect(isDismissed({}, false)).toBe(false);
    expect(isDismissed(null, false)).toBe(false);
    expect(serverKeepsDismissal({ onboardingDismissed: false })).toBe(true);
    expect(serverKeepsDismissal({ accentColor: "blue" })).toBe(false);
  });

  it("the next step is the first required one not done, then the optional one", () => {
    const steps = buildSteps({ content: true, character: true, party: true, encounter: true, ai: false }, true);
    expect(nextStep(steps)!.id).toBe("ai");
  });
});
