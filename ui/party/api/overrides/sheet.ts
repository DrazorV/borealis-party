import { COMMANDS, type ExportResult, type SheetShare } from "../../../src/api/contract";
import { serverFetch } from "../http";
import { override } from "./registry";

// Phones share the sheet PDF from the web view's own share sheet, with the bytes the server made. Android: ./android.ts.
override("sheet", (methods, session) => {
  methods.shareSheet = async (args?: Record<string, unknown>): Promise<SheetShare> => {
    const file = await serverFetch(session, "POST", `/api/rpc/${COMMANDS.exportSheet}`, args ?? {}) as ExportResult;
    return { fileName: file.fileName, mime: file.mime, shared: false, dataBase64: file.dataBase64 ?? "" };
  };
});
