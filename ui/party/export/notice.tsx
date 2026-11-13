import { createRoot } from "react-dom/client";
import { MessageBox } from "../../src/components/controls/Dialog";
import { OverlayHostContext } from "../../src/components/controls/OverlayHost";

/**
 * An app message box with one OK button, for code outside React (an API override). It renders into the window root,
 * as upstream's dialogs do, so Back closes it. Resolves when it is closed.
 */
export function showNotice(title: string, message: string): Promise<void> {
  return new Promise((resolve) => {
    const holder = document.createElement("div");
    holder.setAttribute("data-party-notice", "");
    document.body.appendChild(holder);
    const root = createRoot(holder);
    const host = document.querySelector<HTMLElement>(".aur-window");
    const done = () => {
      root.unmount();
      holder.remove();
      resolve();
    };
    root.render(
      <OverlayHostContext.Provider value={host}>
        <MessageBox title={title} message={message} buttons={[{ id: "ok", label: "OK" }]} onResult={done} />
      </OverlayHostContext.Provider>,
    );
  });
}
