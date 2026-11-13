/**
 * `react-router` as upstream's shell (`src/components/shell/Chrome.tsx` and `MobileChrome.tsx`) sees it, through the
 * party build's import swap (`party/vite.config.ts`). One change: a `NavLink` whose `aria-current` the shell decides
 * itself keeps that decision.
 *
 * Upstream writes `aria-current={selected ? "page" : undefined}`, but NavLink's own default is `"page"` whenever its
 * route prefix-matches, so an explicit `undefined` changes nothing: on `/content/sources` both Sources and Additional
 * Content (`/content`) were `aria-current="page"`, and both looked selected. Upstream's one-line fix is `: false`
 * instead of `: undefined` in those NavLinks.
 *
 * And a `useLocation` that also follows the Dungeon Master switch (../api/dmRole.ts): both tab bars read the main tabs
 * through it on every draw, and the Encounters tab comes and goes with the switch (entry/partySections.ts).
 */
import { type ComponentProps, forwardRef } from "react";
import { NavLink as RouterNavLink, useLocation as useRouterLocation } from "react-router";
import { useDmRole } from "../api/dmRole";
import { refreshMainTabs } from "./partySections";

export * from "react-router";

export function useLocation(): ReturnType<typeof useRouterLocation> {
  useDmRole();
  refreshMainTabs();
  return useRouterLocation();
}

type Props = ComponentProps<typeof RouterNavLink>;

export const NavLink = forwardRef<HTMLAnchorElement, Props>(function NavLink(props, ref) {
  if (!("aria-current" in props)) return <RouterNavLink ref={ref} {...props} />;
  // The shell decided: not current means no aria-current and no router "active" class either.
  const { className } = props;
  const decided = props["aria-current"] === undefined ? false : props["aria-current"];
  return (
    <RouterNavLink
      ref={ref}
      {...props}
      aria-current={decided}
      className={typeof className === "string" ? () => className : className}
    />
  );
});
