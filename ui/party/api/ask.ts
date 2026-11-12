import { createElement, type ReactNode } from "react";
import { createRoot } from "react-dom/client";
import { MessageBox, type MessageBoxButton } from "../../src/components/controls/Dialog";
import { OverlayHostContext } from "../../src/components/controls/OverlayHost";

/**
 * An app message box for code outside React (an API override), answering the id of the button chosen. Closing it any
 * other way (Back, Escape, the close button) answers the last button's id, as upstream's MessageBox does. It renders
 * into the window root like upstream's dialogs, so Back closes it.
 */
export function ask(title: string, message: ReactNode, buttons: MessageBoxButton[]): Promise<string> {
  return new Promise((resolve) => {
    const holder = document.createElement("div");
    holder.setAttribute("data-party-ask", "");
    document.body.appendChild(holder);
    const root = createRoot(holder);
    const host = document.querySelector<HTMLElement>(".aur-window");
    const done = (id: string) => {
      root.unmount();
      holder.remove();
      resolve(id);
    };
    root.render(createElement(OverlayHostContext.Provider, { value: host }, createElement(MessageBox, { title, message, buttons, onResult: done })));
  });
}
