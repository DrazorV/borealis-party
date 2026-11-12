/**
 * Small sample files for the import tests, SRD content only. `zipOf` writes a stored (uncompressed) zip, which is all
 * the sniffer needs: it reads the central directory, never the data.
 */

const enc = new TextEncoder();

function le(n: number, bytes: number): number[] {
  const out: number[] = [];
  for (let i = 0; i < bytes; i++) out.push(Math.floor(n / 2 ** (8 * i)) & 0xff);
  return out;
}

export function zipOf(entries: Record<string, string>, comment = ""): Uint8Array<ArrayBuffer> {
  const local: number[] = [];
  const central: number[] = [];
  let count = 0;
  for (const [name, text] of Object.entries(entries)) {
    const nameBytes = [...enc.encode(name)];
    const data = [...enc.encode(text)];
    const offset = local.length;
    // CRC is not checked by the sniffer; 0 keeps the writer small.
    local.push(...le(0x04034b50, 4), ...le(20, 2), 0, 0, 0, 0, 0, 0, 0, 0, ...le(0, 4), ...le(data.length, 4), ...le(data.length, 4),
      ...le(nameBytes.length, 2), 0, 0, ...nameBytes, ...data);
    central.push(...le(0x02014b50, 4), ...le(20, 2), ...le(20, 2), 0, 0, 0, 0, 0, 0, 0, 0, ...le(0, 4), ...le(data.length, 4), ...le(data.length, 4),
      ...le(nameBytes.length, 2), 0, 0, 0, 0, 0, 0, 0, 0, ...le(0, 4), ...le(offset, 4), ...nameBytes);
    count++;
  }
  const commentBytes = [...enc.encode(comment)];
  const eocd = [...le(0x06054b50, 4), 0, 0, 0, 0, ...le(count, 2), ...le(count, 2), ...le(central.length, 4), ...le(local.length, 4),
    ...le(commentBytes.length, 2), ...commentBytes];
  return new Uint8Array([...local, ...central, ...eocd]);
}

export const fileOf = (name: string, content: string | Uint8Array<ArrayBuffer>) => new File([content], name);

/** An Aurora character file (XML, `<character>` root). */
export const AURORA_CHARACTER = `<?xml version="1.0" encoding="utf-8"?>
<!-- Aurora character -->
<character version="1.3" preview="false">
  <information><name>Ana the Fighter</name></information>
  <build><elements><element type="Class" name="Fighter" id="ID_CLASS_FIGHTER" /></elements></build>
</character>`;

/** An Aurora content file (XML, `<elements>` root). */
export const AURORA_PACK = `<?xml version="1.0" encoding="utf-8"?>
<elements>
  <info><name>SRD sample</name></info>
  <element name="Longsword" type="Weapon" source="System Reference Document" id="ID_SRD_WEAPON_LONGSWORD" />
</elements>`;

/** An Aurora index file. */
export const AURORA_INDEX = `<?xml version="1.0" encoding="utf-8"?>
<index>
  <info><name>SRD sample index</name><update version="0.0.1"><file name="srd-sample.index" url="https://example.com/srd-sample.index" /></update></info>
  <files><file name="srd-sample.xml" url="https://example.com/srd-sample.xml" /></files>
</index>`;

/** A native character JSON (no content marker). */
export const NATIVE_CHARACTER = JSON.stringify({ name: "Ana", level: 1, classes: [{ id: "ID_CLASS_FIGHTER", level: 1 }] });

/** A Borealis content-pack JSON. */
export const BOREALIS_PACK = JSON.stringify({ format: "borealis-content/1", elements: [{ id: "ID_SRD_WEAPON_LONGSWORD", type: "Weapon", name: "Longsword" }] });

/** 5etools-format homebrew: `_meta` plus a spell. */
export const FIVETOOLS_HOMEBREW = JSON.stringify({
  _meta: { sources: [{ json: "SRDX", abbreviation: "SRDX", full: "SRD sample" }] },
  spell: [{ name: "Magic Missile", source: "SRDX", level: 1 }],
});

export const BUNDLE_ZIP = () => zipOf({
  "manifest.json": JSON.stringify({ format: "borealis-bundle/1" }),
  "characters/ana.json": NATIVE_CHARACTER,
});

export const AURORA_ZIP = () => zipOf({ "srd/srd-sample.xml": AURORA_PACK, "srd/srd-sample.index": AURORA_INDEX });

export const FIVETOOLS_ZIP = () => zipOf({
  "5etools-src/package.json": "{}",
  "5etools-src/data/books.json": JSON.stringify({ book: [] }),
  "5etools-src/data/spells/spells-srdx.json": JSON.stringify({ spell: [] }),
  "__MACOSX/manifest.json": "",
});
