import type { ReactNode } from "react";
import type { ContentPack } from "../../src/api/contract";
import { Button } from "../../src/components/Button";
import { useConfirm } from "../../src/components/Confirm";
import { Expander } from "../../src/components/controls";
import { ORIGIN_GROUPS, packOrigin } from "../../src/pages/content/PackGroups";
import { errorMessage } from "../../src/state/useAsync";
import { removePacks } from "../api/packRemoval";
import { contentSync } from "./contentSync";
import "./content.css";

/**
 * Upstream's installed packs grouped by origin (src/pages/content/PackGroups.tsx, the phone layout), with Remove all
 * on each group but the built-in one. Upstream's Additional Content imports this file in place of its own
 * (ui/party/vite.config.ts). The packs go in one server request. They leave the list at once, with a "Removing…" row
 * above it while the server works (./contentSync.ts), and come back with the server's message if it refuses.
 */

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

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
        <Expander
          key={g.origin}
          title={`${g.label} (${g.packs.length})`}
          defaultOpen={false}
          headerExtra={g.origin === "bundled" ? undefined : (
            <Button small variant="danger" className="party-remove-all"
              aria-label={`Remove all ${g.label} packs`} onClick={() => void removeAll(g.label, g.packs)}>Remove all</Button>
          )}
        >
          <ul className="list" aria-label={`${g.label} packs`}>{g.packs.map(row)}</ul>
        </Expander>
      ))}
    </div>
  );
}
