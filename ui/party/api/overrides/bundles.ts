import { phoneOs } from "../../../src/lib/phone";
import { exportBundle, importWatch, shareBundle, looksLikeBundle, openBundleFromImport, pickBundleFile } from "../bundles";
import { override } from "./registry";

override("bundles", (methods, session) => {
  // File > Open bundle...: the browser's file chooser and an upload stand in for the native open panel (../bundles.ts).
  // Every other pick still goes to the server, which does not offer it.
  const serverPick = methods.pickPath;
  methods.pickPath = (args?: Record<string, unknown>) => (args?.purpose === "bundle-file" ? pickBundleFile(session) : serverPick(args));

  // File > Export to bundle...: the server builds the file and the browser's download stands in for the save dialog.
  methods.bundleExport = (args?: Record<string, unknown>) => exportBundle(session, args ?? {});
  // The dialog takes upstream's phone path ("Share as a file") here, as the server reports every client as mobile.
  // Android: ./android.ts.
  methods.bundleShare = async (args?: Record<string, unknown>) => shareBundle(session, args ?? {}, phoneOs() !== null);

  // Start page > Import characters with a .borealis file (or a zip under another name): the Open bundle window instead.
  const serverImport = methods.importCharacter;
  methods.importCharacter = (args?: Record<string, unknown>) => {
    const fileName = String(args?.fileName ?? "");
    const content = String(args?.contentBase64 ?? "");
    return looksLikeBundle(fileName, content) ? openBundleFromImport(session, fileName, content) : serverImport(args);
  };

  // The Open bundle window for such an import: the preview the import already made, and how the window closed.
  const serverOpen = methods.bundleOpen;
  const serverAccept = methods.bundleAccept;
  const serverDiscard = methods.bundleDiscard;
  methods.bundleOpen = async (args?: Record<string, unknown>) => importWatch.open(String(args?.token ?? "")) ?? await serverOpen(args);
  methods.bundleAccept = async (args?: Record<string, unknown>) => {
    const out = await serverAccept(args);
    importWatch.settle(String(args?.bundleId ?? ""), "accepted");
    return out;
  };
  methods.bundleDiscard = async (args?: Record<string, unknown>) => {
    importWatch.settle(String(args?.bundleId ?? ""), "cancelled");
    return await serverDiscard(args);
  };
});
