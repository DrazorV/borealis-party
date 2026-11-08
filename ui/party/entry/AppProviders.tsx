import type { ComponentProps } from "react";
import { AppProviders as UpstreamProviders } from "../../src/AppProviders";
import { signedIn } from "../api/session";
import { ConnectPage } from "../pages/ConnectPage";

/** Nobody signed in on this device: the connect screen instead of the app. */
export function AppProviders(props: ComponentProps<typeof UpstreamProviders>) {
  return signedIn() ? <UpstreamProviders {...props} /> : <ConnectPage />;
}
