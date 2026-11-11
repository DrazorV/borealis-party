import { ContentTab as UpstreamContent } from "../../src/pages/settings/BorealisTabs";

export * from "../../src/pages/settings/BorealisTabs";

/** CONTENT: the account's packs and indexes. The portrait library and the notes vault are folders on a device. */
export function ContentTab(props: Parameters<typeof UpstreamContent>[0]) {
  return <UpstreamContent {...props} portraits={null} vault={null} />;
}
