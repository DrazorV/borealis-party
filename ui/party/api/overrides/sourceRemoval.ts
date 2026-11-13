import { createElement, useState } from "react";
import { Check } from "../../../src/components/Fields";
import { errorMessage } from "../../../src/state/useAsync";
import { contentSync } from "../../content/contentSync";
import { ask } from "../ask";
import { packsFrom, packSources, removePacks } from "../packRemoval";
import { override } from "./registry";

/**
 * Removing a content source (Additional Content: an index, or a 5etools address) takes its packs too, if the user says
 * so. Upstream removes only the address and keeps the packs, which then stay with nothing to update them. When packs
 * came from the source, an app dialog asks first, with "Also remove the N packs that came from it" ticked; they go in
 * one server request (../packRemoval.ts). Characters using them show missing content, as after any removal.
 *
 * After the question the source and its packs leave the lists at once and the call answers; the server goes on in
 * the background, with a "Removing…" activity row (../../content/contentSync.ts). A refusal brings them back, with the
 * server's message in that row.
 */

export interface SourceRemovalDeps {
  ask: typeof ask;
  packSources: typeof packSources;
  removePacks: typeof removePacks;
}

export const sourceRemovalDeps: SourceRemovalDeps = { ask, packSources, removePacks };

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

/** The dialog's text and its tick box, which writes to `choice`. */
function Body({ url, count, choice }: { url: string; count: number; choice: { packs: boolean } }) {
  const [on, setOn] = useState(true);
  return createElement("div", { className: "party-remove-source" },
    createElement("p", null, `Stop following ${url}?`),
    createElement(Check, {
      label: `Also remove the ${plural(count, "pack")} that came from it`,
      description: "Characters using that content will show it as missing.",
      checked: on,
      onChange: (e: { target: { checked: boolean } }) => { choice.packs = e.target.checked; setOn(e.target.checked); },
    }));
}

/**
 * What a source removal should do: "cancel", or remove the source with or without its packs (their ids). No packs
 * came from it: nothing to ask.
 */
export async function askRemoval(url: string, deps: SourceRemovalDeps = sourceRemovalDeps): Promise<"cancel" | string[]> {
  const ids = await deps.packSources().then((s) => packsFrom(s, url), () => []);
  if (ids.length === 0) return [];
  const choice = { packs: true };
  const answer = await deps.ask("Remove source", createElement(Body, { url, count: ids.length, choice }), [
    { id: "remove", label: "Remove", variant: "danger" },
    { id: "cancel", label: "Cancel" },
  ]);
  if (answer !== "remove") return "cancel";
  return choice.packs ? ids : [];
}

const hostOf = (url: string) => { try { return new URL(url).host || url; } catch { return url; } };

override("sourceRemoval", (methods) => {
  for (const [name, list] of [["remove5etoolsUrl", "list5etoolsUrls"], ["removeIndex", "listIndexes"]] as const) {
    const plain = methods[name];
    methods[name] = async (args?: Record<string, unknown>) => {
      const url = String(args?.url ?? "");
      const plan = await askRemoval(url);
      // Cancelled: nothing changes; the caller gets the list as it is.
      if (plan === "cancel") return methods[list]();
      const index = name === "removeIndex";
      const what = `${index ? "the index file" : "the 5etools address"} from ${hostOf(url)}`;
      const packs = plan.length > 0 ? ` and ${plan.length === 1 ? "its pack" : `its ${plan.length} packs`}` : "";
      const op = contentSync.begin(index ? "indexes" : "fivetools", `Removing ${what}${packs}…`,
        index ? { indexes: [url], packs: plan } : { fiveTools: [url], packs: plan });
      void (async () => {
        try {
          await plain(args);
          if (plan.length > 0) await sourceRemovalDeps.removePacks(plan);
          contentSync.done(op, `Removed ${what}${packs}.`);
        } catch (e) {
          contentSync.fail(op, `${what[0]!.toUpperCase()}${what.slice(1)}${packs} ${plan.length > 0 ? "weren't" : "wasn't"} removed: ${errorMessage(e)}`);
        }
      })();
      return methods[list]();
    };
  }
});
