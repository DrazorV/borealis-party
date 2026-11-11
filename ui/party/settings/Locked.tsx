import { type ReactNode, useLayoutEffect, useRef } from "react";
import { Alert } from "../../src/components";
import { SET_BY_DM } from "./scope";

type Pick = (root: HTMLElement) => HTMLElement[];

/** Every form control, and no buttons: a tab's buttons open other screens. */
export const formControls: Pick = (root) => [...root.querySelectorAll<HTMLElement>("input, select, textarea")];

/**
 * The control a field label belongs to, with its label. Upstream's fields carry no attribute to match, so they are found
 * by their label as it appears (phones change its case). If upstream rewords one, it simply comes back editable and the
 * server refuses the change.
 */
export function field(label: string): { controls: Pick; rows: Pick } {
  const find = (root: HTMLElement) => {
    const want = label.toLowerCase();
    const found = [...root.querySelectorAll<HTMLLabelElement>("label.aur-set-field")].find((l) => l.textContent?.trim().toLowerCase() === want);
    const control = found?.htmlFor ? root.ownerDocument.getElementById(found.htmlFor) : null;
    return { found, control };
  };
  return {
    controls: (root) => { const { control } = find(root); return control ? [control] : []; },
    rows: (root) => {
      const { found, control } = find(root);
      return [found, control?.closest<HTMLElement>(".aur-combo") ?? control].filter((e): e is HTMLElement => !!e);
    },
  };
}

/**
 * Upstream's tab with some of its controls disabled (`read`) or taken out (`hide`), kept so while the tab re-renders.
 * `note` says why, above the tab.
 */
export function Locked({ children, mode, pick, note }: { children: ReactNode; mode: "read" | "hide" | null; pick: Pick; note?: boolean }) {
  const ref = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const root = ref.current;
    if (!root || !mode) return;
    const apply = () => {
      for (const element of pick(root)) {
        if (mode === "hide") element.classList.add("party-hidden");
        else if (element instanceof HTMLInputElement || element instanceof HTMLSelectElement || element instanceof HTMLTextAreaElement) element.disabled = true;
      }
    };
    apply();
    const watcher = new MutationObserver(apply);
    watcher.observe(root, { childList: true, subtree: true });
    return () => watcher.disconnect();
  }, [mode, pick]);
  return (
    <>
      {note && mode === "read" && <Alert tone="info" role="status">{SET_BY_DM}</Alert>}
      <div ref={ref} className={`party-locked ${mode === "read" ? "read" : ""}`}>{children}</div>
    </>
  );
}
