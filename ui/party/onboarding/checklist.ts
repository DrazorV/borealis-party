import { IMPORT_PATH } from "../import/door";

/**
 * The first-run checklist (0.3.6): what a new account does first, each step ticking itself once the account has done it
 * (nothing is ticked by hand). The steps are read from what the account has, not from what it clicked: content installed
 * beside the bundled SRD, a character, a party, (a DM) an encounter, and (optional) an AI app over MCP.
 *
 * Closing it is an account setting (`onboardingDismissed`, party/crates/borealis-server/src/settings.rs), so it stays
 * closed on every device. An older server without the setting keeps the answer on the device instead (./OnboardingCard.tsx).
 */

export type StepId = "content" | "character" | "party" | "encounter" | "ai";

/**
 * What the account has, as read from the server. `null`: could not be told (a request failed), which reads as not done;
 * for `ai`, `null` leaves the step out (a server without AI apps).
 */
export interface Facts {
  /** Any content beside the bundled packs: an imported or downloaded pack, homebrew, or an index file. */
  content: boolean | null;
  character: boolean | null;
  /** A party the account is in (a player) or runs (a DM). */
  party: boolean | null;
  /** An encounter in one of the parties the account runs. */
  encounter: boolean | null;
  /** An API token or a connected AI app. */
  ai: boolean | null;
}

export interface Step {
  id: StepId;
  title: string;
  detail: string;
  /** The page that does it. */
  to: string;
  /** The link's words. */
  action: string;
  done: boolean;
  /** Not counted towards "done": the checklist closes itself without it. */
  optional: boolean;
}

/** The steps for the account: a DM gets "Create a party" and "Create an encounter", a player "Join a party". */
export function buildSteps(facts: Facts, dm: boolean): Step[] {
  const steps: Step[] = [
    {
      id: "content", title: "Add content",
      detail: "The SRD is installed. Add your books and sources on Additional Content: files, links or 5etools data.",
      to: IMPORT_PATH, action: "Add content", done: facts.content === true, optional: false,
    },
    {
      id: "character", title: "Create or import a character",
      detail: dm ? "A character of your own, or one to try the rules with." : "Make a new character, or import a .dnd5e file.",
      to: "/characters", action: "Characters", done: facts.character === true, optional: false,
    },
    dm
      ? { id: "party", title: "Create a party", detail: "Make a party on Parties, then send your players its join link.",
          to: "/parties", action: "Parties", done: facts.party === true, optional: false }
      : { id: "party", title: "Join a party", detail: "Ask your DM for a join code or link, then enter it on Parties.",
          to: "/parties", action: "Parties", done: facts.party === true, optional: false },
  ];
  if (dm) {
    steps.push({
      id: "encounter", title: "Create an encounter", detail: "Build your first fight on Encounters: pick a party, add creatures.",
      to: "/encounters", action: "Encounters", done: facts.encounter === true, optional: false,
    });
  }
  if (facts.ai !== null) {
    steps.push({
      id: "ai", title: "Connect an AI app", detail: "Let Claude or another AI app use your account over MCP (Account, AI apps).",
      to: "/account", action: "AI apps", done: facts.ai === true, optional: true,
    });
  }
  return steps;
}

export interface Progress { done: number; total: number; complete: boolean }

/** The required steps done, of all required ones. */
export function progress(steps: Step[]): Progress {
  const required = steps.filter((s) => !s.optional);
  const done = required.filter((s) => s.done).length;
  return { done, total: required.length, complete: done === required.length };
}

/** The checklist shows until it is closed or every required step is done. */
export function showChecklist(steps: Step[], dismissed: boolean): boolean {
  return !dismissed && steps.length > 0 && !progress(steps).complete;
}

/** The first step not done yet: the one the card points at. */
export function nextStep(steps: Step[]): Step | null {
  return steps.find((s) => !s.done && !s.optional) ?? steps.find((s) => !s.done) ?? null;
}

/** The account setting that closes it. */
export const DISMISS_KEY = "onboardingDismissed";

/** Whether the account closed it: the server's setting, or this device's own answer (an older server). */
export function isDismissed(settings: unknown, local: boolean): boolean {
  const saved = settings && typeof settings === "object" ? (settings as Record<string, unknown>)[DISMISS_KEY] : undefined;
  return saved === true || local;
}

/** Whether the server keeps the setting (an older one doesn't answer it). */
export function serverKeepsDismissal(settings: unknown): boolean {
  return Boolean(settings && typeof settings === "object" && typeof (settings as Record<string, unknown>)[DISMISS_KEY] === "boolean");
}
