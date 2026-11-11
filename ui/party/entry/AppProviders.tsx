import type { ComponentProps } from "react";
import { AppProviders as UpstreamProviders } from "../../src/AppProviders";
import { loadSession } from "../api/session";
import { ChangePasswordPage } from "../pages/ChangePasswordPage";
import { ConnectPage } from "../pages/ConnectPage";

/** Nobody signed in on this device: the connect screen instead of the app. A password to replace comes first too. */
export function AppProviders(props: ComponentProps<typeof UpstreamProviders>) {
  const session = loadSession();
  if (!session) return <ConnectPage />;
  if (session.user.mustChangePassword) return <ChangePasswordPage />;
  return <UpstreamProviders {...props} />;
}
