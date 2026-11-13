import { type ReactNode, useId, useState } from "react";
import type { ContentPack } from "../../src/api/contract";
import { Button } from "../../src/components/Button";
import { useConfirm } from "../../src/components/Confirm";
import { Expander } from "../../src/components/controls";
import { ORIGIN_GROUPS, packOrigin } from "../../src/pages/content/PackGroups";
import { errorMessage } from "../../src/state/useAsync";
import { removePacks } from "../api/packRemoval";
import { matches, usePref } from "./compact";
import { contentSync } from "./contentSync";
import "./content.css";

/**
 * Upstream's installed packs grouped by origin (src/pages/content/PackGroups.tsx, the phone layout), with Remove all
 * on each group but the built-in one. Upstream's Additional Content imports this file in place of its own
 * (ui/party/vite.config.ts). The packs go in one server request. They leave the list at once, with a "Removing…" row
 * above it while the server works (./contentSync.ts), and come back with the server's message if it refuses.
 * Since 0.3.1 on desktops too (./packsPlatform.ts), each group remembering whether it is open, with a search in a long one.
 */

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

/** From this many packs on, a group gets a search box. */
export const SEARCH_FROM = 12;

/** One origin's packs, folded to its count; whether it is open is kept on this device (./compact.ts). */
function Group({ origin, label, packs, row, extra }: { origin: string; label: string; packs: ContentPack[]; row: (p: ContentPack) => ReactNode; extra?: ReactNode }) {
  const [open, setOpen] = usePref(`packs.${origin}`, false);
  const [text, setText] = useState("");
  const searchId = useId();
  const shown = packs.filter((p) => matches(p.name, text));
  return (
    <Expander title={`${label} (${packs.length})`} open={open} onToggle={setOpen} headerExtra={extra}>
      {packs.length >= SEARCH_FROM && (
        <label className="party-compact-search party-pack-search" htmlFor={searchId}>
          <span className="sr-only">Search {label} packs</span>
          <input id={searchId} className="input" type="search" placeholder="Search packs" autoComplete="off" spellCheck={false}
            value={text} onChange={(e) => setText(e.target.value)} />
        </label>
      )}
      {shown.length === 0 && <p className="muted small">No pack matches.</p>}
      <ul className="list" aria-label={`${label} packs`}>{shown.map(row)}</ul>
    </Expander>
  );
}

export function PackGroups({ packs, row }: { packs: ContentPack[]; row: (p: ContentPack) => ReactNode }) {
  const confirm = useConfirm();
  const groups = ORIGIN_GROUPS
    .map((g) => ({ ...g, packs: packs.filter((p) => packOrigin(p) === g.origin) }))
    .filter((g) => g.packs.length > 0);

  const removeAll = async (label: string, list: ContentPack[]) => {
    const r = await confirm({
      title: "Remove all",
      body: `Remove all ${plural(list.length, `${label} pack`)}? Characters using their content will show it as missing content. This can't be undone.`,
      confirmLabel: `Remove ${list.length}`,
      danger: true,
    });
    if (r !== "confirm") return;
    const ids = list.map((p) => p.id);
    const what = plural(list.length, `${label} pack`);
    const op = contentSync.begin("packs", `Removing ${what}…`, { packs: ids });
    void removePacks(ids).then(
      (out) => contentSync.done(op, `Removed ${plural(out.removed, "pack")}.`),
      (e) => contentSync.fail(op, `The ${what} ${list.length === 1 ? "wasn't" : "weren't"} removed: ${errorMessage(e)}`),
    );
  };

  return (
    <div className="v4-pack-groups" role="group" aria-label="Content packs by origin">
      {groups.map((g) => (
        <Group
          key={g.origin}
          origin={g.origin}
          label={g.label}
          packs={g.packs}
          row={row}
          extra={g.origin === "bundled" ? undefined : (
            <Button small variant="danger" className="party-remove-all"
              aria-label={`Remove all ${g.label} packs`} onClick={() => void removeAll(g.label, g.packs)}>Remove all</Button>
          )}
        />
      ))}
    </div>
  );
}
