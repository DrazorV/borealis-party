import { Alert, Button } from "../../src/components";
import { PhoneCase } from "../../src/lib/phoneCase";
import type { DataFilesTab as UpstreamDataFiles } from "../../src/pages/settings/DataFilesTab";
import { DescCheck, SectionDesc, SectionTitle } from "../../src/pages/settings/rows";

export { heldPath, type HeldFolder, type HeldPicks } from "../../src/pages/settings/DataFilesTab";

/**
 * DATA FILES on a server: upstream's tab without the documents and secondary content folders and the logs, which are
 * the server's. An account's own files come in by upload on the Content page.
 */
export function DataFilesTab({ form, set, clearCache, cacheCleared, error }: Parameters<typeof UpstreamDataFiles>[0]) {
  return (
    <>
      <h3 className="aur-set-title13 first flush"><PhoneCase>DATA FILES</PhoneCase></h3>
      <SectionDesc>Your additional content settings.</SectionDesc>
      <SectionTitle>ADDITIONAL CONTENT</SectionTitle>
      <SectionDesc>Manage additional content added through index files.</SectionDesc>
      <DescCheck spaced title="Automatic Updates" description="Check your index files and 5etools sources for updates when you open Borealis Party, at most once a day." checked={form.autoContentUpdate} onChange={(v) => set({ autoContentUpdate: v })} />
      <DescCheck spaced title="Content Import Notice" description="Remind me to import only content I own before each content import." checked={form.showImportNotice} onChange={(v) => set({ showImportNotice: v })} />
      <div className="aur-set-left-clear">
        <Button disabled={cacheCleared} onClick={clearCache}>Clear Cache</Button>
      </div>
      {error && <Alert tone="danger" role="alert">{error}</Alert>}
    </>
  );
}
