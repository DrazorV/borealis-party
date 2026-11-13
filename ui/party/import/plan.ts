import { sniffFile, type FileKind } from "./sniff";

/**
 * Which importer each chosen file goes to (plan I9). Content files of one pick are converted together, so references
 * between them resolve; 5etools-format JSON files are staged together too (homebrew can build on another file); every
 * character, bundle and 5etools zip is its own import.
 */
export interface ImportPlan {
  /** `import_character`. */
  characters: File[];
  /** `stage_bundle`, then the Open bundle window. */
  bundles: File[];
  /** `preview_import_pack`, then upstream's import preview, which installs. */
  packs: File[];
  /** `stage_5etools_files`. */
  fiveToolsFiles: File[];
  /** `stage_5etools_zip`, one each. */
  fiveToolsZips: File[];
  /** Nothing reads them. */
  others: File[];
  kinds: Map<File, FileKind>;
}

export async function planImport(files: readonly File[], sniff: (f: File) => Promise<FileKind> = sniffFile): Promise<ImportPlan> {
  const plan: ImportPlan = { characters: [], bundles: [], packs: [], fiveToolsFiles: [], fiveToolsZips: [], others: [], kinds: new Map() };
  for (const file of files) {
    const kind = await sniff(file).catch((): FileKind => "other");
    plan.kinds.set(file, kind);
    switch (kind) {
      case "character": plan.characters.push(file); break;
      case "bundle": plan.bundles.push(file); break;
      case "pack": plan.packs.push(file); break;
      case "fivetools": plan.fiveToolsFiles.push(file); break;
      case "fivetools-zip": plan.fiveToolsZips.push(file); break;
      default: plan.others.push(file);
    }
  }
  return plan;
}

/** How a result row names what the file was taken for. */
export const KIND_LABEL: Record<FileKind, string> = {
  character: "Character",
  bundle: "Borealis bundle",
  pack: "Aurora content",
  fivetools: "5etools-format data",
  "fivetools-zip": "5etools-format data (.zip)",
  other: "Not importable",
};

/** Content (packs and 5etools data) asks for upstream's content notice first; characters and bundles don't. */
export const needsNotice = (plan: ImportPlan) => plan.packs.length + plan.fiveToolsFiles.length + plan.fiveToolsZips.length > 0;

/** Upstream's messages start in lower case where they follow a file name ("it is a Borealis bundle..."). */
export const sentence = (text: string) => (text ? text[0]!.toUpperCase() + text.slice(1) : text);
