import { type Platform, usePlatform as upstreamPlatform } from "../../src/state/platform";

/**
 * Upstream's Installed packs list (src/pages/content/PacksTab.tsx) groups the packs by origin on phones only and lists
 * them flat on desktops. The party groups them everywhere (./PackGroups.tsx: each group folded to its count, with
 * Remove all), so the page stays a set of summary cards on a desktop too (ui/party/vite.config.ts points PacksTab's
 * platform here). PacksTab's other phone-only parts are its own import controls, which the party hides
 * (./content.css), so nothing else changes.
 */
export type { Platform };

export function usePlatform(): Platform {
  return { ...upstreamPlatform(), mobile: true };
}
