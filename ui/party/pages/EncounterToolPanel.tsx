import { type ReactNode, useEffect, useState } from "react";
import { IconButton } from "../../src/components/Button";
import { Modal } from "../../src/components/Modal";
import { usePhone } from "../../src/lib/phone";

/** Where the encounter workspace uses the whole screen: a list, the creatures, a tool panel and the side column. */
export const WIDE_QUERY = "(min-width: 1100px)";

/** True on a big screen (a DM preparing a session); phones never are. jsdom (no matchMedia) is not. */
export function useWide(): boolean {
  const get = () => typeof window !== "undefined" && typeof window.matchMedia === "function" && window.matchMedia(WIDE_QUERY).matches;
  const [wide, setWide] = useState(get);
  const phone = usePhone();
  useEffect(() => {
    if (typeof window.matchMedia !== "function") return;
    const mq = window.matchMedia(WIDE_QUERY);
    const on = () => setWide(mq.matches);
    mq.addEventListener("change", on);
    on();
    return () => mq.removeEventListener("change", on);
  }, []);
  return wide && !phone;
}

/**
 * One tool beside the creatures: the creature picker, and later the AI assistant (part C). On a big screen it is a
 * column of the builder (`.enc-tool`, between the creatures and the side column), so the DM sees the creatures while
 * adding; one tool is open at a time. Smaller screens get the app's full-screen sheet.
 */
export function EncounterToolPanel({ title, onClose, footer, className = "", children }: {
  title: string;
  onClose: () => void;
  footer?: ReactNode;
  className?: string;
  children: ReactNode;
}) {
  const wide = useWide();
  if (!wide) return <Modal title={title} full className={className} onClose={onClose} footer={footer}>{children}</Modal>;
  return (
    <aside className={`card enc-tool ${className}`} aria-label={title}
      onKeyDown={(e) => { if (e.key === "Escape") { e.stopPropagation(); onClose(); } }}>
      <div className="card-head">
        <h2>{title}</h2>
        <span className="spacer" />
        <IconButton icon="x" label={`Close ${title}`} onClick={onClose} />
      </div>
      <div className="enc-tool-body">{children}</div>
      {footer && <div className="enc-tool-foot">{footer}</div>}
    </aside>
  );
}
