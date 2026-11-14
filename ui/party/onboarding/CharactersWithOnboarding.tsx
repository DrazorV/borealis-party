import { CharactersPage as UpstreamCharactersPage } from "../../src/pages/characters/CharactersPage";
import { OnboardingCard } from "./OnboardingCard";

/**
 * The Characters page (the app's start) with the first-run checklist above upstream's own page (./OnboardingCard.tsx).
 * Upstream's App imports this file in place of its CharactersPage (ui/party/vite.config.ts).
 */
export function CharactersPage() {
  return (
    <>
      <OnboardingCard />
      <UpstreamCharactersPage />
    </>
  );
}
